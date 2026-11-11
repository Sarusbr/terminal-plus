//! SSH connection + PTY channel management.
//!
//! One `Conn` per tab. Every terminal pane is an independent shell channel on that
//! connection. Terminal output is streamed to the webview through a Tauri IPC
//! `Channel` as raw bytes (no JSON/base64 encoding). Input goes through an ordered
//! per-channel queue so keystrokes are never reordered.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::{
    check_known_hosts_path, known_hosts::learn_known_hosts_path, load_secret_key, HashAlg,
    PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
};
use russh::{ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Runtime, State};
use tokio::sync::{mpsc, oneshot};

use crate::secrets;

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;
/// Runtime-agnostic event emitter (keeps `Client` non-generic).
type Emit = Arc<dyn Fn(&'static str, serde_json::Value) + Send + Sync>;

fn emitter<R: Runtime>(app: &AppHandle<R>) -> Emit {
    let app = app.clone();
    Arc::new(move |ev, v| {
        let _ = app.emit(ev, v);
    })
}

pub(crate) struct Conn {
    pub(crate) handle: client::Handle<Client>,
    pub(crate) server_id: Option<String>,
}

pub(crate) enum PtyCmd {
    Data(Vec<u8>),
    Resize(u32, u32),
    /// Is a foreground program (other than the shell) running? Only answerable locally.
    Busy(std::sync::mpsc::Sender<Option<bool>>),
    Close,
}

#[derive(Default)]
pub struct SshState {
    pub(crate) conns: Mutex<HashMap<String, Arc<Conn>>>,
    /// pty id → (owner conn id, input queue). Local shells use the owner id "local".
    pub(crate) ptys: Mutex<HashMap<String, (String, mpsc::UnboundedSender<PtyCmd>)>>,
    waiters: Waiters,
}

impl SshState {
    pub(crate) fn conn(&self, conn_id: &str) -> Result<Arc<Conn>, String> {
        self.conns
            .lock()
            .unwrap()
            .get(conn_id)
            .cloned()
            .ok_or_else(|| "NOT_CONNECTED".to_string())
    }
}

/// `~/.ssh/known_hosts` (overridable with `TP_KNOWN_HOSTS`, used by tests).
fn known_hosts_file() -> PathBuf {
    if let Some(p) = std::env::var_os("TP_KNOWN_HOSTS") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".ssh").join("known_hosts")
}

#[derive(Default)]
struct HostKeyOutcome {
    /// Set when the user rejected the key or it changed; used to produce a clear error.
    error: Option<String>,
}

pub(crate) struct Client {
    emit: Emit,
    conn_id: String,
    host: String,
    port: u16,
    waiters: Waiters,
    outcome: Arc<Mutex<HostKeyOutcome>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HostKeyPrompt {
    conn_id: String,
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key: PublicKey = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        let kh = known_hosts_file();
        match check_known_hosts_path(&self.host, self.port, &key, &kh) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(russh::keys::Error::KeyChanged { line }) => {
                self.outcome.lock().unwrap().error = Some(format!(
                    "HOSTKEY_CHANGED: {} host key differs from known_hosts line {line}. Possible MITM; connection refused.",
                    self.host
                ));
                return Ok(false);
            }
            Err(e) => log::warn!("known_hosts check failed: {e}"),
        }

        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(self.conn_id.clone(), tx);
        (self.emit)(
            "ssh-hostkey",
            serde_json::to_value(HostKeyPrompt {
                conn_id: self.conn_id.clone(),
                host: self.host.clone(),
                port: self.port,
                algorithm: key.algorithm().to_string(),
                fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            })
            .unwrap_or_default(),
        );
        let accepted = rx.await.unwrap_or(false);
        if accepted {
            if let Err(e) = learn_known_hosts_path(&self.host, self.port, &key, &kh) {
                log::warn!("could not write known_hosts: {e}");
            }
        } else {
            self.outcome.lock().unwrap().error = Some("HOSTKEY_REJECTED".into());
        }
        Ok(accepted)
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        (self.emit)("ssh-disconnected", serde_json::Value::String(self.conn_id.clone()));
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(e) => Err(e),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectArgs {
    conn_id: String,
    host: String,
    port: u16,
    user: String,
    /// "password" | "key"
    auth: String,
    password: Option<String>,
    key_path: Option<String>,
    passphrase: Option<String>,
    server_id: Option<String>,
    #[serde(default)]
    save_secret: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    ip: String,
}

async fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| format!("DNS timeout: {host}"))?
    .map_err(|e| format!("DNS: {e}"))?
    .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| format!("DNS: no address for {host}"))
}

