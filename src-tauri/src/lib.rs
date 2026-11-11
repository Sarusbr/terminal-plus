mod fsops;
mod local;
mod secrets;
mod ssh;
mod socks;
mod store;
mod vpn;
mod transfer;

/// Toggle native blur behind the (transparent) window.
#[tauri::command]
fn set_window_effect(window: tauri::WebviewWindow, enabled: bool, level: Option<String>) -> Result<(), String> {
    let level = level.unwrap_or_else(|| "medium".into());
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if enabled {
            let material = match level.as_str() {
                "light" => NSVisualEffectMaterial::UnderWindowBackground,
                "strong" => NSVisualEffectMaterial::FullScreenUI,
                _ => NSVisualEffectMaterial::HudWindow,
            };
            let _ = clear_vibrancy(&window);
            apply_vibrancy(&window, material, Some(NSVisualEffectState::Active), None)
                .map_err(|e| e.to_string())?;
        } else {
            let _ = clear_vibrancy(&window);
        }
    }
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_acrylic, apply_blur, apply_mica, clear_acrylic, clear_blur, clear_mica};
        let _ = clear_mica(&window);
        let _ = clear_acrylic(&window);
        let _ = clear_blur(&window);
        if enabled {
            // Mica: Windows 11 · Acrylic: Windows 10 1809+ / 11 · Blur: older Windows 10.
            // Try the preferred one for the level, fall back to whatever this Windows supports.
            let order: &[&str] = match level.as_str() {
                "light" => &["mica", "acrylic", "blur"],
                "strong" => &["acrylic", "blur", "mica"],
                _ => &["acrylic", "mica", "blur"],
            };
            let tint = if level == "strong" { 60 } else { 120 };
            let mut last = String::from("unsupported");
            for effect in order {
                let r = match *effect {
                    "mica" => apply_mica(&window, Some(true)),
                    "acrylic" => apply_acrylic(&window, Some((18, 18, 18, tint))),
                    _ => apply_blur(&window, Some((18, 18, 18, tint))),
                };
                match r {
                    Ok(()) => return Ok(()),
                    Err(e) => last = format!("{effect}: {e}"),
                }
            }
            return Err(last);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = (window, enabled, level);
    Ok(())
}

#[tauri::command]
fn platform() -> &'static str {
    std::env::consts::OS
}

/// One-time move of saved servers/settings from the pre-0.1 identifier's directory.
fn migrate_config(app: &tauri::App) {
    use tauri::Manager;
    let Ok(new_dir) = app.path().app_config_dir() else { return };
    let Some(old_dir) = new_dir.parent().map(|p| p.join("dev.terminalplus.app")) else { return };
    if !old_dir.is_dir() || old_dir == new_dir {
        return;
    }
    let _ = std::fs::create_dir_all(&new_dir);
    for f in ["servers.json", "settings.json"] {
        let (from, to) = (old_dir.join(f), new_dir.join(f));
        if from.is_file() && !to.exists() {
            let _ = std::fs::copy(&from, &to);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            migrate_config(app);
            Ok(())
        })
        .manage(ssh::SshState::default())
        .manage(transfer::TransferState::default())
        .manage(vpn::VpnState::default())
        .invoke_handler(tauri::generate_handler![
            set_window_effect,
            platform,
            vpn::vpn_connect,
            vpn::vpn_input,
            vpn::vpn_disconnect,
            vpn::vpn_list,
            vpn::vpn_log,
            vpn::vpn_socks_port,
            vpn::vpn_secret_exists,
            vpn::vpn_secret_reveal,
            vpn::vpn_secret_set,
            vpn::components::vpn_component_status,
            vpn::components::vpn_component_install,
            vpn::components::vpn_component_remove,
            transfer::transfer_start,
            transfer::transfer_cancel,
            fsops::file_read,
            fsops::file_write,
            fsops::temp_write,
            fsops::local_list_dir,
            fsops::fs_op,
            local::local_shells,
            local::local_exec,
            ssh::pty_busy,
            local::local_info,
            local::local_pty_open,
            ssh::ssh_connect,
            ssh::ssh_hostkey_respond,
            ssh::ssh_disconnect,
            ssh::pty_open,
            ssh::pty_write,
            ssh::pty_resize,
            ssh::pty_close,
            ssh::pty_write_saved_password,
            secrets::secret_set,
            secrets::secret_delete,
            secrets::secret_exists,
            store::store_load,
            store::store_save,
            store::ssh_config_import,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Terminal Plus")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                // Settings → "VPN: keep global connection when the app closes".
                let keep = app
                    .path()
                    .app_config_dir()
                    .ok()
                    .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("vpnKeepGlobal").and_then(|b| b.as_bool()))
                    .unwrap_or(false);
                if let Some(st) = app.try_state::<vpn::VpnState>() {
                    vpn::shutdown(&st, keep);
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
            }
        });
}
