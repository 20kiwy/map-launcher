//! Small persisted settings (port + data folder) so the app remembers the
//! user's choices across runs. Stored in the per-user config dir — never next
//! to the exe, which may live somewhere read-only (Program Files).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
}

/// Per-user config dir for map-launcher (`%APPDATA%\map-launcher` on Windows,
/// `~/.config/map-launcher` elsewhere).
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join("map-launcher"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config").join("map-launcher"))
    }
}

fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("tile-server.json"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(dir) = config_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir.join("tile-server.json"), json);
        }
    }
}