fn default_keys() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|n| home.join(".ssh").join(n))
        .filter(|p| p.exists())
        .collect()
}

async fn auth_password(
    handle: &mut client::Handle<Client>,
    user: &str,
    password: &str,
) -> Result<bool, String> {
    let res = handle
        .authenticate_password(user, password)
        .await
        .map_err(|e| e.to_string())?;
    if res.success() {
        return Ok(true);
    }
    // Many servers only allow keyboard-interactive; answer every prompt with the password.
    let mut resp = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await
        .map_err(|e| e.to_string())?;
    for _ in 0..5 {
        match resp {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(true),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                resp = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(false)
}

async fn auth_key(
    handle: &mut client::Handle<Client>,
    user: &str,
    path: &PathBuf,
    passphrase: Option<&str>,
) -> Result<bool, String> {
    let key = match load_secret_key(path, passphrase) {
        Ok(k) => k,
        Err(russh::keys::Error::KeyIsEncrypted) => return Err("NEED_PASSPHRASE".into()),
        Err(e) if passphrase.is_some() => {
            log::warn!("key decode failed: {e}");
            return Err("NEED_PASSPHRASE: wrong passphrase".into());
        }
        Err(e) => return Err(format!("KEY_ERROR: {}: {e}", path.display())),
    };
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    let res = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
        .await
        .map_err(|e| e.to_string())?;
    Ok(res.success())
}

#[tauri::command]
pub async fn ssh_connect<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SshState>,
    args: ConnectArgs,
) -> Result<ConnectResult, String> {
    let addr = resolve(&args.host, args.port).await?;
    let tcp = tokio::time::timeout(Duration::from_secs(12), tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| format!("TIMEOUT: {addr}"))?
        .map_err(|e| format!("TCP: {e}"))?;
    let _ = tcp.set_nodelay(true);

    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(20)),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    });
    let outcome = Arc::new(Mutex::new(HostKeyOutcome::default()));
    let handler = Client {
        emit: emitter(&app),
        conn_id: args.conn_id.clone(),
        host: args.host.clone(),
        port: args.port,
        waiters: state.waiters.clone(),
        outcome: outcome.clone(),
    };
    let mut handle = match client::connect_stream(config, tcp, handler).await {
        Ok(h) => h,
        Err(e) => {
            state.waiters.lock().unwrap().remove(&args.conn_id);
            if let Some(msg) = outcome.lock().unwrap().error.take() {
                return Err(msg);
            }
            return Err(format!("SSH: {e}"));
        }
    };

    let sid = args.server_id.as_deref();
    let ok = match args.auth.as_str() {
        "password" => {
            let (pw, from_keychain) = match (&args.password, sid) {
                (Some(p), _) => (p.clone(), false),
                (None, Some(id)) => match secrets::get(&secrets::account("pw", id)) {
                    Some(p) => (p, true),
                    None => return Err("NEED_PASSWORD".into()),
                },
                (None, None) => return Err("NEED_PASSWORD".into()),
            };
            let ok = auth_password(&mut handle, &args.user, &pw).await?;
            if !ok {
                return Err(if from_keychain {
                    "NEED_PASSWORD: saved password rejected".into()
                } else {
                    "AUTH_FAILED".into()
                });
            }
            if args.save_secret && !from_keychain {
                if let Some(id) = sid {
                    let _ = secrets::set(&secrets::account("pw", id), &pw);
                }
            }
            true
        }
        _ => {
            let keys: Vec<PathBuf> = match args.key_path.as_deref().filter(|s| !s.is_empty()) {
                Some(p) => vec![PathBuf::from(shellexpand_home(p))],
                None => default_keys(),
            };
            if keys.is_empty() {
                return Err("KEY_ERROR: no key file".into());
            }
            let saved_pp = sid.and_then(|id| secrets::get(&secrets::account("pp", id)));
            let pp = args.passphrase.clone().or(saved_pp);
            let mut ok = false;
            for k in &keys {
                if auth_key(&mut handle, &args.user, k, pp.as_deref()).await? {
                    ok = true;
                    break;
                }
            }
            if ok && args.save_secret && args.passphrase.is_some() {
                if let (Some(id), Some(p)) = (sid, &args.passphrase) {
                    let _ = secrets::set(&secrets::account("pp", id), p);
                }
            }
            ok
        }
    };
    if !ok {
        return Err("AUTH_FAILED".into());
    }

    state.conns.lock().unwrap().insert(
        args.conn_id.clone(),
        Arc::new(Conn { handle, server_id: args.server_id.clone() }),
    );
    Ok(ConnectResult { ip: addr.ip().to_string() })
}

