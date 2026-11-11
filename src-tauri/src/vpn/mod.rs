//! VPN module. Providers (OpenConnect today; WireGuard/OpenVPN plug into the same shape)
//! implement: how to launch (`launch`) and how to read their output (`Parser`). This file
//! owns the generic parts: process lifecycle, prompts, status events, logs, stats.
//!
//! Modes:
//! - "app":    only Terminal Plus traffic. The provider exposes a local SOCKS5 proxy
//!             (no admin rights); SSH/SFTP for linked servers dial through it.
//! - "global": system-wide tun + routes; needs admin rights (asked on every connect).

pub mod components;
mod openconnect;

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::secrets;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConnectReq {
    pub id: String,
    pub name: String,
    /// "openconnect" (others later)
    pub protocol: String,
    /// openconnect --protocol: gp | anyconnect | fortinet | pulse | f5 | array
    pub oc_protocol: Option<String>,
    pub server: String,
    #[serde(default)]
    pub user: String,
    pub group: Option<String>,
    pub cert_path: Option<String>,
    /// Pinned server certificate ("pin-sha256:…"), accepted by the user earlier.
    pub servercert: Option<String>,
    /// "app" | "global"
    pub mode: String,
    pub password: Option<String>,
    #[serde(default)]
    pub save_password: bool,
}

/// What a provider parser asks the runner to do.
pub enum Action {
    /// Write a line to the VPN process (answer to a prompt).
    Send(String),
    /// Ask the user (MFA code, password, gateway choice …).
    Ask { kind: &'static str, message: String, options: Vec<String> },
    Connected { ip: String },
    Reconnecting,
    Stats { rx: u64, tx: u64 },
    Fail { kind: &'static str, message: String, pin: Option<String> },
}

pub trait Parser: Send {
    /// Feed raw output. `tail` is the current unterminated line (prompts have no newline).
    fn line(&mut self, line: &str) -> Vec<Action>;
    fn prompt(&mut self, tail: &str) -> Vec<Action>;
}

pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub name: String,
    /// connecting | connected | reconnecting | disconnected | error
    pub state: String,
    pub mode: String,
    pub ip: Option<String>,
    pub socks_port: Option<u16>,
    pub since: Option<u64>,
    pub rx: u64,
    pub tx: u64,
    pub error: Option<String>,
    /// auth | cert | hip | denied | component | network | other
    pub error_kind: Option<String>,
    pub pin: Option<String>,
}

enum Ctl {
    /// App mode: our child process.
    Child(u32),
    /// Global mode (macOS/Linux): root process watched through a control file.
    Root { ctrl: PathBuf },
}

struct Conn {
    status: Mutex<Status>,
    input: Mutex<Option<Box<dyn Write + Send>>>,
    log: Mutex<VecDeque<String>>,
    ctl: Mutex<Option<Ctl>>,
    stopping: std::sync::atomic::AtomicBool,
    /// `openconnect --authenticate` results (COOKIE, CONNECT_URL, FINGERPRINT, RESOLVE).
    /// Secret — never logged, never written to disk.
    auth: Mutex<HashMap<String, String>>,
}

#[derive(Default)]
pub struct VpnState {
    conns: Mutex<HashMap<String, Arc<Conn>>>,
    /// Log of the last finished connection per profile (so a failure can still be read).
    last_logs: Mutex<HashMap<String, Vec<String>>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn emit_status<R: Runtime>(app: &AppHandle<R>, c: &Conn) {
    let s = c.status.lock().unwrap().clone();
    let _ = app.emit("vpn-status", s);
}

fn push_log(c: &Conn, line: &str) {
    let mut l = c.log.lock().unwrap();
    l.push_back(line.to_string());
    while l.len() > 400 {
        l.pop_front();
    }
}

/// Wait until something listens on 127.0.0.1:`port`.
fn wait_port(port: u16, max: Duration) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let end = std::time::Instant::now() + max;
    while std::time::Instant::now() < end {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

/// Apply parser actions to a connection.
fn apply<R: Runtime>(app: &AppHandle<R>, c: &Arc<Conn>, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Send(text) => {
                if let Some(w) = c.input.lock().unwrap().as_mut() {
                    let _ = w.write_all(format!("{text}\n").as_bytes());
                    let _ = w.flush();
                }
            }
            Action::Ask { kind, message, options } => {
                let id = c.status.lock().unwrap().id.clone();
                let _ = app.emit("vpn-prompt", serde_json::json!({ "id": id, "kind": kind, "message": message, "options": options }));
            }
            Action::Connected { ip } => {
                // App mode: openconnect reports "Configured" before ocproxy listens. Only call
                // it connected once the local SOCKS port accepts connections, so whatever
                // waits on us (SSH to a linked server) doesn't hit "connection refused".
                let (port, first) = {
                    let s = c.status.lock().unwrap();
                    (s.socks_port, s.state != "connected" && s.state != "reconnecting")
                };
                if let (Some(port), true) = (port, first) {
                    let (a2, c2) = (app.clone(), c.clone());
                    std::thread::spawn(move || {
                        let ready = wait_port(port, Duration::from_secs(15));
                        if c2.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        if !ready {
                            apply(&a2, &c2, vec![Action::Fail { kind: "other", message: "Yerel VPN proxy'si (ocproxy) başlamadı".into(), pin: None }]);
                            return;
                        }
                        {
                            let mut s = c2.status.lock().unwrap();
                            s.state = "connected".into();
                            s.ip = Some(ip);
                            s.since = Some(now_ms());
                        }
                        emit_status(&a2, &c2);
                    });
                    continue;
                }
                {
                    let mut s = c.status.lock().unwrap();
                    let first = s.state != "connected" && s.state != "reconnecting";
                    s.state = "connected".into();
                    s.ip = Some(ip);
                    if first {
                        s.since = Some(now_ms());
                    }
                }
                emit_status(app, c);
            }
            Action::Reconnecting => {
                c.status.lock().unwrap().state = "reconnecting".into();
                emit_status(app, c);
            }
            Action::Stats { rx, tx } => {
                let mut s = c.status.lock().unwrap();
                s.rx = rx;
                s.tx = tx;
            }
            Action::Fail { kind, message, pin } => {
                {
                    let mut s = c.status.lock().unwrap();
                    s.state = "error".into();
                    s.error = Some(message);
                    s.error_kind = Some(kind.into());
                    s.pin = pin;
                }
                emit_status(app, c);
                stop(c);
            }
        }
    }
}

/// Stream output from `r` into the parser (line by line; prompts via the unterminated tail).
fn pump<R: Runtime>(app: AppHandle<R>, c: Arc<Conn>, parser: Arc<Mutex<Box<dyn Parser>>>, mut r: impl Read) {
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.push_str(&String::from_utf8_lossy(&buf[..n]));
        while let Some(i) = pending.find('\n') {
            let line: String = pending.drain(..=i).collect();
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = auth_var(&line) {
                c.auth.lock().unwrap().insert(k, v);
                continue; // contains the session cookie: never log
            }
            if !line.starts_with("RX: ") {
                push_log(&c, &line);
            }
            let acts = parser.lock().unwrap().line(&line);
            apply(&app, &c, acts);
        }
        if !pending.trim().is_empty() {
            let acts = parser.lock().unwrap().prompt(&pending);
            if !acts.is_empty() {
                push_log(&c, pending.trim());
                pending.clear();
                apply(&app, &c, acts);
            }
        }
    }
}

