//! File access for the Files page and the built-in editor.
//! Local tabs use std::fs. SSH tabs open a short-lived SFTP channel on the existing
//! connection only for the duration of a read/write (no extra SSH connection).

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::ssh::SshState;

const MAX_EDIT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// None → local machine.
    conn_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    /// "d" dir, "f" file, "l" symlink, "o" other
    kind: &'static str,
    size: u64,
    mode: u32,
    mtime: i64,
    link: Option<String>,
    /// For symlinks: does it point to a directory?
    link_dir: bool,
}

async fn sftp(state: &SshState, conn_id: &str) -> Result<russh_sftp::client::SftpSession, String> {
    let conn = state.conn(conn_id)?;
    let ch = conn.handle.channel_open_session().await.map_err(|e| e.to_string())?;
    ch.request_subsystem(true, "sftp").await.map_err(|e| e.to_string())?;
    russh_sftp::client::SftpSession::new(ch.into_stream())
        .await
        .map_err(|e| format!("SFTP: {e}"))
}

fn map_io(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "PERMISSION_DENIED".into()
    } else {
        e.to_string()
    }
}

fn map_sftp(e: russh_sftp::client::error::Error) -> String {
    let s = format!("{e:?}");
    if s.contains("PermissionDenied") {
        "PERMISSION_DENIED".into()
    } else if s.contains("NoSuchFile") {
        "NOT_FOUND".into()
    } else {
        e.to_string()
    }
}

fn decode(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > MAX_EDIT_BYTES {
        return Err("TOO_LARGE".into());
    }
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("BINARY".into());
    }
    String::from_utf8(bytes).map_err(|_| "BINARY".into())
}

/// Read a text file. Missing file → "" with `exists=false` (nano semantics: new file).
#[derive(Serialize)]
pub struct ReadResult {
    pub(crate) content: String,
    pub(crate) exists: bool,
}

#[tauri::command]
pub async fn file_read(state: State<'_, SshState>, target: Target, path: String) -> Result<ReadResult, String> {
    let bytes = match &target.conn_id {
        None => match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ReadResult { content: String::new(), exists: false })
            }
            Err(e) => return Err(map_io(e)),
        },
        Some(id) => {
            let s = sftp(&state, id).await?;
            let r = s.read(path.clone()).await;
            let _ = s.close().await;
            match r.map_err(map_sftp) {
                Ok(b) => b,
                Err(e) if e == "NOT_FOUND" => return Ok(ReadResult { content: String::new(), exists: false }),
                Err(e) => return Err(e),
            }
        }
    };
    Ok(ReadResult { content: decode(bytes)?, exists: true })
}

/// Write a file. Returns "PERMISSION_DENIED" so the UI can retry through sudo.
#[tauri::command]
pub async fn file_write(state: State<'_, SshState>, target: Target, path: String, content: String) -> Result<(), String> {
    match &target.conn_id {
        None => tokio::fs::write(&path, content.as_bytes()).await.map_err(map_io),
        Some(id) => {
            let s = sftp(&state, id).await?;
            // Truncate in place (keeps owner/mode of existing files), create if missing.
            use russh_sftp::protocol::OpenFlags;
            use tokio::io::AsyncWriteExt;
            let flags = OpenFlags::WRITE | OpenFlags::TRUNCATE | OpenFlags::CREATE;
            let r = match s.open_with_flags(path.clone(), flags).await {
                Ok(mut f) => {
                    let w = f.write_all(content.as_bytes()).await.map_err(|e| e.to_string());
                    let _ = f.shutdown().await;
                    w
                }
                Err(e) => Err(map_sftp(e)),
            };
            let _ = s.close().await;
            r
        }
    }
}

/// Write content to a fresh temp file (for `sudo cp tmp target`). Returns its path.
#[tauri::command]
pub async fn temp_write(state: State<'_, SshState>, target: Target, content: String) -> Result<String, String> {
    let name = format!(".tp_save_{}", uuid::Uuid::new_v4().simple());
    match &target.conn_id {
        None => {
            let p = std::env::temp_dir().join(name);
            tokio::fs::write(&p, content.as_bytes()).await.map_err(map_io)?;
            Ok(p.to_string_lossy().into_owned())
        }
        Some(id) => {
            let p = format!("/tmp/{name}");
            let s = sftp(&state, id).await?;
            use tokio::io::AsyncWriteExt;
            let r = match s.create(p.clone()).await {
                Ok(mut f) => {
                    let w = f.write_all(content.as_bytes()).await.map_err(|e| e.to_string());
                    let _ = f.shutdown().await;
                    w
                }
                Err(e) => Err(map_sftp(e)),
            };
            let _ = s.close().await;
            r.map(|_| p)
        }
    }
}