fn shellexpand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

#[tauri::command]
pub fn ssh_hostkey_respond(state: State<'_, SshState>, conn_id: String, accept: bool) {
    if let Some(tx) = state.waiters.lock().unwrap().remove(&conn_id) {
        let _ = tx.send(accept);
    }
}

#[tauri::command]
pub async fn ssh_disconnect(state: State<'_, SshState>, conn_id: String) -> Result<(), String> {
    let conn = state.conns.lock().unwrap().remove(&conn_id);
    {
        let mut ptys = state.ptys.lock().unwrap();
        ptys.retain(|_, (c, tx)| {
            if *c == conn_id {
                let _ = tx.send(PtyCmd::Close);
                false
            } else {
                true
            }
        });
    }
    if let Some(conn) = conn {
        let _ = conn.handle.disconnect(Disconnect::ByApplication, "", "en").await;
    }
    Ok(())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyExit {
    pub(crate) pty_id: String,
    pub(crate) code: Option<u32>,
}

/// Open a new interactive shell channel on an existing connection.
#[tauri::command]
pub async fn pty_open<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SshState>,
    conn_id: String,
    pty_id: String,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let conn = state.conn(&conn_id)?;
    let ch = conn
        .handle
        .channel_open_session()
        .await
        .map_err(|e| e.to_string())?;
    let _ = ch.set_env(false, "COLORTERM", "truecolor").await;
    ch.request_pty(false, "xterm-256color", cols.max(2), rows.max(2), 0, 0, &[])
        .await
        .map_err(|e| e.to_string())?;
    ch.request_shell(false).await.map_err(|e| e.to_string())?;

    let (mut reader, writer) = ch.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<PtyCmd>();
    state.ptys.lock().unwrap().insert(pty_id.clone(), (conn_id, tx));

    // Writer task: ordered input + resize.
    tauri::async_runtime::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            let r = match cmd {
                PtyCmd::Data(d) => writer.data(&d[..]).await,
                PtyCmd::Resize(c, r) => writer.window_change(c, r, 0, 0).await,
                PtyCmd::Busy(tx) => {
                    let _ = tx.send(None); // unknown over SSH
                    Ok(())
                }
                PtyCmd::Close => {
                    let _ = writer.eof().await;
                    let _ = writer.close().await;
                    break;
                }
            };
            if r.is_err() {
                break;
            }
        }
    });

    // Reader task: stream raw bytes to the webview.
    tauri::async_runtime::spawn(async move {
        let mut code = None;
        while let Some(msg) = reader.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    if on_data.send(InvokeResponseBody::Raw(data.to_vec())).is_err() {
                        break;
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        if let Some(st) = tauri::Manager::try_state::<SshState>(&app) {
            st.ptys.lock().unwrap().remove(&pty_id);
        }
        let _ = app.emit("pty-exit", PtyExit { pty_id, code });
    });
    Ok(())
}

fn send(state: &SshState, pty_id: &str, cmd: PtyCmd) -> Result<(), String> {
    let ptys = state.ptys.lock().unwrap();
    let (_, tx) = ptys.get(pty_id).ok_or("PTY_CLOSED")?;
    tx.send(cmd).map_err(|_| "PTY_CLOSED".to_string())
}

// Sync commands run in order on the main thread → keystroke order is preserved.
#[tauri::command]
pub fn pty_write(state: State<'_, SshState>, pty_id: String, data: String) -> Result<(), String> {
    send(&state, &pty_id, PtyCmd::Data(data.into_bytes()))
}

#[tauri::command]
pub fn pty_resize(
    state: State<'_, SshState>,
    pty_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    send(&state, &pty_id, PtyCmd::Resize(cols.max(2), rows.max(2)))
}

