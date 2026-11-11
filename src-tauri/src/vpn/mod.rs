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
}

#[derive(Default)]
pub struct VpnState {
    conns: Mutex<HashMap<String, Arc<Conn>>>,
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
        st.conns.lock().unwrap().remove(id);
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
        // Tail the log + watch the pid.
        let mut pos = 0u64;
        let mut seen_pid = None;
        let mut pending = String::new();
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            ticks += 1;
            if let Ok(mut f) = std::fs::File::open(&log) {
                use std::io::Seek;
                let _ = f.seek(std::io::SeekFrom::Start(pos));
                let mut chunk = String::new();
                if let Ok(n) = f.read_to_string(&mut chunk) {
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
                push_log(&c, &line);
                let acts = p.lock().unwrap().line(&line);
                apply(&a, &c, acts);
            }
            if !pending.trim().is_empty() {
                let acts = p.lock().unwrap().prompt(&pending);
                if !acts.is_empty() {
                    pending.clear();
                    apply(&a, &c, acts);
                }
            }
            if seen_pid.is_none() {
                seen_pid = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok());
            }
            // Stats from interface counters every ~2 s.
            if ticks % 8 == 0 {
                if let (Some(i), true) = (&iface, c.status.lock().unwrap().state == "connected") {
                    if let Some((rx, tx)) = iface_bytes(i) {
                        let mut s = c.status.lock().unwrap();
                        s.rx = rx;
                        s.tx = tx;
                    }
                }
            }
            let alive = match seen_pid {
                Some(pid) => pid_alive(pid),
                // openconnect writes the pid file only after connecting; before that, the
                // log is the signal. Give up if nothing happens for 2 minutes.
                None => ticks < 480 && std::fs::metadata(&fifo).is_ok() && !c.stopping.load(std::sync::atomic::Ordering::Relaxed),
            };
            if !alive {
                if seen_pid.is_none() && !c.stopping.load(std::sync::atomic::Ordering::Relaxed) && c.status.lock().unwrap().state == "connecting" {
                    // No pid yet: maybe openconnect quit early — the log tells why.
                }
                break;
            }
            if c.stopping.load(std::sync::atomic::Ordering::Relaxed) && seen_pid.is_none() {
                break;
            }
        }
        let _ = std::fs::remove_file(&ctrl);
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::fs::remove_dir_all(&dir);
        finish(&a, &id, &c);
    });
    Ok(())
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

/// `Write` that forwards bytes to the named-pipe writer task.
#[cfg(windows)]
struct ChanWriter(tokio::sync::mpsc::UnboundedSender<Vec<u8>>);
#[cfg(windows)]
impl Write for ChanWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.send(b.to_vec()).map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Global mode on Windows: openconnect (+ Wintun) started elevated through UAC.
/// stdin is a named pipe served by us (secrets never touch disk); output goes to log files
/// we tail; deleting the control file makes the elevated watcher stop openconnect.
#[cfg(windows)]
fn start_elevated_windows<R: Runtime>(app: &AppHandle<R>, id: &str, conn: Arc<Conn>, parser: Arc<Mutex<Box<dyn Parser>>>, l: Launch) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("tp-vpn-{tag}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pipe = format!(r"\\.\pipe\tp-vpn-{tag}");
    let (out_log, err_log, pid_file, ctrl, script) =
        (dir.join("out.log"), dir.join("err.log"), dir.join("pid"), dir.join("run"), dir.join("start.ps1"));
    std::fs::write(&ctrl, b"").map_err(|e| e.to_string())?;
    let iface = "TerminalPlus";

    // Named pipe server → openconnect stdin.
    let server = ServerOptions::new().first_pipe_instance(true).create(&pipe).map_err(|e| e.to_string())?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    *conn.input.lock().unwrap() = Some(Box::new(ChanWriter(tx)));
    tauri::async_runtime::spawn(async move {
        let mut server = server;
        if server.connect().await.is_err() {
            return;
        }
        while let Some(b) = rx.recv().await {
            if server.write_all(&b).await.is_err() {
                break;
            }
            let _ = server.flush().await;
        }
    });

    let q = |s: &str| s.replace('\'', "''");
    // Windows command-line quoting for openconnect's arguments.
    let winarg = |a: &str| if a.contains([' ', '"']) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.to_string() };
    let mut args: Vec<String> = vec![format!("--interface={iface}")];
    args.extend(l.args.iter().map(|a| winarg(a)));
    let ps = format!(
        "$ErrorActionPreference='Stop'\n$env:LC_ALL='C'\n\
         $p = Start-Process -FilePath '{prog}' -ArgumentList '{args}' -RedirectStandardInput '{pipe}' \
         -RedirectStandardOutput '{out}' -RedirectStandardError '{err}' -WindowStyle Hidden -PassThru\n\
         Set-Content -Path '{pid}' -Value $p.Id\n\
         while ((Test-Path '{ctrl}') -and -not $p.HasExited) {{ Start-Sleep -Milliseconds 700 }}\n\
         if (-not $p.HasExited) {{ Stop-Process -Id $p.Id -Force }}\n",
        prog = q(&l.program.to_string_lossy()),
        args = q(&args.join(" ")),
        pipe = q(&pipe),
        out = q(&out_log.to_string_lossy()),
        err = q(&err_log.to_string_lossy()),
        pid = q(&pid_file.to_string_lossy()),
        ctrl = q(&ctrl.to_string_lossy()),
    );
    std::fs::write(&script, ps).map_err(|e| e.to_string())?;
    *conn.ctl.lock().unwrap() = Some(Ctl::Root { ctrl: ctrl.clone() });

    let (a, c, id, p) = (app.clone(), conn.clone(), id.to_string(), parser);
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
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
            {
                let mut s = c.status.lock().unwrap();
                s.state = "error".into();
                s.error = Some("Yönetici izni (UAC) verilmedi".into());
                s.error_kind = Some("denied".into());
            }
            let _ = std::fs::remove_dir_all(&dir);
            finish(&a, &id, &c);
            return;
        }
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
                    let mut chunk = String::new();
                    if let Ok(n) = fh.read_to_string(&mut chunk) {
                        pos[k] += n as u64;
                        pending[k].push_str(&chunk);
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
                if !pending[k].trim().is_empty() {
                    let acts = p.lock().unwrap().prompt(&pending[k]);
                    if !acts.is_empty() {
                        pending[k].clear();
                        apply(&a, &c, acts);
                    }
                }
            }
            if seen_pid.is_none() {
                seen_pid = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse().ok());
            }
            if ticks % 8 == 0 && c.status.lock().unwrap().state == "connected" {
                if let Some((rx, tx)) = win_iface_bytes(iface) {
                    let mut s = c.status.lock().unwrap();
                    s.rx = rx;
                    s.tx = tx;
                }
            }
            let alive = match seen_pid {
                Some(pid) => win_pid_alive(pid),
                None => ticks < 480 && !c.stopping.load(std::sync::atomic::Ordering::Relaxed),
            };
            if !alive {
                break;
            }
        }
        let _ = std::fs::remove_file(&ctrl);
        std::thread::sleep(Duration::from_millis(300));
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

