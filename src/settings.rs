//! Sauvegarde des réglages dans %APPDATA%\micro-studio\settings.json.

use crate::params::Params;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub params: Params,
    pub user_presets: Vec<(String, Params)>,
    pub calibrated: bool,
}

/// Emplacement standard du fichier de réglages (`%APPDATA%` sous Windows).
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("micro-studio").join("settings.json"))
}

impl Settings {
    /// Charge depuis l'emplacement standard ; réglages par défaut si absent ou illisible.
    pub fn load() -> Self {
        default_path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    /// Charge un fichier ; réglages par défaut si absent ou illisible (un fichier
    /// corrompu ne doit jamais empêcher le logiciel de démarrer).
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Sauvegarde de façon atomique : écriture dans un fichier temporaire puis
    /// renommage, pour ne jamais laisser un fichier à moitié écrit en cas de coupure.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("micro-studio-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sous-dossier").join("settings.json");
        let mut s = Settings { input_device: Some("Micro USB".into()), calibrated: true, ..Settings::default() };
        s.params.input_gain_db = 9.0;
        s.user_presets.push(("Mes réglages 1".into(), s.params.clone()));
        s.save_to(&path).expect("sauvegarde");
        let back = Settings::load_from(&path);
        assert_eq!(back.input_device.as_deref(), Some("Micro USB"));
        assert!(back.calibrated);
        assert_eq!(back.params, s.params);
        assert_eq!(back.user_presets.len(), 1);
        assert!(!path.with_extension("json.tmp").exists(), "pas de fichier temporaire oublié");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_corrupt_file_gives_defaults() {
        let dir = temp_dir("corrupt");
        assert!(!Settings::load_from(&dir.join("absent.json")).calibrated);
        std::fs::create_dir_all(&dir).expect("dossier");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ ceci n'est pas du JSON").expect("écriture");
        let s = Settings::load_from(&path);
        assert_eq!(s.params, Params::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_reports_errors() {
        let dir = temp_dir("error");
        std::fs::create_dir_all(&dir).expect("dossier");
        // Un dossier à la place du fichier : l'écriture doit échouer proprement.
        let path = dir.join("settings.json");
        std::fs::create_dir_all(path.with_extension("json.tmp")).expect("dossier piège");
        assert!(Settings::default().save_to(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn default_path_ends_with_app_folder() {
        if let Some(p) = default_path() {
            assert!(p.ends_with(Path::new("micro-studio").join("settings.json")));
        }
    }
}
