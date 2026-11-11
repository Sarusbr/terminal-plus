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
        // Prefer PowerShell 7, then Windows PowerShell.
        for p in ["pwsh.exe", "powershell.exe"] {
            if which(p) {
                return p.into();
            }
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() }
        })
    }
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

#[tauri::command]
pub fn local_pty_open<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SshState>,
    pty_id: String,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> Result<Option<&'static str>, String> {
    let size = PtySize { rows: rows.max(2), cols: cols.max(2), pixel_width: 0, pixel_height: 0 };
    let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;

    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    // Run mode for panel commands (see runner.ts): "zle" for zsh, "line" for bash.
    let mut mode = None;
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
    Ok(mode)
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
        let mode = local_pty_open(h.clone(), h.state(), "l1".into(), 80, 24, ch).unwrap();
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
