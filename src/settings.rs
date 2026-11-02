//! Sauvegarde des réglages dans %APPDATA%\micro-studio\settings.json.

use crate::params::Params;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub params: Params,
    pub user_presets: Vec<(String, Params)>,
    pub calibrated: bool,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("micro-studio").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}
