//! Настройки программы в `settings.json` (папка из `net::config_dir`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Код языка интерфейса; пусто — язык системы.
    pub language: String,
    pub auto_update: bool,
    /// Время последней проверки обновлений (секунды Unix).
    pub last_update_check: u64,
    /// Версия, которую пользователь попросил не предлагать.
    pub skipped_version: String,
    pub threshold: f32,
    pub dilate: i32,
    pub feather: i32,
    /// Ширина сохранённой панорамы; 0 — исходная.
    pub export_width: u32,
    /// Качество JPEG при сохранении.
    pub export_quality: u8,
    /// Панорама или кубическая карта.
    pub export_format: crate::export::Format,
}

/// Ниже этого качества JPEG панорама заметно портится.
pub const MIN_QUALITY: u8 = 50;

impl Default for Settings {
    fn default() -> Self {
        let p = crate::processor::Params::default();
        Settings {
            language: String::new(),
            auto_update: true,
            last_update_check: 0,
            skipped_version: String::new(),
            threshold: p.threshold,
            dilate: p.dilate,
            feather: p.feather,
            export_width: 0,
            export_quality: 85,
            export_format: Default::default(),
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
        // незнакомый формат — панорама, остальные настройки не теряются
        let s2 = Settings::from_json(r#"{"exportFormat":"sphere","exportQuality":70}"#);
        assert_eq!((s2.export_format, s2.export_quality), (crate::export::Format::Equirect, 70));
        let s3 = Settings::from_json(r#"{"exportFormat":"cross"}"#);
        assert_eq!(s3.export_format, crate::export::Format::CubeCross);
        let back = Settings::from_json(&serde_json::to_string(&s).unwrap());
        assert_eq!(back, s);
    }
}