/// `COOKIE='…'` style lines printed by `openconnect --authenticate` (shell-quoted).
fn auth_var(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=')?;
    if !matches!(k, "COOKIE" | "HOST" | "CONNECT_URL" | "FINGERPRINT" | "RESOLVE") {
        return None;
    }
    let v = v.trim();
    let v = if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') { v[1..v.len() - 1].replace("'\\''", "'") } else { v.to_string() };
    Some((k.to_string(), v))
}

fn stop(c: &Conn) {
    c.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
    match c.ctl.lock().unwrap().as_ref() {
        #[cfg(unix)]
        Some(Ctl::Child(pid)) => unsafe {
            libc::kill(*pid as i32, libc::SIGINT); // clean logout
        },
        #[cfg(not(unix))]
        Some(Ctl::Child(pid)) => {
            let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T"]).status();
        }
        Some(Ctl::Root { ctrl, .. }) => {
            let _ = std::fs::remove_file(ctrl); // root watcher sees it and stops openconnect
        }
        None => {}
    }
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // As a normal user, signalling a root process fails with EPERM — it still exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Mark finished (unless an error was already recorded) and forget the connection.
fn finish<R: Runtime>(app: &AppHandle<R>, id: &str, c: &Arc<Conn>) {
    {
        let mut s = c.status.lock().unwrap();
        if s.state != "error" {
            let unexpected = !c.stopping.load(std::sync::atomic::Ordering::Relaxed);
            if unexpected && (s.state == "connected" || s.state == "reconnecting") {
                s.state = "error".into();
                s.error = Some("Bağlantı koptu".into());
                s.error_kind = Some("network".into());
            } else if unexpected && s.state == "connecting" {
                s.state = "error".into();
                s.error = Some(c.log.lock().unwrap().iter().rev().take(2).cloned().collect::<Vec<_>>().join(" · "));
                s.error_kind = Some("other".into());
            } else {
                s.state = "disconnected".into();
            }
        }
    }
    emit_status(app, c);
    if let Some(st) = app.try_state::<VpnState>() {
        // A retry may already have replaced this entry with a newer connection.
        st.last_logs.lock().unwrap().insert(id.to_string(), c.log.lock().unwrap().iter().cloned().collect());
        let mut conns = st.conns.lock().unwrap();
        if conns.get(id).map(|x| Arc::ptr_eq(x, c)).unwrap_or(false) {
            conns.remove(id);
        }
    }
}

#[tauri::command]
pub async fn vpn_connect<R: Runtime>(app: AppHandle<R>, state: State<'_, VpnState>, req: ConnectReq) -> Result<Status, String> {
    if let Some(c) = state.conns.lock().unwrap().get(&req.id) {
        let s = c.status.lock().unwrap().clone();
        if s.state != "error" && s.state != "disconnected" {
            return Ok(s);
        }
    }
    if req.protocol != "openconnect" {
        return Err("Bu protokol henüz desteklenmiyor".into());
    }
    let mut req = req;
    let saved = secrets::get(&secrets::account("vpw", &req.id));
    if req.password.is_none() {
        req.password = saved.clone();
    } else if req.save_password {
        let _ = secrets::set(&secrets::account("vpw", &req.id), req.password.as_deref().unwrap_or(""));
    }

    let socks_port = if req.mode == "app" { Some(free_port()?) } else { None };
    let launch = openconnect::launch(&app, &req, socks_port)?;
    let parser: Arc<Mutex<Box<dyn Parser>>> = Arc::new(Mutex::new(Box::new(openconnect::OcParser::new(&req))));

    let conn = Arc::new(Conn {
        status: Mutex::new(Status {
            id: req.id.clone(),
            name: req.name.clone(),
            state: "connecting".into(),
            mode: req.mode.clone(),
            socks_port,
            ..Default::default()
        }),
        input: Mutex::new(None),
        log: Mutex::new(VecDeque::new()),
        ctl: Mutex::new(None),
        stopping: Default::default(),
        auth: Default::default(),
    });
    state.conns.lock().unwrap().insert(req.id.clone(), conn.clone());
    emit_status(&app, &conn);

    if req.mode == "app" {
        start_child(&app, &req.id, conn.clone(), parser, launch)?;
    } else {
        #[cfg(unix)]
        start_root(&app, &req.id, conn.clone(), parser, launch)?;
        #[cfg(windows)]
        start_elevated_windows(&app, &req.id, conn.clone(), parser, launch)?;
    }
    let s = conn.status.lock().unwrap().clone();
    Ok(s)
}

fn start_child<R: Runtime>(app: &AppHandle<R>, id: &str, conn: Arc<Conn>, parser: Arc<Mutex<Box<dyn Parser>>>, l: Launch) -> Result<(), String> {
    let mut cmd = std::process::Command::new(&l.program);
    cmd.args(&l.args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", l.program.display()))?;
    *conn.input.lock().unwrap() = child.stdin.take().map(|w| Box::new(w) as Box<dyn Write + Send>);
    *conn.ctl.lock().unwrap() = Some(Ctl::Child(child.id()));
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    {
        let (a, c, p) = (app.clone(), conn.clone(), parser.clone());
        std::thread::spawn(move || pump(a, c, p, out));
    }
    {
        let (a, c, p) = (app.clone(), conn.clone(), parser.clone());
        std::thread::spawn(move || pump(a, c, p, err));
    }
    // Stats: openconnect prints "RX: …; TX: …" on SIGUSR1.
    #[cfg(unix)]
    {
        let (c, pid) = (conn.clone(), child.id() as i32);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3));
            if c.stopping.load(std::sync::atomic::Ordering::Relaxed) || !pid_alive(pid) {
                break;
            }
            if c.status.lock().unwrap().state == "connected" {
                unsafe { libc::kill(pid, libc::SIGUSR1) };
            }
        });
    }
    let (a, c, id) = (app.clone(), conn, id.to_string());
    std::thread::spawn(move || {
        let _ = child.wait();
        finish(&a, &id, &c);
    });
    Ok(())
}

