//! File transfers (drag & drop upload / download) with progress and cancel.
//!
//! SSH tabs open one SFTP channel on the existing connection for the duration of a
//! transfer and close it afterwards. Local tabs simply copy files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ssh::SshState;

#[derive(Default)]
pub struct TransferState {
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

enum Side {
    Local,
    Remote(SftpSession),
}

type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

impl Side {
    fn is_remote(&self) -> bool {
        matches!(self, Side::Remote(_))
    }

    /// Join a '/'-separated relative path onto a base path of this side.
    fn join(&self, base: &str, rel: &str) -> String {
        match self {
            Side::Remote(_) => format!("{}/{}", base.trim_end_matches('/'), rel),
            Side::Local => {
                let mut p = PathBuf::from(base);
                rel.split('/').filter(|c| !c.is_empty()).for_each(|c| p.push(c));
                p.to_string_lossy().into_owned()
            }
        }
    }

    fn basename(&self, path: &str) -> String {
        match self {
            Side::Remote(_) => path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string(),
            Side::Local => Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string()),
        }
    }

    /// (is_dir, size); None for things we skip (sockets, dangling links, dir symlinks).
    async fn stat(&self, path: &str) -> Result<Option<(bool, u64)>, String> {
        match self {
            Side::Local => {
                let lm = tokio::fs::symlink_metadata(path).await.map_err(|e| e.to_string())?;
                if lm.file_type().is_symlink() {
                    return Ok(match tokio::fs::metadata(path).await {
                        Ok(m) if m.is_file() => Some((false, m.len())),
                        _ => None, // dir symlink (loop risk) or dangling
                    });
                }
                Ok(if lm.is_dir() {
                    Some((true, 0))
                } else if lm.is_file() {
                    Some((false, lm.len()))
                } else {
                    None
                })
            }
            Side::Remote(s) => {
                let lm = s.symlink_metadata(path.to_string()).await.map_err(|e| e.to_string())?;
                let ft = lm.file_type();
                if ft.is_symlink() {
                    return Ok(match s.metadata(path.to_string()).await {
                        Ok(m) if m.file_type().is_file() => Some((false, m.len())),
                        _ => None,
                    });
                }
                Ok(if ft.is_dir() {
                    Some((true, 0))
                } else if ft.is_file() {
                    Some((false, lm.len()))
                } else {
                    None
                })
            }
        }
    }

    async fn list(&self, path: &str) -> Result<Vec<String>, String> {
        match self {
            Side::Local => {
                let mut rd = tokio::fs::read_dir(path).await.map_err(|e| e.to_string())?;
                let mut out = vec![];
                while let Ok(Some(e)) = rd.next_entry().await {
                    out.push(e.file_name().to_string_lossy().into_owned());
                }
                Ok(out)
            }
            Side::Remote(s) => Ok(s
                .read_dir(path.to_string())
                .await
                .map_err(|e| e.to_string())?
                .map(|e| e.file_name())
                .filter(|n| n != "." && n != "..")
                .collect()),
        }
    }

    async fn mkdir(&self, path: &str) {
        match self {
            Side::Local => {
                let _ = tokio::fs::create_dir_all(path).await;
            }
            Side::Remote(s) => {
                let _ = s.create_dir(path.to_string()).await;
            }
        }
    }

    async fn open(&self, path: &str) -> Result<Reader, String> {
        match self {
            Side::Local => Ok(Box::new(tokio::fs::File::open(path).await.map_err(|e| e.to_string())?)),
            Side::Remote(s) => Ok(Box::new(s.open(path.to_string()).await.map_err(|e| e.to_string())?)),
        }
    }

    async fn create(&self, path: &str) -> Result<Writer, String> {
        match self {
            Side::Local => Ok(Box::new(tokio::fs::File::create(path).await.map_err(|e| e.to_string())?)),
            Side::Remote(s) => {
                let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
                let f = s.open_with_flags(path.to_string(), flags).await.map_err(|e| {
                    if format!("{e:?}").contains("PermissionDenied") {
                        format!("Yetki yok: {path}")
                    } else {
                        e.to_string()
                    }
                })?;
                Ok(Box::new(f))
            }
        }
    }

    async fn remove(&self, path: &str) {
        match self {
            Side::Local => {
                let _ = tokio::fs::remove_file(path).await;
            }
            Side::Remote(s) => {
                let _ = s.remove_file(path.to_string()).await;
            }
        }
    }

    async fn close(&self) {
        if let Side::Remote(s) = self {
            let _ = s.close().await;
        }
    }
}