#[tauri::command]
pub fn pty_close(state: State<'_, SshState>, pty_id: String) -> Result<(), String> {
    let r = send(&state, &pty_id, PtyCmd::Close);
    state.ptys.lock().unwrap().remove(&pty_id);
    r
}

/// Foreground-process check: Some(true) when a program other than the shell owns the tty.
#[tauri::command]
pub fn pty_busy(state: State<'_, SshState>, pty_id: String) -> Option<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    send(&state, &pty_id, PtyCmd::Busy(tx)).ok()?;
    rx.recv_timeout(Duration::from_millis(500)).ok().flatten()
}

/// Type the saved password for this connection's server into the PTY (for sudo prompts).
/// The secret never crosses into the webview. Returns false when nothing is saved.
#[tauri::command]
pub fn pty_write_saved_password(
    state: State<'_, SshState>,
    conn_id: String,
    pty_id: String,
) -> Result<bool, String> {
    // conn_id "local" → the local machine's sudo password (saved under server id "local").
    let sid = if conn_id == "local" {
        "local".to_string()
    } else {
        match state.conn(&conn_id)?.server_id.clone() {
            Some(s) => s,
            None => return Ok(false),
        }
    };
    let Some(pw) = secrets::get(&secrets::account("pw", &sid)) else { return Ok(false) };
    send(&state, &pty_id, PtyCmd::Data(format!("{pw}\n").into_bytes()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    //! Integration test against a real sshd. Run with:
    //! TP_TEST_SSH=host:port:user:password cargo test --lib -- --ignored
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tauri::{Listener, Manager};

    fn args(conn_id: &str, host: &str, port: u16, user: &str, pw: &str) -> ConnectArgs {
        ConnectArgs {
            conn_id: conn_id.into(), host: host.into(), port, user: user.into(),
            auth: "password".into(), password: Some(pw.into()), key_path: None,
            passphrase: None, server_id: None, save_secret: false,
        }
    }

    async fn read_until(rx: &mut mpsc::UnboundedReceiver<Vec<u8>>, needle: &str) -> String {
        let mut out = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !out.contains(needle) {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(b)) => out.push_str(&String::from_utf8_lossy(&b)),
                _ => panic!("timeout waiting for {needle:?}; got {out:?}"),
            }
        }
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn connect_pty_roundtrip() {
        let spec = std::env::var("TP_TEST_SSH").expect("TP_TEST_SSH=host:port:user:pw");
        let parts: Vec<&str> = spec.split(':').collect();
        let (host, port, user, pw) = (parts[0], parts[1].parse().unwrap(), parts[2], parts[3]);
        let kh = std::env::temp_dir().join(format!("tp_kh_{}", uuid::Uuid::new_v4()));
        std::env::set_var("TP_KNOWN_HOSTS", &kh);

        let app = tauri::test::mock_app();
        app.manage(SshState::default());
        app.manage(crate::transfer::TransferState::default());
        let h = app.handle().clone();
        let prompts = Arc::new(AtomicUsize::new(0));
        {
            let (h2, prompts) = (h.clone(), prompts.clone());
            h.listen("ssh-hostkey", move |ev| {
                prompts.fetch_add(1, Ordering::SeqCst);
                let v: serde_json::Value = serde_json::from_str(ev.payload()).unwrap();
                assert!(v["fingerprint"].as_str().unwrap().starts_with("SHA256:"));
                ssh_hostkey_respond(h2.state(), v["connId"].as_str().unwrap().into(), true);
            });
        }

        // Wrong password is rejected.
        let e = ssh_connect(h.clone(), h.state(), args("bad", host, port, user, "nope")).await;
        assert_eq!(e.err().as_deref(), Some("AUTH_FAILED"));
        assert_eq!(prompts.load(Ordering::SeqCst), 1, "first connect asks for host key");

        // Known host now: no second prompt.
        let r = ssh_connect(h.clone(), h.state(), args("c1", host, port, user, pw)).await.unwrap();
        assert_eq!(r.ip, "127.0.0.1");
        assert_eq!(prompts.load(Ordering::SeqCst), 1);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let ch = Channel::new(move |body| {
            if let InvokeResponseBody::Raw(b) = body {
                let _ = tx.send(b);
            }
            Ok(())
        });
        pty_open(h.clone(), h.state(), "c1".into(), "p1".into(), 80, 24, ch).await.unwrap();
        pty_write(h.state(), "p1".into(), "echo TP_$((40+2))\n".into()).unwrap();
        read_until(&mut rx, "TP_42").await;

        pty_resize(h.state(), "p1".into(), 100, 30).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        pty_write(h.state(), "p1".into(), "stty size; echo $TERM\n".into()).unwrap();
        let out = read_until(&mut rx, "xterm-256color").await;
        assert!(out.contains("30 100"), "resize not applied: {out:?}");

        // Second shell channel on the same connection (split pane).
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        let ch2 = Channel::new(move |body| {
            if let InvokeResponseBody::Raw(b) = body {
                let _ = tx2.send(b);
            }
            Ok(())
        });
        pty_open(h.clone(), h.state(), "c1".into(), "p2".into(), 80, 24, ch2).await.unwrap();
        pty_write(h.state(), "p2".into(), "echo SECOND_$((1+1))\n".into()).unwrap();
        read_until(&mut rx2, "SECOND_2").await;

        // SFTP write/read; shorter rewrite must truncate.
        let tgt = || serde_json::from_value::<crate::fsops::Target>(serde_json::json!({"connId": "c1"})).unwrap();
        let path = "/tmp/tp_sftp_test.txt".to_string();
        crate::fsops::file_write(h.state(), tgt(), path.clone(), "hello world\nline2\n".into()).await.unwrap();
        crate::fsops::file_write(h.state(), tgt(), path.clone(), "short\n".into()).await.unwrap();
        let r = crate::fsops::file_read(h.state(), tgt(), path.clone()).await.unwrap();
        assert_eq!(r.content, "short\n");
        let missing = crate::fsops::file_read(h.state(), tgt(), "/tmp/does-not-exist-tp".into()).await.unwrap();
        assert!(!missing.exists);
        let denied = crate::fsops::file_write(h.state(), tgt(), "/etc/tp_denied".into(), "x".into()).await;
        assert_eq!(denied.err().as_deref(), Some("PERMISSION_DENIED"));

        // Transfer round trip: nested dir up, then back down; contents must match.
        let base = std::env::temp_dir().join(format!("tp_tr_{}", uuid::Uuid::new_v4().simple()));
        let srcdir = base.join("proj");
        std::fs::create_dir_all(srcdir.join("sub/deep")).unwrap();
        std::fs::write(srcdir.join("a.txt"), "alpha").unwrap();
        let big: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(srcdir.join("sub/deep/big.bin"), &big).unwrap();
        let (dtx, mut drx) = mpsc::unbounded_channel::<serde_json::Value>();
        h.listen("transfer-done", move |ev| {
            let _ = dtx.send(serde_json::from_str(ev.payload()).unwrap());
        });
        let rdir = format!("/tmp/tp_up_{}", uuid::Uuid::new_v4().simple());
        pty_write(h.state(), "p1".into(), format!("mkdir -p {rdir}\n")).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let targs = |id: &str, dir: &str, src: Vec<String>, dest: &str| {
            serde_json::from_value::<crate::transfer::TransferArgs>(serde_json::json!({
                "id": id, "connId": "c1", "direction": dir, "sources": src, "destDir": dest
            })).unwrap()
        };
        crate::transfer::transfer_start(h.clone(), h.state(), h.state(),
            targs("t1", "up", vec![srcdir.to_string_lossy().into()], &rdir)).await.unwrap();
        let done = tokio::time::timeout(Duration::from_secs(20), drx.recv()).await.unwrap().unwrap();
        assert!(done["error"].is_null(), "upload failed: {done}");
        let back = base.join("back");
        std::fs::create_dir_all(&back).unwrap();
        crate::transfer::transfer_start(h.clone(), h.state(), h.state(),
            targs("t2", "down", vec![format!("{rdir}/proj")], &back.to_string_lossy())).await.unwrap();
        let done = tokio::time::timeout(Duration::from_secs(20), drx.recv()).await.unwrap().unwrap();
        assert!(done["error"].is_null(), "download failed: {done}");
        assert_eq!(std::fs::read(back.join("proj/a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(back.join("proj/sub/deep/big.bin")).unwrap(), big);
        let _ = std::fs::remove_dir_all(base);

        ssh_disconnect(h.state(), "c1".into()).await.unwrap();
        assert!(pty_write(h.state(), "p1".into(), "x".into()).is_err());
        let _ = std::fs::remove_file(kh);
    }
}
