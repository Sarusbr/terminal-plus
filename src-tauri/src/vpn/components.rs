//! Optional VPN components (openconnect, ocproxy, …). Not shipped with the app: downloaded
//! on first use from this repo's `vpn-components` release, verified (sha256) and unpacked
//! under the app data dir. A binary already on the system PATH (e.g. Homebrew) also works.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub const MANIFEST_URL: &str =
    "https://github.com/Sarusbr/terminal-plus/releases/download/vpn-components/manifest.json";

/// Platform key used in the manifest, e.g. "macos-aarch64", "windows-x86_64".
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Deserialize, Debug, Clone)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub version: String,
}

#[derive(Deserialize, Debug)]
struct Manifest {
    /// component → platform → asset
    components: std::collections::HashMap<String, std::collections::HashMap<String, Asset>>,
}

fn base_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("components"))
}

/// Executable name inside a component, per platform.
fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Directories searched for system-installed tools.
fn system_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin", "/usr/bin", "/usr/sbin"] {
        v.push(PathBuf::from(d));
    }
    v
}

/// Find a tool: downloaded component first, then the system.
pub fn find<R: Runtime>(app: &AppHandle<R>, component: &str, tool: &str) -> Option<PathBuf> {
    if let Ok(dir) = base_dir(app) {
        let p = dir.join(component).join("bin").join(exe(tool));
        if p.is_file() {
            return Some(p);
        }
    }
    system_dirs().into_iter().map(|d| d.join(exe(tool))).find(|p| p.is_file())
}

/// Component-provided support file (e.g. vpnc-script), falling back to known system paths.
pub fn support_file<R: Runtime>(app: &AppHandle<R>, component: &str, rel: &str, system: &[&str]) -> Option<PathBuf> {
    if let Ok(dir) = base_dir(app) {
        let p = dir.join(component).join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    system.iter().map(PathBuf::from).find(|p| p.is_file())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    /// "downloaded" | "system" | "missing"
    source: &'static str,
    path: Option<String>,
    version: Option<String>,
}

#[tauri::command]
pub fn vpn_component_status<R: Runtime>(app: AppHandle<R>, component: String, tool: String) -> ComponentStatus {
    let downloaded = base_dir(&app).ok().map(|d| d.join(&component));
    let version = downloaded.as_ref().and_then(|d| std::fs::read_to_string(d.join("VERSION")).ok()).map(|s| s.trim().to_string());
    match find(&app, &component, &tool) {
        Some(p) => ComponentStatus {
            source: if downloaded.map(|d| p.starts_with(d)).unwrap_or(false) { "downloaded" } else { "system" },
            path: Some(p.to_string_lossy().into_owned()),
            version,
        },
        None => ComponentStatus { source: "missing", path: None, version: None },
    }
}

fn http_get(url: &str) -> Result<ureq::Response, String> {
    ureq::get(url)
        .set("User-Agent", "TerminalPlus")
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "NOT_AVAILABLE".to_string(),
            e => e.to_string(),
        })
}

/// Download + verify + unpack a component. Emits `vpn-download` {component, done, total}.
#[tauri::command]
pub async fn vpn_component_install<R: Runtime>(app: AppHandle<R>, component: String) -> Result<(), String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || install_blocking(&app2, &component))
        .await
        .map_err(|e| e.to_string())?
}

fn install_blocking<R: Runtime>(app: &AppHandle<R>, component: &str) -> Result<(), String> {
    let body = http_get(MANIFEST_URL)?.into_string().map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let key = platform_key();
    let asset = manifest
        .components
        .get(component)
        .and_then(|m| m.get(&key))
        .cloned()
        .ok_or_else(|| format!("NOT_AVAILABLE: {component} ({key})"))?;

    let mut reader = http_get(&asset.url)?.into_reader();
    let mut buf = Vec::with_capacity(asset.size as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut last = std::time::Instant::now();
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if last.elapsed().as_millis() > 100 {
            last = std::time::Instant::now();
            let _ = app.emit("vpn-download", serde_json::json!({ "component": component, "done": buf.len(), "total": asset.size }));
        }
    }
    let digest = hex(&Sha256::digest(&buf));
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        return Err("Doğrulama başarısız (sha256 uyuşmuyor)".into());
    }
    let dest = base_dir(app)?.join(component);
    let tmp = dest.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    unzip(&buf, &tmp)?;
    std::fs::write(tmp.join("VERSION"), &asset.version).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    let _ = app.emit("vpn-download", serde_json::json!({ "component": component, "done": asset.size, "total": asset.size, "finished": true }));
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unzip(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue }; // rejects ../ paths
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755 | 0o600));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn vpn_component_remove<R: Runtime>(app: AppHandle<R>, component: String) -> Result<(), String> {
    let d = base_dir(&app)?.join(component);
    if d.exists() {
        std::fs::remove_dir_all(d).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unzip_keeps_modes_and_rejects_traversal() {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let exe = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            z.start_file("bin/openconnect", exe).unwrap();
            z.write_all(b"#!/bin/sh\necho ok\n").unwrap();
            z.start_file("../evil", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("tp_unzip_{}", uuid::Uuid::new_v4().simple()));
        unzip(&buf, &dir.join("c")).unwrap();
        let f = dir.join("c/bin/openconnect");
        assert!(f.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o111, 0o111);
        }
        assert!(!dir.join("evil").exists());
        assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_dir_all(dir);
    }
}