#[tauri::command]
pub fn vpn_input(state: State<'_, VpnState>, id: String, text: String) -> Result<(), String> {
    let conns = state.conns.lock().unwrap();
    let c = conns.get(&id).ok_or("NOT_CONNECTED")?;
    let mut w = c.input.lock().unwrap();
    let w = w.as_mut().ok_or("NOT_READY")?;
    w.write_all(format!("{text}\n").as_bytes()).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vpn_disconnect(state: State<'_, VpnState>, id: String) {
    if let Some(c) = state.conns.lock().unwrap().get(&id) {
        stop(c);
    }
}

#[tauri::command]
pub fn vpn_list(state: State<'_, VpnState>) -> Vec<Status> {
    state.conns.lock().unwrap().values().map(|c| c.status.lock().unwrap().clone()).collect()
}

#[tauri::command]
pub fn vpn_log(state: State<'_, VpnState>, id: String) -> Vec<String> {
    state.conns.lock().unwrap().get(&id).map(|c| c.log.lock().unwrap().iter().cloned().collect()).unwrap_or_default()
}

/// Is the SOCKS proxy of an app-mode connection up? Used before SSH dials through it.
#[tauri::command]
pub fn vpn_socks_port(state: State<'_, VpnState>, id: String) -> Option<u16> {
    let conns = state.conns.lock().unwrap();
    let s = conns.get(&id)?.status.lock().unwrap().clone();
    (s.state == "connected" && s.mode == "app").then_some(s.socks_port).flatten()
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
            assert_ne!(s.state, "error", "vpn error: {:?} {:?}", s.error, vpn_log(h.state(), "t1".into()));
            if s.state == "connected" {
                assert!(s.ip.as_deref().unwrap_or("").starts_with("10."), "ip {:?}", s.ip);
                port = s.socks_port;
            }
        }
        let port = port.unwrap();
        assert_eq!(vpn_socks_port(h.state(), "t1".into()), Some(port));
        // Through the tunnel to a host only reachable via VPN.
        let (host, tport) = p[4].split_once(':').unwrap();
        let mut s = None;
        for _ in 0..20 {
            match crate::socks::connect(&format!("127.0.0.1:{port}"), host, tport.parse().unwrap()).await {
                Ok(x) => { s = Some(x); break; }
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
        let mut s = s.expect("socks connect");
        let mut banner = [0u8; 8];
        tokio::io::AsyncReadExt::read_exact(&mut s, &mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-");
        // Stats arrive via SIGUSR1.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let st = vpn_list(h.state()).into_iter().find(|x| x.id == "t1").unwrap();
        assert!(st.rx > 0 && st.tx > 0, "stats {st:?}");
        vpn_disconnect(h.state(), "t1".into());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let s = tokio::time::timeout_at(deadline, rx.recv()).await.expect("no disconnect").unwrap();
            if s.state == "disconnected" { break; }
            assert_ne!(s.state, "error", "{:?}", s.error);
        }
        assert!(vpn_list(h.state()).is_empty());
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
        let log = vpn_log(h.state(), "g1".into());
        let joined = last.to_string();
        // Auth went through the FIFO; then the tun device can't be opened without root.
        assert_ne!(last["errorKind"], "auth", "{joined}");
        assert!(last["error"].as_str().unwrap_or("").to_lowercase().contains("tun") || joined.contains("tun"), "{joined} {log:?}");
        for _ in 0..40 {
            if vpn_list(h.state()).is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(vpn_list(h.state()).is_empty(), "connection not cleaned up");
        let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("tp-vpn-")).collect();
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
        vpn_disconnect(h.state(), "c2".into());
    }
}
