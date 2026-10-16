//! User settings and the app's own data folder.
//!
//! Unlost writes very little of its own: this settings file (only when the user changes a
//! setting) and the WebView2 profile. In portable mode (a `portable` file next to the exe) both
//! live in `.\data` beside the exe, so running Unlost from a USB stick keeps its writes off the
//! disk being recovered. Installed, they live in `%LOCALAPPDATA%\Unlost`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `system`, `en` or `tr`.
    pub language: String,
    /// `system`, `light` or `dark`.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: "system".into(),
            theme: "system".into(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = portable_dir() {
        return dir;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("Unlost")
}

/// `<exe dir>\data` when a `portable` marker file sits next to the exe.
pub fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("portable").is_file().then(|| dir.join("data"))
}

pub fn is_portable() -> bool {
    portable_dir().is_some()
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

impl Settings {
    /// A missing or unreadable file gives defaults: these settings protect nothing.
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(settings_path(dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let path = settings_path(dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(tmp.path()), Settings::default());
        let s = Settings {
            language: "tr".into(),
            theme: "dark".into(),
        };
        s.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()), s);
        std::fs::write(tmp.path().join("settings.json"), b"{ broken").unwrap();
        assert_eq!(Settings::load(tmp.path()), Settings::default());
    }
}