/// Phase 1 of a privileged connection: `openconnect --authenticate` as the normal user,
/// with ordinary pipes, so every prompt (password, MFA, gateway, certificate) works the
/// same as in app mode. Returns COOKIE/CONNECT_URL/FINGERPRINT/RESOLVE, or None (the
/// parser has already reported why).
#[cfg_attr(not(windows), allow(dead_code))]
fn authenticate(conn: &Arc<Conn>, app: &AppHandle<impl Runtime>, parser: &Arc<Mutex<Box<dyn Parser>>>, l: &Launch) -> Option<HashMap<String, String>> {
    let mut args: Vec<String> = l.args.iter().filter(|a| !a.starts_with("--script")).cloned().collect();
    args.insert(0, "--authenticate".into());
    let mut cmd = std::process::Command::new(&l.program);
    cmd.args(&args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            apply(app, conn, vec![Action::Fail { kind: "component", message: format!("{}: {e}", l.program.display()), pin: None }]);
            return None;
        }
    };
    *conn.input.lock().unwrap() = child.stdin.take().map(|w| Box::new(w) as Box<dyn Write + Send>);
    *conn.ctl.lock().unwrap() = Some(Ctl::Child(child.id()));
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t1 = { let (a, c, p) = (app.clone(), conn.clone(), parser.clone()); std::thread::spawn(move || pump(a, c, p, out)) };
    let t2 = { let (a, c, p) = (app.clone(), conn.clone(), parser.clone()); std::thread::spawn(move || pump(a, c, p, err)) };
    let status = child.wait();
    let _ = (t1.join(), t2.join());
    *conn.input.lock().unwrap() = None;
    let vars = std::mem::take(&mut *conn.auth.lock().unwrap());
    let ok = status.map(|s| s.success()).unwrap_or(false) && vars.contains_key("COOKIE");
    if ok { Some(vars) } else { None }
}

/// Global mode on macOS/Linux: run openconnect as root via the OS password prompt.
/// Secrets travel through a FIFO (never written to disk); stopping = deleting a control
/// file that a root-side watcher polls, so disconnecting needs no second password.
#[cfg(unix)]
fn start_root<R: Runtime>(app: &AppHandle<R>, id: &str, conn: Arc<Conn>, parser: Arc<Mutex<Box<dyn Parser>>>, l: Launch) -> Result<(), String> {
    use std::ffi::CString;
    let dir = std::env::temp_dir().join(format!("tp-vpn-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (fifo, log, pid_file, ctrl, script) =
        (dir.join("in"), dir.join("log"), dir.join("pid"), dir.join("run"), dir.join("start.sh"));
    let cpath = CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
    if unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) } != 0 {
        return Err("mkfifo failed".into());
    }
    std::fs::write(&ctrl, b"").map_err(|e| e.to_string())?;
    std::fs::write(&log, b"").map_err(|e| e.to_string())?;

    // Pick a tun unit so stats can be read from the interface counters.
    let iface = if cfg!(target_os = "macos") { Some(free_utun()) } else { Some(format!("tpvpn{}", &uuid::Uuid::new_v4().simple().to_string()[..4])) };
    #[cfg(test)]
    let iface = std::env::var("TP_TEST_IFACE").ok().or(iface);
    let sh = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut args: Vec<String> = l.args.iter().map(|a| sh(a)).collect();
    if let Some(i) = &iface {
        args.insert(0, format!("--interface={}", sh(i)));
    }
    let body = format!(
        "#!/bin/sh\nLC_ALL=C; export LC_ALL\n{prog} {args} --pid-file={pid} < {fifo} >> {log} 2>&1 &\nOC=$!\n\
         ( while [ -e {ctrl} ] && kill -0 $OC 2>/dev/null; do sleep 1; done; kill -INT $OC 2>/dev/null; sleep 4; kill -9 $OC 2>/dev/null ) >/dev/null 2>&1 &\nexit 0\n",
        prog = sh(&l.program.to_string_lossy()),
        args = args.join(" "),
        pid = sh(&pid_file.to_string_lossy()),
        fifo = sh(&fifo.to_string_lossy()),
        log = sh(&log.to_string_lossy()),
        ctrl = sh(&ctrl.to_string_lossy()),
    );
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    let (name, mode) = { let s = conn.status.lock().unwrap(); (s.name.clone(), s.mode.clone()) };
    let _ = std::fs::write(dir.join("meta.json"), serde_json::json!({ "id": id, "name": name, "mode": mode, "iface": iface }).to_string());
    *conn.ctl.lock().unwrap() = Some(Ctl::Root { ctrl: ctrl.clone() });

    // FIFO writer: opening blocks until openconnect (root) opens it for reading.
    {
        let (c, f) = (conn.clone(), fifo.clone());
        std::thread::spawn(move || {
            if let Ok(w) = std::fs::OpenOptions::new().write(true).open(&f) {
                *c.input.lock().unwrap() = Some(Box::new(w));
            }
        });
    }

    let (a, c, id, p) = (app.clone(), conn.clone(), id.to_string(), parser);
    std::thread::spawn(move || {
        let ok = elevate(&script);
        if let Err(e) = ok {
            {
                let mut s = c.status.lock().unwrap();
                s.state = "error".into();
                s.error = Some(e);
                s.error_kind = Some("denied".into());
            }
            // Unblock the FIFO writer thread.
            let _ = std::fs::OpenOptions::new().read(true).custom_flags_nonblock().open(&fifo);
            let _ = std::fs::remove_dir_all(&dir);
            finish(&a, &id, &c);
            return;
        }
        root_watch(&a, &id, &c, &p, &RootFiles { dir, log, pid_file, ctrl, fifo }, iface.as_deref(), 0);
    });
    Ok(())
}

#[cfg(unix)]
struct RootFiles {
    dir: PathBuf,
    log: PathBuf,
    pid_file: PathBuf,
    ctrl: PathBuf,
    fifo: PathBuf,
}

