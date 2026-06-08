// Persisted preferences (app config dir / settings.json) plus the API key,
// which lives in the OS keychain rather than a plaintext file.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const KEYRING_SERVICE: &str = "meetclaw";
const KEYRING_ACCOUNT: &str = "gemini_api_key";

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    pub model: Option<String>,
    pub device: Option<String>,
    pub save_dir: Option<String>,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no app config dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create config dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

/// Mutate-and-persist helper.
pub fn update(app: &AppHandle, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let mut s = load(app);
    f(&mut s);
    save(app, &s)
}

/// The configured meetings directory, if set to a non-empty path.
pub fn save_dir(app: &AppHandle) -> Option<PathBuf> {
    load(app)
        .save_dir
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

// --- API key (OS keychain) ---

pub fn get_api_key() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    entry.get_password().ok().filter(|k| !k.trim().is_empty())
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| e.to_string())?;
    if key.trim().is_empty() {
        // Clearing the key; ignore "not found".
        let _ = entry.delete_credential();
        Ok(())
    } else {
        entry.set_password(key).map_err(|e| e.to_string())
    }
}

pub fn has_api_key() -> bool {
    get_api_key().is_some()
}
