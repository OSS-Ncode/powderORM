//! `powder.config.json` — the project configuration `powder setup` writes
//! (selected languages, database URL, optional AI endpoint). Loosely typed
//! JSON so unknown keys round-trip.

use std::path::Path;

use serde_json::{json, Value as J};

pub const CONFIG_FILE: &str = "powder.config.json";

pub fn load_config(cwd: &Path) -> Result<J, String> {
    let path = cwd.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("invalid {CONFIG_FILE}: {e}"))
}

pub fn save_config(cwd: &Path, doc: &J) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    std::fs::write(cwd.join(CONFIG_FILE), text + "\n").map_err(|e| e.to_string())
}
