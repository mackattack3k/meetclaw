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
    pub language: Option<String>,
    pub audio_source: Option<String>,
    pub camera: Option<bool>,
    // Agent: persisted allow-rules (e.g. "web_search", "run_command:gh"),
    // whether auto mode (run everything without prompting) is on, and the
    // working directory the agent runs shell commands in.
    pub agent_allow_rules: Option<Vec<String>>,
    pub agent_auto: Option<bool>,
    pub agent_workspace: Option<String>,
}

/// Allow-rules, seeding the read-only `web_search` auto-allow on first use.
pub fn agent_allow_rules(app: &AppHandle) -> Vec<String> {
    load(app)
        .agent_allow_rules
        .unwrap_or_else(|| vec!["web_search".to_string()])
}

/// The directory the agent runs shell commands in: the configured workspace, or
/// `<app data dir>/agent-workspace`. Created if missing.
pub fn agent_workspace(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = match load(app).agent_workspace.filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("no app data dir: {e}"))?
            .join("agent-workspace"),
    };
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create agent workspace: {e}"))?;
    Ok(dir)
}

fn agent_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no app config dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create config dir: {e}"))?;
    Ok(dir.join("MEETCLAW.md"))
}

const DEFAULT_AGENT_CONFIG: &str = "# MEETCLAW.md\n\n\
Instructions for the in-meeting assistant. Edit freely.\n\n\
- Be concise. Propose an action only when it clearly helps.\n\
- Prefer read-only commands; explain destructive ones before proposing them.\n\n\
## Context\n\n\
(Describe your projects, tools, and preferences here.)\n";

/// The user-editable `MEETCLAW.md` that steers the agent. Falls back to a
/// starter template when the file doesn't exist yet.
pub fn read_agent_config(app: &AppHandle) -> String {
    agent_config_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_else(|| DEFAULT_AGENT_CONFIG.to_string())
}

pub fn write_agent_config(app: &AppHandle, content: &str) -> Result<(), String> {
    let path = agent_config_path(app)?;
    fs::write(path, content).map_err(|e| format!("write MEETCLAW.md: {e}"))
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
