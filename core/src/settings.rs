use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub dev_mode: u8,
    #[serde(default)]
    pub skip_warning: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { dev_mode: 0, skip_warning: false }
    }
}

impl Settings {
    pub fn settings_path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("settings.json");
            }
        }
        PathBuf::from("settings.json")
    }

    pub fn load_or_create() -> Self {
        let path = Self::settings_path();
        match Self::load_from(&path) {
            Some(s) => s,
            None => {
                let s = Self::default();
                let _ = s.save_to(&path);
                s
            }
        }
    }

    fn load_from(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Self>(&data).ok()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::settings_path())
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .unwrap_or_else(|_| "{\n  \"dev_mode\": 0,\n  \"skip_warning\": false\n}".to_string());
        std::fs::write(path, json)
    }

    pub fn is_dev_mode(&self) -> bool {
        self.dev_mode == 1
    }
}