/// Follow a root-run openconnect: tail its log through the parser, read interface
/// counters, and finish when the process is gone. `start_at` skips already-seen log bytes
/// (used when adopting a connection that outlived a previous app session).
#[cfg(unix)]
fn root_watch<R: Runtime>(a: &AppHandle<R>, id: &str, c: &Arc<Conn>, p: &Arc<Mutex<Box<dyn Parser>>>, f: &RootFiles, iface: Option<&str>, start_at: u64) {
    let mut pos = start_at;
    let mut seen_pid = None;
    let mut pending = String::new();
    let mut ticks = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        ticks += 1;
        if let Ok(mut fh) = std::fs::File::open(&f.log) {
            use std::io::Seek;
            let _ = fh.seek(std::io::SeekFrom::Start(pos));
            let mut chunk = String::new();
            if let Ok(n) = fh.read_to_string(&mut chunk) {
                pos += n as u64;
                pending.push_str(&chunk);
            }
        }
        while let Some(i) = pending.find('\n') {
            let line: String = pending.drain(..=i).collect();
            let line = line.trim_end().to_string();
            if line.is_empty() {
                continue;
            }
            push_log(c, &line);
            let acts = p.lock().unwrap().line(&line);
            apply(a, c, acts);
        }
        if !pending.trim().is_empty() {
            let acts = p.lock().unwrap().prompt(&pending);
            if !acts.is_empty() {
                pending.clear();
                apply(a, c, acts);
            }
        }
        if seen_pid.is_none() {
            seen_pid = std::fs::read_to_string(&f.pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok());
        }
        // Stats from interface counters every ~2 s. (Read state into a local first:
        // holding the status lock across the update would deadlock this thread.)
        if ticks % 8 == 0 {
            let connected = c.status.lock().unwrap().state == "connected";
            if let (Some(i), true) = (iface, connected) {
                if let Some((rx, tx)) = iface_bytes(i) {
                    let mut s = c.status.lock().unwrap();
                    s.rx = rx;
                    s.tx = tx;
                }
            }
        }
        let stopping = c.stopping.load(std::sync::atomic::Ordering::Relaxed);
        let alive = match seen_pid {
            Some(pid) => pid_alive(pid),
            // openconnect writes the pid file only after connecting; before that, the
            // log is the signal. Give up if nothing happens for 2 minutes.
            None => ticks < 480 && std::fs::metadata(&f.fifo).is_ok() && !stopping,
        };
        if !alive || (stopping && seen_pid.is_none()) {
            break;
        }
    }
    let _ = std::fs::remove_file(&f.ctrl);
    std::thread::sleep(Duration::from_millis(300));
    let _ = std::fs::remove_dir_all(&f.dir);
    finish(a, id, c);
}

