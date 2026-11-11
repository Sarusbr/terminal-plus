//! OS keychain access. Secrets never touch disk in plain text and are never logged.

use keyring::Entry;

const SERVICE: &str = "terminal-plus";

fn entry(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|e| e.to_string())
}

pub fn get(account: &str) -> Option<String> {
    entry(account).ok()?.get_password().ok()
}

pub fn set(account: &str, secret: &str) -> Result<(), String> {
    entry(account)?.set_password(secret).map_err(|e| e.to_string())
}

pub fn delete(account: &str) -> Result<(), String> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Keychain account name for a server secret. `kind` is "pw" (password) or "pp" (key passphrase).
pub fn account(kind: &str, server_id: &str) -> String {
    format!("{kind}:{server_id}")
}

#[tauri::command]
pub fn secret_set(kind: String, server_id: String, secret: String) -> Result<(), String> {
    set(&account(&kind, &server_id), &secret)
}

#[tauri::command]
pub fn secret_delete(kind: String, server_id: String) -> Result<(), String> {
    delete(&account(&kind, &server_id))
}

#[tauri::command]
pub fn secret_exists(kind: String, server_id: String) -> bool {
    get(&account(&kind, &server_id)).is_some()
}