#[derive(Serialize, Clone)]
struct PlanFile {
    name: String,
    size: u64,
}

struct Item {
    src: String,
    dst: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferArgs {
    pub(crate) id: String,
    /// None → local tab (plain copy).
    conn_id: Option<String>,
    /// "up" (local → remote) or "down" (remote → local)
    direction: String,
    sources: Vec<String>,
    dest_dir: String,
}

async fn open_sftp(state: &SshState, conn_id: &str) -> Result<SftpSession, String> {
    let conn = state.conn(conn_id)?;
    let ch = conn.handle.channel_open_session().await.map_err(|e| e.to_string())?;
    ch.request_subsystem(true, "sftp").await.map_err(|e| e.to_string())?;
    SftpSession::new(ch.into_stream()).await.map_err(|e| format!("SFTP: {e}"))
}

#[tauri::command]
pub async fn transfer_start<R: Runtime>(
    app: AppHandle<R>,
    ssh: State<'_, SshState>,
    st: State<'_, TransferState>,
    args: TransferArgs,
) -> Result<(), String> {
    let (src, dst) = match &args.conn_id {
        None => (Side::Local, Side::Local),
        Some(id) => {
            let s = open_sftp(&ssh, id).await?;
            if args.direction == "down" { (Side::Remote(s), Side::Local) } else { (Side::Local, Side::Remote(s)) }
        }
    };
    let cancel = Arc::new(AtomicBool::new(false));
    st.cancels.lock().unwrap().insert(args.id.clone(), cancel.clone());

    tauri::async_runtime::spawn(async move {
        let id = args.id.clone();
        let res = run(&app, &id, &src, &dst, &args.sources, &args.dest_dir, &cancel).await;
        src.close().await;
        dst.close().await;
        if let Some(st) = app.try_state::<TransferState>() {
            st.cancels.lock().unwrap().remove(&id);
        }
        let cancelled = cancel.load(Ordering::Relaxed);
        let _ = app.emit(
            "transfer-done",
            serde_json::json!({ "id": id, "cancelled": cancelled, "error": res.err().filter(|_| !cancelled) }),
        );
    });
    Ok(())
}

async fn run<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    src: &Side,
    dst: &Side,
    sources: &[String],
    dest_dir: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // 1) Plan: expand directories into a flat file list.
    let mut dirs: Vec<String> = vec![];
    let mut items: Vec<Item> = vec![];
    let mut plan: Vec<PlanFile> = vec![];
    for root in sources {
        let mut stack = vec![(root.clone(), src.basename(root))];
        while let Some((path, rel)) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            match src.stat(&path).await? {
                Some((true, _)) => {
                    dirs.push(dst.join(dest_dir, &rel));
                    let mut names = src.list(&path).await?;
                    names.sort();
                    for n in names.into_iter().rev() {
                        stack.push((src.join(&path, &n), format!("{rel}/{n}")));
                    }
                }
                Some((false, size)) => {
                    items.push(Item { src: path, dst: dst.join(dest_dir, &rel) });
                    plan.push(PlanFile { name: rel, size });
                }
                None => {}
            }
        }
    }
    let total: u64 = plan.iter().map(|p| p.size).sum();
    let _ = app.emit(
        "transfer-plan",
        serde_json::json!({ "id": id, "files": plan, "total": total, "up": dst.is_remote() }),
    );
    for d in &dirs {
        dst.mkdir(d).await;
    }

    // 2) Copy.
    let mut bytes: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    let mut last_emit = Instant::now();
    for (index, it) in items.iter().enumerate() {
        let mut r = src.open(&it.src).await?;
        let mut w = dst.create(&it.dst).await?;
        let mut file_done: u64 = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(w);
                dst.remove(&it.dst).await;
                return Err("cancelled".into());
            }
            let n = r.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
            file_done += n as u64;
            bytes += n as u64;
            if last_emit.elapsed() > Duration::from_millis(120) {
                last_emit = Instant::now();
                let _ = app.emit(
                    "transfer-progress",
                    serde_json::json!({ "id": id, "index": index, "fileDone": file_done, "bytes": bytes }),
                );
            }
        }
        w.shutdown().await.map_err(|e| e.to_string())?;
        let _ = app.emit(
            "transfer-progress",
            serde_json::json!({ "id": id, "index": index + 1, "fileDone": 0, "bytes": bytes }),
        );
    }
    Ok(())
}

#[tauri::command]
pub fn transfer_cancel(st: State<'_, TransferState>, id: String) {
    if let Some(c) = st.cancels.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
    }
}