/// Re-attach global connections that outlived a previous app session (crash, or "keep
/// global VPN on exit"). Each root session leaves meta.json + pid in its temp dir.
#[tauri::command]
pub fn vpn_adopt<R: Runtime>(app: AppHandle<R>, state: State<'_, VpnState>) -> Vec<Status> {
    #[cfg(unix)]
    {
        let mut out = vec![];
        let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return out };
        for e in rd.flatten() {
            let dir = e.path();
            if !e.file_name().to_string_lossy().starts_with("tp-vpn-") {
                continue;
            }
            let meta: Option<serde_json::Value> = std::fs::read_to_string(dir.join("meta.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
            let pid = std::fs::read_to_string(dir.join("pid")).ok().and_then(|s| s.trim().parse::<i32>().ok());
            let alive = pid.map(pid_alive).unwrap_or(false);
            if !alive {
                // Leftover of a session whose process is gone (crash, killed app). Young dirs
                // may belong to a connect in progress elsewhere — leave those alone.
                let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs() > 600).unwrap_or(false);
                if old {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                continue;
            }
            let (Some(meta), Some(pid)) = (meta, pid) else { continue };
            if !dir.join("run").exists() {
                continue;
            }
            let _ = pid;
            let id = meta["id"].as_str().unwrap_or_default().to_string();
            if id.is_empty() || state.conns.lock().unwrap().contains_key(&id) {
                continue;
            }
            let log = std::fs::read_to_string(dir.join("log")).unwrap_or_default();
            let ip = log.lines().rev().find_map(|l| l.strip_prefix("Configured as ")).and_then(|r| r.split([',', ' ']).next()).map(String::from);
            let files = RootFiles { log: dir.join("log"), pid_file: dir.join("pid"), ctrl: dir.join("run"), fifo: dir.join("in"), dir: dir.clone() };
            let since = std::fs::metadata(&files.pid_file).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
            let conn = Arc::new(Conn {
                status: Mutex::new(Status {
                    id: id.clone(), name: meta["name"].as_str().unwrap_or("VPN").into(), state: "connected".into(),
                    mode: "global".into(), ip, since, ..Default::default()
                }),
                input: Mutex::new(None),
                log: Mutex::new({
                    let lines: Vec<&str> = log.lines().collect();
                    lines[lines.len().saturating_sub(100)..].iter().map(|l| l.to_string()).collect()
                }),
                ctl: Mutex::new(Some(Ctl::Root { ctrl: files.ctrl.clone() })),
                stopping: Default::default(),
                auth: Default::default(),
            });
            state.conns.lock().unwrap().insert(id.clone(), conn.clone());
            out.push(conn.status.lock().unwrap().clone());
            let req: ConnectReq = serde_json::from_value(serde_json::json!({ "id": id, "name": "", "protocol": "openconnect", "server": "", "mode": "global" })).unwrap();
            let parser: Arc<Mutex<Box<dyn Parser>>> = Arc::new(Mutex::new(Box::new(openconnect::OcParser::new(&req))));
            let iface = meta["iface"].as_str().map(String::from);
            let (a, start) = (app.clone(), log.len() as u64);
            std::thread::spawn(move || root_watch(&a, &id, &conn, &parser, &files, iface.as_deref(), start));
        }
        out
    }
    #[cfg(not(unix))]
    {
        let _ = (app, state);
        vec![]
    }
}

#[cfg(unix)]
trait NonBlock {
    fn custom_flags_nonblock(&mut self) -> &mut Self;
}
#[cfg(unix)]
impl NonBlock for std::fs::OpenOptions {
    fn custom_flags_nonblock(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(libc::O_NONBLOCK)
    }
}

/// Global mode on Windows, in two phases:
/// 1. `openconnect --authenticate` unprivileged (normal pipes: prompts, MFA, certificate).
/// 2. An elevated PowerShell (UAC) reads only the session cookie from our named pipe and
///    starts openconnect with .NET's own stdin/stdout pipes (Wintun tunnel). Output is
///    copied unbuffered to log files we tail; deleting the control file stops it.
///    Neither the password nor the cookie is written to disk.
#[cfg(windows)]
fn start_elevated_windows<R: Runtime>(app: &AppHandle<R>, id: &str, conn: Arc<Conn>, parser: Arc<Mutex<Box<dyn Parser>>>, l: Launch) -> Result<(), String> {
    let (a, c, id, p) = (app.clone(), conn, id.to_string(), parser);
    std::thread::spawn(move || {
        let Some(vars) = authenticate(&c, &a, &p, &l) else {
            finish(&a, &id, &c);
            return;
        };
        if c.stopping.load(std::sync::atomic::Ordering::Relaxed) {
            finish(&a, &id, &c);
            return;
        }
        push_log(&c, "Kimlik doğrulandı; tünel yönetici izniyle başlatılıyor (UAC)…");
        if let Err(e) = tunnel_windows(&a, &id, &c, &p, &l, vars) {
            {
                let mut s = c.status.lock().unwrap();
                s.state = "error".into();
                s.error = Some(e);
                s.error_kind = Some("denied".into());
            }
            finish(&a, &id, &c);
        }
    });
    Ok(())
}

#[cfg(windows)]
fn tunnel_windows<R: Runtime>(a: &AppHandle<R>, id: &str, c: &Arc<Conn>, p: &Arc<Mutex<Box<dyn Parser>>>, l: &Launch, vars: HashMap<String, String>) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("tp-vpn-{tag}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pipe_name = format!("tp-vpn-{tag}");
    let (out_log, err_log, pid_file, ctrl, script) =
        (dir.join("out.log"), dir.join("err.log"), dir.join("pid"), dir.join("run"), dir.join("start.ps1"));
    std::fs::write(&ctrl, b"").map_err(|e| e.to_string())?;
    let iface = "TerminalPlus";

    // Hand the cookie to the elevated wrapper once it connects, then close the pipe.
    let cookie = vars.get("COOKIE").cloned().unwrap_or_default();
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(format!(r"\\.\pipe\{pipe_name}"))
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        let mut server = server;
        let connected = tokio::time::timeout(Duration::from_secs(120), server.connect()).await;
        if matches!(connected, Ok(Ok(()))) {
            let _ = server.write_all(format!("{cookie}\n").as_bytes()).await;
            let _ = server.flush().await;
            let _ = server.disconnect();
        }
    });

    // Tunnel arguments: protocol + script from the profile, the rest from --authenticate.
    let winarg = |a: &str| if a.contains([' ', '"']) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.to_string() };
    let mut args: Vec<String> = vec![format!("--interface={iface}"), "--cookie-on-stdin".into()];
    args.extend(l.args.iter().filter(|a| a.starts_with("--protocol=") || a.starts_with("--script=")).cloned());
    if let Some(f) = vars.get("FINGERPRINT").filter(|f| !f.is_empty()) {
        args.push(format!("--servercert={f}"));
    }
    if let Some(r) = vars.get("RESOLVE").filter(|r| !r.is_empty()) {
        args.push(format!("--resolve={r}"));
    }
    args.push("--reconnect-timeout=60".into());
    let target = vars.get("CONNECT_URL").or(vars.get("HOST")).cloned().unwrap_or_else(|| l.args.last().cloned().unwrap_or_default());
    args.push(target);
    let arg_line = args.iter().map(|x| winarg(x)).collect::<Vec<_>>().join(" ");

    let q = |s: &str| s.replace('\'', "''");
    let ps = format!(
        r#"$ErrorActionPreference = 'Stop'
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', '{pipe}', [System.IO.Pipes.PipeDirection]::In)
$pipe.Connect(60000)
$reader = New-Object System.IO.StreamReader($pipe)
$cookie = $reader.ReadLine()
$reader.Dispose()
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = '{prog}'
$psi.Arguments = '{args}'
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables['LC_ALL'] = 'C'
$proc = [System.Diagnostics.Process]::Start($psi)
Set-Content -Path '{pid}' -Value $proc.Id
$out = New-Object System.IO.FileStream('{out}', 'Append', 'Write', 'ReadWrite', 1)
$err = New-Object System.IO.FileStream('{err}', 'Append', 'Write', 'ReadWrite', 1)
$t1 = $proc.StandardOutput.BaseStream.CopyToAsync($out)
$t2 = $proc.StandardError.BaseStream.CopyToAsync($err)
$proc.StandardInput.WriteLine($cookie)
$proc.StandardInput.Flush()
$proc.StandardInput.Close()
$cookie = $null
while ((Test-Path '{ctrl}') -and -not $proc.HasExited) {{ Start-Sleep -Milliseconds 700 }}
if (-not $proc.HasExited) {{ $proc.Kill() }}
Start-Sleep -Milliseconds 500
$out.Dispose(); $err.Dispose()
"#,
        pipe = pipe_name,
        prog = q(&l.program.to_string_lossy()),
        args = q(&arg_line),
        pid = q(&pid_file.to_string_lossy()),
        out = q(&out_log.to_string_lossy()),
        err = q(&err_log.to_string_lossy()),
        ctrl = q(&ctrl.to_string_lossy()),
    );
    std::fs::write(&script, ps).map_err(|e| e.to_string())?;
    *c.ctl.lock().unwrap() = Some(Ctl::Root { ctrl: ctrl.clone() });

    let launcher = format!(
        "try {{ Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}' }} catch {{ exit 1 }}",
        q(&script.to_string_lossy())
    );
    let ok = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &launcher])
        .creation_flags(0x0800_0000)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("Yönetici izni (UAC) verilmedi".into());
    }

    let (a, c, id, p) = (a.clone(), c.clone(), id.to_string(), p.clone());
    std::thread::spawn(move || {
        let mut pos = [0u64; 2];
        let mut pending = [String::new(), String::new()];
        let mut seen_pid: Option<u32> = None;
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            ticks += 1;
            for (k, f) in [&out_log, &err_log].iter().enumerate() {
                if let Ok(mut fh) = std::fs::File::open(f) {
                    use std::io::Seek;
                    let _ = fh.seek(std::io::SeekFrom::Start(pos[k]));
                    let mut buf = Vec::new();
                    if let Ok(n) = fh.read_to_end(&mut buf) {
                        pos[k] += n as u64;
                        pending[k].push_str(&String::from_utf8_lossy(&buf));
                    }
                }
                while let Some(i) = pending[k].find('\n') {
                    let line: String = pending[k].drain(..=i).collect();
                    let line = line.trim_end().to_string();
                    if line.is_empty() { continue; }
                    push_log(&c, &line);
                    let acts = p.lock().unwrap().line(&line);
                    apply(&a, &c, acts);
                }
            }
            if seen_pid.is_none() {
                seen_pid = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse().ok());
            }
            if ticks % 8 == 0 {
                let connected = c.status.lock().unwrap().state == "connected";
                if connected {
                    if let Some((rx, tx)) = win_iface_bytes(iface) {
                        let mut s = c.status.lock().unwrap();
                        s.rx = rx;
                        s.tx = tx;
                    }
                }
            }
            let stopping = c.stopping.load(std::sync::atomic::Ordering::Relaxed);
            let alive = match seen_pid {
                Some(pid) => win_pid_alive(pid),
                // Waiting for UAC + process start; give up after 2 minutes.
                None => ticks < 480 && !stopping,
            };
            if !alive {
                break;
            }
        }
        let _ = std::fs::remove_file(&ctrl);
        std::thread::sleep(Duration::from_millis(800));
        let _ = std::fs::remove_dir_all(&dir);
        finish(&a, &id, &c);
    });
    Ok(())
}