/// List a local directory (local tabs). Remote listing goes through the shell runner.
#[tauri::command]
pub fn local_list_dir(path: String) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(&path).map_err(map_io)?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(lmeta) = std::fs::symlink_metadata(e.path()) else { continue };
        let ft = lmeta.file_type();
        let (kind, link, link_dir) = if ft.is_symlink() {
            let target = std::fs::read_link(e.path()).ok().map(|p| p.to_string_lossy().into_owned());
            ("l", target, std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false))
        } else if ft.is_dir() {
            ("d", None, false)
        } else if ft.is_file() {
            ("f", None, false)
        } else {
            ("o", None, false)
        };
        #[cfg(unix)]
        let mode = { use std::os::unix::fs::PermissionsExt; lmeta.permissions().mode() & 0o7777 };
        #[cfg(not(unix))]
        let mode = if lmeta.permissions().readonly() { 0o444 } else { 0o644 };
        let mtime = lmeta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry { name, kind, size: lmeta.len(), mode, mtime, link, link_dir });
    }
    Ok(out)
}

fn copy_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_recursive(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn io_msg(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "Yetki yok".into(),
        std::io::ErrorKind::AlreadyExists => "Zaten var".into(),
        std::io::ErrorKind::NotFound => "Bulunamadı".into(),
        _ => e.to_string(),
    }
}

/// Local file operations for the Files page where shell commands aren't used (Windows).
/// op: mkdir | touch | rename (paths[0] → dest) | remove | copy / move (paths → dest dir)
#[tauri::command]
pub fn fs_op(op: String, paths: Vec<String>, dest: Option<String>) -> Result<(), String> {
    use std::path::Path;
    let first = paths.first().ok_or("no path")?;
    match op.as_str() {
        "mkdir" => std::fs::create_dir_all(first).map_err(io_msg),
        "touch" => std::fs::OpenOptions::new().create(true).append(true).open(first).map(|_| ()).map_err(io_msg),
        "rename" => {
            let to = dest.ok_or("no dest")?;
            if Path::new(&to).exists() {
                return Err("Zaten var".into());
            }
            std::fs::rename(first, to).map_err(io_msg)
        }
        "remove" => {
            for p in &paths {
                let m = std::fs::symlink_metadata(p).map_err(io_msg)?;
                if m.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }.map_err(io_msg)?;
            }
            Ok(())
        }
        "copy" | "move" => {
            let dir = std::path::PathBuf::from(dest.ok_or("no dest")?);
            for p in &paths {
                let src = Path::new(p);
                let name = src.file_name().ok_or("bad path")?;
                let to = dir.join(name);
                if to == src {
                    continue;
                }
                if op == "move" && std::fs::rename(src, &to).is_ok() {
                    continue;
                }
                copy_recursive(src, &to).map_err(io_msg)?;
                if op == "move" {
                    let m = std::fs::symlink_metadata(src).map_err(io_msg)?;
                    if m.is_dir() { std::fs::remove_dir_all(src) } else { std::fs::remove_file(src) }.map_err(io_msg)?;
                }
            }
            Ok(())
        }
        _ => Err(format!("unknown op {op}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_ops_roundtrip() {
        let base = std::env::temp_dir().join(format!("tp_fsop_{}", uuid::Uuid::new_v4().simple()));
        let b = |p: &str| base.join(p).to_string_lossy().into_owned();
        fs_op("mkdir".into(), vec![b("a/sub")], None).unwrap();
        fs_op("touch".into(), vec![b("a/sub/f.txt")], None).unwrap();
        std::fs::write(b("a/sub/f.txt"), "x").unwrap();
        fs_op("mkdir".into(), vec![b("dst")], None).unwrap();
        fs_op("copy".into(), vec![b("a")], Some(b("dst"))).unwrap();
        assert_eq!(std::fs::read_to_string(b("dst/a/sub/f.txt")).unwrap(), "x");
        fs_op("rename".into(), vec![b("dst/a")], Some(b("dst/b"))).unwrap();
        assert!(fs_op("rename".into(), vec![b("a")], Some(b("dst"))).is_err());
        fs_op("move".into(), vec![b("dst/b")], Some(b(""))).unwrap();
        assert!(base.join("b/sub/f.txt").is_file());
        fs_op("remove".into(), vec![b("a"), b("b")], None).unwrap();
        assert!(!base.join("a").exists() && !base.join("b").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
