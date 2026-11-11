//! Persistent non-secret data: saved servers and settings (JSON in the app config dir),
//! plus `~/.ssh/config` import.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn config_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

fn valid_name(name: &str) -> bool {
    matches!(name, "servers" | "settings" | "vpn")
}

/// Load a JSON document ("servers" or "settings"). Returns null when absent.
#[tauri::command]
pub fn store_load(app: AppHandle, name: String) -> Result<Value, String> {
    if !valid_name(&name) {
        return Err("invalid store".into());
    }
    let path = config_file(&app, &format!("{name}.json"))?;
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e.to_string()),
    }
}

/// Atomically save a JSON document.
#[tauri::command]
pub fn store_save(app: AppHandle, name: String, data: Value) -> Result<(), String> {
    if !valid_name(&name) {
        return Err("invalid store".into());
    }
    let path = config_file(&app, &format!("{name}.json"))?;
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHost {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub key_path: Option<String>,
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    p.to_string()
}

/// Minimal `~/.ssh/config` parser: concrete `Host` aliases (no wildcards) with
/// HostName / Port / User / IdentityFile. `Include` and `Match` are ignored.
pub fn parse_ssh_config(text: &str) -> Vec<SshConfigHost> {
    let mut out: Vec<SshConfigHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        let val = val.trim_matches('"');
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in val.split_whitespace() {
                    if alias.contains('*') || alias.contains('?') || alias.starts_with('!') {
                        continue;
                    }
                    out.push(SshConfigHost {
                        name: alias.to_string(),
                        host: alias.to_string(),
                        port: 22,
                        ..Default::default()
                    });
                    current.push(out.len() - 1);
                }
            }
            "match" => current.clear(),
            "hostname" => current.iter().for_each(|&i| out[i].host = val.to_string()),
            "port" => {
                if let Ok(p) = val.parse() {
                    current.iter().for_each(|&i| out[i].port = p)
                }
            }
            "user" => current.iter().for_each(|&i| out[i].user = Some(val.to_string())),
            "identityfile" => current.iter().for_each(|&i| {
                if out[i].key_path.is_none() {
                    out[i].key_path = Some(expand_tilde(val))
                }
            }),
            _ => {}
        }
    }
    out
}

#[tauri::command]
pub fn ssh_config_import() -> Result<Vec<SshConfigHost>, String> {
    let path = dirs::home_dir().ok_or("no home dir")?.join(".ssh").join("config");
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse_ssh_config(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts() {
        let cfg = "Host *\n  User x\nHost prod web\n  HostName 10.0.0.5\n  Port 2222\n  User root\n  IdentityFile ~/.ssh/id_ed25519\n# c\nHost=dev\n  HostName=dev.example.com\n";
        let h = parse_ssh_config(cfg);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].name, "prod");
        assert_eq!(h[1].host, "10.0.0.5");
        assert_eq!(h[1].port, 2222);
        assert_eq!(h[0].user.as_deref(), Some("root"));
        assert!(h[0].key_path.as_ref().unwrap().ends_with(".ssh/id_ed25519"));
        assert_eq!(h[2].host, "dev.example.com");
    }
}
