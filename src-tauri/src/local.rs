//! Local shell (no SSH). Uses the OS pty (openpty on Unix, ConPTY on Windows) and plugs
//! into the same input queue as SSH channels, so pty_write/resize/close work unchanged.

use std::io::{Read, Write};
use std::path::PathBuf;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::mpsc;

use crate::ssh::{PtyCmd, PtyExit, SshState};

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() }
        })
    }
}

/// Shells offered in the "+" menu: (id, executable, label).
fn shell_options() -> Vec<(&'static str, String, String)> {
    #[cfg(windows)]
    {
        let mut v = vec![
            ("cmd", default_shell(), "CMD".to_string()),
            ("powershell", "powershell.exe".to_string(), "PowerShell".to_string()),
        ];
        if which("pwsh.exe") {
            v.push(("pwsh", "pwsh.exe".to_string(), "PowerShell 7".to_string()));
        }
        v
    }
    #[cfg(not(windows))]
    {
        let sh = default_shell();
        let name = std::path::Path::new(&sh).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        vec![("default", sh, name)]
    }
}

#[derive(Serialize)]
pub struct ShellOption {
    id: &'static str,
    name: String,
}

#[tauri::command]
pub fn local_shells() -> Vec<ShellOption> {
    shell_options().into_iter().map(|(id, _, name)| ShellOption { id, name }).collect()
}

/// PowerShell: report the cwd to the terminal (OSC 9;9) before the user's own prompt, and
/// keep space-prefixed lines out of PSReadLine history.
#[cfg_attr(not(windows), allow(dead_code))]
const PS_INIT: &str = r#"$global:__tp_prompt = $function:prompt
function global:prompt {
  $e = [char]27
  $loc = $executionContext.SessionState.Path.CurrentLocation
  $p = if ($loc.Provider.Name -eq 'FileSystem') { $loc.ProviderPath } else { '' }
  "$e]9;9;$p$e" + (& $global:__tp_prompt)
}
if (Get-Module PSReadLine) { Set-PSReadLineOption -AddToHistoryHandler { param($line) -not $line.StartsWith(' ') } }
"#;

#[cfg_attr(not(windows), allow(dead_code))]
fn ps_encoded(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

#[cfg(windows)]
fn which(exe: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        .unwrap_or(false)
}

/// zsh: our ZDOTDIR sources the user's real startup files stage by stage (like VS Code's
/// shell integration), then installs the history-free panel-command widgets.
const ZSH_ENV: &str = r#"_tp_zd=$ZDOTDIR
ZDOTDIR=${TP_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zshenv ]] && . $ZDOTDIR/.zshenv
TP_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$_tp_zd
"#;
const ZSH_PROFILE: &str = r#"ZDOTDIR=$TP_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zprofile ]] && . $ZDOTDIR/.zprofile
ZDOTDIR=$_tp_zd
"#;
const ZSH_RC: &str = r#"[[ $HISTFILE == $_tp_zd/* ]] && HISTFILE=$TP_USER_ZDOTDIR/${HISTFILE:t}
ZDOTDIR=$TP_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zshrc ]] && . $ZDOTDIR/.zshrc
ZDOTDIR=$_tp_zd
_tp_begin() { _tp_cmd=; zle -K _tpmap; }
_tp_acc() { _tp_cmd+=$KEYS; }
_tp_run() { zle -K main; local c=$_tp_cmd; _tp_cmd=; eval "$c"; zle reset-prompt; zle -R; }
zle -N _tp_begin; zle -N _tp_acc; zle -N _tp_run
bindkey -N _tpmap
bindkey -M _tpmap -R '^@'-'^?' _tp_acc
bindkey -M _tpmap -R '\M-^@'-'\M-^?' _tp_acc
bindkey -M _tpmap '^^' _tp_run
for _tp_m in emacs viins vicmd; do bindkey -M $_tp_m '^[[57401~' _tp_begin; done; unset _tp_m
"#;
const ZSH_LOGIN: &str = r#"ZDOTDIR=$TP_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zlogin ]] && . $ZDOTDIR/.zlogin
unset _tp_zd
"#;
/// bash: login-equivalent startup via --rcfile, then ignorespace for panel commands.
const BASH_RC: &str = r#"[ -r /etc/profile ] && . /etc/profile
if [ -r ~/.bash_profile ]; then . ~/.bash_profile
elif [ -r ~/.bash_login ]; then . ~/.bash_login
elif [ -r ~/.profile ]; then . ~/.profile
elif [ -r ~/.bashrc ]; then . ~/.bashrc; fi
HISTCONTROL="ignorespace${HISTCONTROL:+:$HISTCONTROL}"
"#;