#[cfg(windows)]
fn win_pid_alive(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
        .unwrap_or(false)
}

/// (rx, tx) from `netsh interface ipv4 show subinterfaces`.
#[cfg(windows)]
fn win_iface_bytes(iface: &str) -> Option<(u64, u64)> {
    use std::os::windows::process::CommandExt;
    let o = std::process::Command::new("netsh")
        .args(["interface", "ipv4", "show", "subinterfaces"])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&o.stdout);
    let line = text.lines().find(|l| l.trim_end().ends_with(iface))?;
    let p: Vec<&str> = line.split_whitespace().collect();
    // MTU  MediaSenseState  Bytes In  Bytes Out  Interface
    Some((p.get(2)?.parse().ok()?, p.get(3)?.parse().ok()?))
}

/// Run a script as root with the OS password dialog. Every call asks for the password.
#[cfg(unix)]
fn elevate(script: &std::path::Path) -> Result<(), String> {
    #[cfg(test)]
    if std::env::var("TP_ELEVATE").as_deref() == Ok("direct") {
        let ok = std::process::Command::new("/bin/sh").arg(script).status().map_err(|e| e.to_string())?.success();
        return if ok { Ok(()) } else { Err("script failed".into()) };
    }
    let path = script.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let out = if cfg!(target_os = "macos") {
        std::process::Command::new("osascript")
            .args([
                "-e",
                &format!("do shell script \"/bin/sh \" & quoted form of \"{path}\" with prompt \"Terminal Plus genel VPN bağlantısı için yönetici izni istiyor.\" with administrator privileges"),
            ])
            .output()
    } else {
        std::process::Command::new("pkexec").args(["/bin/sh", &script.to_string_lossy()]).output()
    }
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") || err.contains("cancel") || out.status.code() == Some(126) {
            Err("Yönetici izni verilmedi".into())
        } else {
            Err(format!("Yönetici izni alınamadı: {}", err.trim()))
        }
    }
}

#[cfg(target_os = "macos")]
fn free_utun() -> String {
    let used = std::process::Command::new("ifconfig").arg("-l").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    (20..64).map(|n| format!("utun{n}")).find(|u| !used.split_whitespace().any(|x| x == u)).unwrap_or_else(|| "utun20".into())
}
#[cfg(all(unix, not(target_os = "macos")))]
fn free_utun() -> String {
    "tpvpn0".into()
}

/// (rx, tx) bytes of an interface, from `netstat -ibn` (macOS) or /sys (Linux).
#[cfg(unix)]
fn iface_bytes(iface: &str) -> Option<(u64, u64)> {
    if cfg!(target_os = "linux") {
        let rd = |f: &str| std::fs::read_to_string(format!("/sys/class/net/{iface}/statistics/{f}")).ok()?.trim().parse().ok();
        return Some((rd("rx_bytes")?, rd("tx_bytes")?));
    }
    let out = std::process::Command::new("netstat").args(["-ibn", "-I", iface]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with(iface) && l.contains("<Link#"))?;
    let p: Vec<&str> = line.split_whitespace().collect();
    // Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll
    let n = p.len();
    Some((p.get(n - 5)?.parse().ok()?, p.get(n - 2)?.parse().ok()?))
}

