//! Настройки программы в `settings.json` (папка из `net::config_dir`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub auto_update: bool,
    /// Время последней проверки обновлений (секунды Unix).
    pub last_update_check: u64,
    /// Версия, которую пользователь попросил не предлагать.
    pub skipped_version: String,
    pub threshold: f32,
    pub dilate: i32,
    pub feather: i32,
}

impl Default for Settings {
    fn default() -> Self {
        let p = crate::processor::Params::default();
        Settings {
            auto_update: true,
            last_update_check: 0,
            skipped_version: String::new(),
            threshold: p.threshold,
            dilate: p.dilate,
            feather: p.feather,
        }
    }
}

fn file() -> std::path::PathBuf {
    crate::net::config_dir().join("settings.json")
}

impl Settings {
    pub fn from_json(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn load() -> Self {
        std::fs::read_to_string(file()).map(|t| Self::from_json(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unknown_fields() {
        let s = Settings::from_json(r#"{"autoUpdate":false,"skippedVersion":"0.4.0","x":1}"#);
        assert!(!s.auto_update);
        assert_eq!(s.skipped_version, "0.4.0");
        assert_eq!(s.dilate, Settings::default().dilate);
        assert_eq!(Settings::from_json("not json"), Settings::default());
        let back = Settings::from_json(&serde_json::to_string(&s).unwrap());
        assert_eq!(back, s);
    }
}