/// Write integration files; returns (dir, kind) where kind is "zsh" | "bash".
fn integration_dir<R: Runtime>(app: &AppHandle<R>, shell: &str) -> Option<(PathBuf, &'static str)> {
    let name = std::path::Path::new(shell).file_name()?.to_string_lossy().into_owned();
    let base = app.path().app_data_dir().ok()?.join("shell");
    let kind = match name.as_str() {
        "zsh" => "zsh",
        "bash" => "bash",
        _ => return None,
    };
    let dir = base.join(kind);
    std::fs::create_dir_all(&dir).ok()?;
    let files: &[(&str, &str)] = if kind == "zsh" {
        &[(".zshenv", ZSH_ENV), (".zprofile", ZSH_PROFILE), (".zshrc", ZSH_RC), (".zlogin", ZSH_LOGIN)]
    } else {
        &[("bashrc", BASH_RC)]
    };
    for (f, body) in files {
        let p = dir.join(f);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(*body) {
            std::fs::write(&p, body).ok()?;
        }
    }
    Some((dir, kind))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInfo {
    shell: String,
    user: String,
}

#[tauri::command]
pub fn local_info() -> LocalInfo {
    let shell = default_shell();
    let name = std::path::Path::new(&shell)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(shell);
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    LocalInfo { shell: name, user }
}

#[derive(Serialize)]
pub struct LocalOpen {
    /// Panel run mode (runner.ts): "zle" (zsh), "line" (bash) or null.
    mode: Option<&'static str>,
    /// Shell kind: "cmd" | "powershell" | "posix"
    kind: &'static str,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn local_pty_open<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SshState>,
    pty_id: String,
    cols: u16,
    rows: u16,
    shell: Option<String>,
    on_data: Channel<InvokeResponseBody>,
) -> Result<LocalOpen, String> {
    let size = PtySize { rows: rows.max(2), cols: cols.max(2), pixel_width: 0, pixel_height: 0 };
    let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;

    let opts = shell_options();
    let (shell_id, shell) = opts
        .iter()
        .find(|(id, _, _)| Some(*id) == shell.as_deref())
        .or(opts.first())
        .map(|(id, exe, _)| (*id, exe.clone()))
        .unwrap_or(("default", default_shell()));
    let mut cmd = CommandBuilder::new(&shell);
    let mut mode = None;
    let mut kind = "posix";
    #[cfg(windows)]
    {
        if shell_id == "cmd" {
            // Prompt reports the cwd (OSC 9;9) — same trick Windows Terminal documents.
            let user = std::env::var("PROMPT").unwrap_or_else(|_| "$P$G".into());
            cmd.env("PROMPT", format!("$e]9;9;$P$e\\{user}"));
            kind = "cmd";
        } else {
            cmd.args(["-NoLogo", "-NoExit", "-EncodedCommand", &ps_encoded(PS_INIT)]);
            kind = "powershell";
        }
    }
    #[cfg(not(windows))]
    let _ = shell_id;
    #[cfg(not(windows))]
    match integration_dir(&app, &shell) {
        Some((dir, "zsh")) => {
            cmd.arg("-l");
            if let Some(zd) = std::env::var_os("ZDOTDIR") {
                cmd.env("TP_USER_ZDOTDIR", zd);
            }
            cmd.env("ZDOTDIR", &dir);
            mode = Some("zle");
        }
        Some((dir, _)) => {
            cmd.args(["--rcfile", &dir.join("bashrc").to_string_lossy(), "-i"]);
            mode = Some("line");
        }
        None => cmd.arg("-l"), // login shell: loads the user's profile like Terminal.app does
    }
    let _ = &mut kind;
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "TerminalPlus");
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut killer = child.clone_killer();
    let shell_pid = child.process_id();
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let master = pair.master;

    let (tx, mut rx) = mpsc::unbounded_channel::<PtyCmd>();
    state.ptys.lock().unwrap().insert(pty_id.clone(), ("local".into(), tx));

    // Input / resize / close.
    std::thread::spawn(move || {
        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                PtyCmd::Data(d) => {
                    if writer.write_all(&d).and_then(|_| writer.flush()).is_err() {
                        break;
                    }
                }
                PtyCmd::Resize(c, r) => {
                    let _ = master.resize(PtySize {
                        rows: r as u16, cols: c as u16, pixel_width: 0, pixel_height: 0,
                    });
                }
                PtyCmd::Busy(tx) => {
                    #[cfg(unix)]
                    let busy = match (master.process_group_leader(), shell_pid) {
                        (Some(fg), Some(sh)) => Some(fg as u32 != sh),
                        _ => None,
                    };
                    #[cfg(not(unix))]
                    let busy = { let _ = shell_pid; None };
                    let _ = tx.send(busy);
                }
                PtyCmd::Close => {
                    let _ = killer.kill();
                    break;
                }
            }
        }
    });

    // Output → webview as raw bytes; on EOF report exit code.
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if on_data.send(InvokeResponseBody::Raw(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code());
        if let Some(st) = app.try_state::<SshState>() {
            st.ptys.lock().unwrap().remove(&pty_id);
        }
        let _ = app.emit("pty-exit", PtyExit { pty_id, code: code.or(Some(0)) });
    });
    Ok(LocalOpen { mode, kind })
}