// The commands below are async so they never run on (and can never stall) the UI thread.
#[tauri::command]
pub async fn vpn_input(state: State<'_, VpnState>, id: String, text: String) -> Result<(), String> {
    let c = state.conns.lock().unwrap().get(&id).cloned().ok_or("NOT_CONNECTED")?;
    let mut w = c.input.lock().unwrap();
    let w = w.as_mut().ok_or("NOT_READY")?;
    w.write_all(format!("{text}\n").as_bytes()).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn vpn_disconnect(state: State<'_, VpnState>, id: String) -> Result<(), String> {
    let c = state.conns.lock().unwrap().get(&id).cloned();
    if let Some(c) = c {
        stop(&c);
    }
    Ok(())
}

#[tauri::command]
pub async fn vpn_list(state: State<'_, VpnState>) -> Result<Vec<Status>, String> {
    let conns: Vec<Arc<Conn>> = state.conns.lock().unwrap().values().cloned().collect();
    Ok(conns.iter().map(|c| c.status.lock().unwrap().clone()).collect())
}

#[tauri::command]
pub async fn vpn_log(state: State<'_, VpnState>, id: String) -> Result<Vec<String>, String> {
    let c = state.conns.lock().unwrap().get(&id).cloned();
    Ok(match c {
        Some(c) => c.log.lock().unwrap().iter().cloned().collect(),
        None => state.last_logs.lock().unwrap().get(&id).cloned().unwrap_or_default(),
    })
}

/// Is the SOCKS proxy of an app-mode connection up? Used before SSH dials through it.
#[tauri::command]
pub async fn vpn_socks_port(state: State<'_, VpnState>, id: String) -> Result<Option<u16>, String> {
    let c = state.conns.lock().unwrap().get(&id).cloned();
    Ok(c.and_then(|c| {
        let s = c.status.lock().unwrap().clone();
        (s.state == "connected" && s.mode == "app").then_some(s.socks_port).flatten()
    }))
}

/// Disconnect everything (app exit). Global connections are left up when `keep_global`.
pub fn shutdown(state: &VpnState, keep_global: bool) {
    for c in state.conns.lock().unwrap().values() {
        let global = matches!(c.ctl.lock().unwrap().as_ref(), Some(Ctl::Root { .. }));
        if !(global && keep_global) {
            stop(c);
        }
    }
}

#[tauri::command]
pub fn vpn_secret_exists(id: String) -> bool {
    secrets::get(&secrets::account("vpw", &id)).is_some()
}

/// Reveal the saved password (profile details, behind an explicit "show" click).
#[tauri::command]
pub fn vpn_secret_reveal(id: String) -> Option<String> {
    secrets::get(&secrets::account("vpw", &id))
}

#[tauri::command]
pub fn vpn_secret_set(id: String, secret: Option<String>) -> Result<(), String> {
    let acc = secrets::account("vpw", &id);
    match secret.filter(|s| !s.is_empty()) {
        Some(s) => secrets::set(&acc, &s),
        None => secrets::delete(&acc),
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end against a real OpenConnect server (e.g. ocserv). Run with:
    //! TP_TEST_VPN='https://127.0.0.1:4443|user|pass|pin-sha256:…|172.17.0.2:2222' \
    //!   cargo test --lib vpn::tests -- --ignored --nocapture
    use super::*;
    use tauri::Listener;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn app_mode_socks_ssh() {
        let spec = std::env::var("TP_TEST_VPN").expect("TP_TEST_VPN");
        let p: Vec<&str> = spec.split('|').collect();
        let app = tauri::test::mock_app();
        app.manage(VpnState::default());
        let h = app.handle().clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Status>();
        h.listen("vpn-status", move |e| {
            let _ = tx.send(serde_json::from_str(e.payload()).unwrap());
        });
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "t1", "name": "test", "protocol": "openconnect", "ocProtocol": "anyconnect",
            "server": p[0], "user": p[1], "password": p[2], "servercert": p[3], "mode": "app"
        })).unwrap();
        vpn_connect(h.clone(), h.state(), req).await.unwrap();
        let mut port = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while port.is_none() {
            let s = tokio::time::timeout_at(deadline, rx.recv()).await.expect("timeout").unwrap();
            assert_ne!(s.state, "error", "vpn error: {:?} {:?}", s.error, vpn_log(h.state(), "t1".into()).await.unwrap());
            if s.state == "connected" {
                assert!(s.ip.as_deref().unwrap_or("").starts_with("10."), "ip {:?}", s.ip);
                port = s.socks_port;
            }
        }
        let port = port.unwrap();
        assert_eq!(vpn_socks_port(h.state(), "t1".into()).await.unwrap(), Some(port));
        // Through the tunnel to a host only reachable via VPN.
        let (host, tport) = p[4].split_once(':').unwrap();
        // "connected" must mean usable: the very first SOCKS dial has to work.
        let mut s = crate::socks::connect(&format!("127.0.0.1:{port}"), host, tport.parse().unwrap()).await
            .expect("SOCKS not usable right after 'connected'");
        let mut banner = [0u8; 8];
        tokio::io::AsyncReadExt::read_exact(&mut s, &mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-");
        // Stats arrive via SIGUSR1.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let st = vpn_list(h.state()).await.unwrap().into_iter().find(|x| x.id == "t1").unwrap();
        assert!(st.rx > 0 && st.tx > 0, "stats {st:?}");
        vpn_disconnect(h.state(), "t1".into()).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let s = tokio::time::timeout_at(deadline, rx.recv()).await.expect("no disconnect").unwrap();
            if s.state == "disconnected" { break; }
            assert_ne!(s.state, "error", "{:?}", s.error);
        }
        assert!(vpn_list(h.state()).await.unwrap().is_empty());
    }

    /// Regression: stats polling while connected used to deadlock the status mutex and
    /// freeze the UI. A fake openconnect "connects"; counters come from lo0.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread")]
    async fn global_connected_stats_do_not_deadlock() {
        std::env::set_var("TP_ELEVATE", "direct");
        std::env::set_var("TP_TEST_IFACE", "lo0");
        let dir = std::env::temp_dir().join(format!("tp_fakeoc_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("openconnect");
        std::fs::write(&fake, "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --pid-file=*) echo $$ > \"${a#--pid-file=}\";; esac; done\n\
echo 'Configured as 10.9.9.9, with SSL connected'\ntrap 'exit 0' INT TERM\nwhile :; do sleep 1; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let app = tauri::test::mock_app();
        app.manage(VpnState::default());
        let h = app.handle().clone();
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "dl", "name": "fake", "protocol": "openconnect", "server": "x", "mode": "global"
        })).unwrap();
        let conn = Arc::new(Conn {
            status: Mutex::new(Status { id: "dl".into(), name: "fake".into(), state: "connecting".into(), mode: "global".into(), ..Default::default() }),
            input: Mutex::new(None), log: Mutex::new(VecDeque::new()), ctl: Mutex::new(None), stopping: Default::default(),
            auth: Default::default(),
        });
        h.state::<VpnState>().conns.lock().unwrap().insert("dl".into(), conn.clone());
        let parser: Arc<Mutex<Box<dyn Parser>>> = Arc::new(Mutex::new(Box::new(openconnect::OcParser::new(&req))));
        start_root(&h, "dl", conn, parser, Launch { program: fake, args: vec![] }).unwrap();

        // Stay connected across several stats ticks; every status query must answer fast.
        let mut connected_polls = 0;
        for _ in 0..16 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let list = tokio::time::timeout(Duration::from_secs(2), vpn_list(h.state())).await.expect("vpn_list blocked (deadlock)").unwrap();
            if list.iter().any(|s| s.state == "connected") { connected_polls += 1; }
        }
        assert!(connected_polls >= 8, "never stayed connected");
        let s = vpn_list(h.state()).await.unwrap().into_iter().next().unwrap();
        assert!(s.rx > 0 && s.tx > 0, "no interface stats: {s:?}");
        vpn_disconnect(h.state(), "dl".into()).await.unwrap();
        for _ in 0..40 {
            if vpn_list(h.state()).await.unwrap().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(vpn_list(h.state()).await.unwrap().is_empty(), "not cleaned up after disconnect");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Two-phase flow used for Windows global mode: `--authenticate` (password via a normal
    /// pipe) → session cookie → second openconnect with --cookie-on-stdin builds the tunnel.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn authenticate_then_cookie_tunnel() {
        let spec = std::env::var("TP_TEST_VPN").expect("TP_TEST_VPN");
        let p: Vec<&str> = spec.split('|').collect();
        let app = tauri::test::mock_app();
        app.manage(VpnState::default());
        let h = app.handle().clone();
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "a1", "name": "t", "protocol": "openconnect", "ocProtocol": "anyconnect",
            "server": p[0], "user": p[1], "password": p[2], "servercert": p[3], "mode": "global"
        })).unwrap();
        let launch = openconnect::launch(&h, &req, None).unwrap();
        let conn = Arc::new(Conn {
            status: Mutex::new(Status { id: "a1".into(), state: "connecting".into(), ..Default::default() }),
            input: Mutex::new(None), log: Mutex::new(VecDeque::new()), ctl: Mutex::new(None), stopping: Default::default(),
            auth: Default::default(),
        });
        let parser: Arc<Mutex<Box<dyn Parser>>> = Arc::new(Mutex::new(Box::new(openconnect::OcParser::new(&req))));
        let (h2, c2, p2) = (h.clone(), conn.clone(), parser.clone());
        let vars = tokio::task::spawn_blocking(move || authenticate(&c2, &h2, &p2, &launch)).await.unwrap()
            .unwrap_or_else(|| panic!("authenticate failed: {:?}", conn.log.lock().unwrap()));
        assert!(!vars["COOKIE"].is_empty());
        assert!(vars["FINGERPRINT"].starts_with("pin-sha256:") || vars["FINGERPRINT"].len() >= 40, "{:?}", vars.get("FINGERPRINT"));
        assert!(conn.log.lock().unwrap().iter().all(|l| !l.contains(&vars["COOKIE"])), "cookie leaked into the log");

        // Phase 2 equivalent: tunnel from the cookie alone (app mode so no root is needed).
        let port = free_port().unwrap();
        let ocproxy = components::find(&h, "openconnect", "ocproxy").unwrap();
        let target = vars.get("CONNECT_URL").or(vars.get("HOST")).unwrap().clone();
        let mut child = std::process::Command::new(components::find(&h, "openconnect", "openconnect").unwrap())
            .args(["--protocol=anyconnect", "--cookie-on-stdin", &format!("--servercert={}", vars["FINGERPRINT"]),
                   "--script-tun", &format!("--script={} -D {port}", ocproxy.display()), &target])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().unwrap();
        child.stdin.take().unwrap().write_all(format!("{}\n", vars["COOKIE"]).as_bytes()).unwrap();
        assert!(wait_port(port, Duration::from_secs(15)), "tunnel from cookie never came up");
        let (host, tport) = p[4].split_once(':').unwrap();
        let mut s = None;
        for _ in 0..20 {
            if let Ok(x) = crate::socks::connect(&format!("127.0.0.1:{port}"), host, tport.parse().unwrap()).await { s = Some(x); break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let mut banner = [0u8; 8];
        tokio::io::AsyncReadExt::read_exact(s.as_mut().expect("socks"), &mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-");
        unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
        let _ = child.wait();
        let _ = std::process::Command::new("pkill").args(["-f", &format!("ocproxy -D {port}")]).status();
    }

    #[test]
    fn auth_vars_are_unquoted() {
        assert_eq!(auth_var("COOKIE='a'\\''b'"), Some(("COOKIE".into(), "a'b".into())));
        assert_eq!(auth_var("CONNECT_URL='https://h:443/x'").unwrap().1, "https://h:443/x");
        assert_eq!(auth_var("FOO=bar"), None);
    }

    /// Global-mode plumbing without root: FIFO password, log tail, failure + cleanup.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn global_mode_plumbing_without_root() {
        std::env::set_var("TP_ELEVATE", "direct");
        let spec = std::env::var("TP_TEST_VPN").expect("TP_TEST_VPN");
        let p: Vec<&str> = spec.split('|').collect();
        let app = tauri::test::mock_app();
        app.manage(VpnState::default());
        let h = app.handle().clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let tx2 = tx.clone();
        h.listen("vpn-status", move |e| { let _ = tx.send(serde_json::from_str(e.payload()).unwrap()); });
        h.listen("vpn-prompt", move |e| { let _ = tx2.send(serde_json::from_str(e.payload()).unwrap()); });
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "g1", "name": "t", "protocol": "openconnect", "ocProtocol": "anyconnect",
            "server": p[0], "user": p[1], "password": p[2], "servercert": p[3], "mode": "global"
        })).unwrap();
        vpn_connect(h.clone(), h.state(), req).await.unwrap();
        let last = loop {
            let v = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.expect("timeout").unwrap();
            assert!(v.get("kind").is_none(), "unexpected prompt (password not delivered via FIFO?): {v}");
            if v["state"] == "error" || v["state"] == "disconnected" { break v; }
        };
        let log = vpn_log(h.state(), "g1".into()).await.unwrap();
        let joined = last.to_string();
        // Auth went through the FIFO; then the tun device can't be opened without root.
        assert_ne!(last["errorKind"], "auth", "{joined}");
        assert!(last["error"].as_str().unwrap_or("").to_lowercase().contains("tun") || joined.contains("tun"), "{joined} {log:?}");
        for _ in 0..40 {
            if vpn_list(h.state()).await.unwrap().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(vpn_list(h.state()).await.unwrap().is_empty(), "connection not cleaned up");
        // Only this test's session dir (a real VPN may be running on the dev machine).
        let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("tp-vpn-"))
            .filter(|e| std::fs::read_to_string(e.path().join("meta.json")).map(|m| m.contains("\"g1\"")).unwrap_or(false))
            .collect();
        assert!(leftovers.is_empty(), "temp dirs left: {leftovers:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn wrong_password_and_unknown_cert() {
        let spec = std::env::var("TP_TEST_VPN").expect("TP_TEST_VPN");
        let p: Vec<&str> = spec.split('|').collect();
        let app = tauri::test::mock_app();
        app.manage(VpnState::default());
        let h = app.handle().clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let tx2 = tx.clone();
        h.listen("vpn-status", move |e| { let _ = tx.send(serde_json::from_str(e.payload()).unwrap()); });
        h.listen("vpn-prompt", move |e| { let _ = tx2.send(serde_json::from_str(e.payload()).unwrap()); });
        // Unknown certificate → cert error carrying the pin.
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "c1", "name": "t", "protocol": "openconnect", "ocProtocol": "anyconnect",
            "server": p[0], "user": p[1], "password": p[2], "mode": "app"
        })).unwrap();
        vpn_connect(h.clone(), h.state(), req).await.unwrap();
        let mut got = None;
        while got.is_none() {
            let v = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("timeout").unwrap();
            if v["state"] == "error" { got = Some(v); }
        }
        let v = got.unwrap();
        assert_eq!(v["errorKind"], "cert", "{v}");
        assert_eq!(v["pin"].as_str(), Some(p[3]));
        // Wrong password → asked again (prompt), then we give up.
        let req: ConnectReq = serde_json::from_value(serde_json::json!({
            "id": "c2", "name": "t", "protocol": "openconnect", "ocProtocol": "anyconnect",
            "server": p[0], "user": p[1], "password": "WRONG", "servercert": p[3], "mode": "app"
        })).unwrap();
        vpn_connect(h.clone(), h.state(), req).await.unwrap();
        loop {
            let v = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("timeout").unwrap();
            if v["kind"] == "password" { break; }
            assert_ne!(v["state"], "connected");
        }
        vpn_disconnect(h.state(), "c2".into()).await.unwrap();
    }
}