#[derive(Serialize)]
pub struct ExecOut {
    code: i32,
    out: String,
    err: String,
}

/// Run a program directly (no shell, no terminal) — used by panels on Windows local tabs.
#[tauri::command]
pub async fn local_exec(program: String, args: Vec<String>, cwd: Option<String>, timeout_ms: Option<u64>) -> Result<ExecOut, String> {
    let mut c = tokio::process::Command::new(&program);
    c.args(&args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    if let Some(d) = cwd.filter(|d| std::path::Path::new(d).is_dir()) {
        c.current_dir(d);
    }
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    let out = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms.unwrap_or(30_000)), c.output())
        .await
        .map_err(|_| "TIMEOUT".to_string())?
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { format!("NOT_FOUND: {program}") } else { e.to_string() })?;
    Ok(ExecOut {
        code: out.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&out.stdout).into_owned(),
        err: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn local_shell_roundtrip() {
        let app = tauri::test::mock_app();
        app.manage(SshState::default());
        let h = app.handle().clone();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let ch = Channel::new(move |b| {
            if let InvokeResponseBody::Raw(v) = b {
                let _ = tx.send(v);
            }
            Ok(())
        });
        let mode = local_pty_open(h.clone(), h.state(), "l1".into(), 80, 24, None, ch).unwrap().mode;
        let zsh = default_shell().ends_with("zsh");
        if zsh {
            assert_eq!(mode, Some("zle"));
            // Widget path: runs without echo/history; ZDOTDIR restored to the user's.
            crate::ssh::pty_write(h.state(), "l1".into(), "\x1b[57401~echo W_$((1+1)) Z=${ZDOTDIR:-$HOME}\x1e".into()).unwrap();
        }
        crate::ssh::pty_write(h.state(), "l1".into(), "echo LOCAL_$((20+1))\n".into()).unwrap();
        let mut out = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !out.contains("LOCAL_21") || (zsh && !out.contains("W_2")) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(b) => out.push_str(&String::from_utf8_lossy(&b)),
                Err(_) => panic!("no output: {out:?}"),
            }
        }
        if zsh {
            let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
            let user_zd = std::env::var("ZDOTDIR").unwrap_or(home);
            assert!(out.contains(&format!("Z={user_zd}")), "ZDOTDIR not restored: {out:?}");
        }
        crate::ssh::pty_close(h.state(), "l1".into()).unwrap();
    }
}
