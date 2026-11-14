//! Язык интерфейса. Тексты окна — в `ui/index.html` (словарь `I18N`); здесь — выбор языка и несколько строк для
//! системных диалогов, которые открывает Rust.

/// Поддерживаемые языки по порядку в меню. Первый — запасной.
pub const LANGUAGES: [&str; 8] = ["en", "ru", "es", "pt", "de", "fr", "zh", "ja"];

/// Язык из тега вида `pt-BR`, `zh-Hans-CN`, `ru_RU`; None — не поддерживается.
pub fn supported(tag: &str) -> Option<&'static str> {
    let base = tag.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase();
    LANGUAGES.iter().copied().find(|l| *l == base)
}

/// Первый поддерживаемый язык из списка предпочтений системы, иначе английский.
pub fn system_language() -> &'static str {
    sys_locale::get_locales().find_map(|l| supported(&l)).unwrap_or("en")
}

/// Язык из настройки: пусто или неизвестный код — язык системы.
pub fn resolve(setting: &str) -> &'static str {
    supported(setting).unwrap_or_else(system_language)
}

#[derive(Clone, Copy)]
pub enum Text {
    PickFirst,
    PickSecond,
    Photos,
    SaveTitle,
}

pub fn tr(lang: &str, text: Text) -> &'static str {
    use Text::*;
    match (lang, text) {
        ("ru", PickFirst) => "Выберите первое 360° фото",
        ("ru", PickSecond) => "Выберите второе 360° фото",
        ("ru", Photos) => "360° фото",
        ("ru", SaveTitle) => "Сохранить чистую 360° панораму",
        ("es", PickFirst) => "Elige la primera foto 360°",
        ("es", PickSecond) => "Elige la segunda foto 360°",
        ("es", Photos) => "Fotos 360°",
        ("es", SaveTitle) => "Guardar la panorámica 360° limpia",
        ("pt", PickFirst) => "Escolha a primeira foto 360°",
        ("pt", PickSecond) => "Escolha a segunda foto 360°",
        ("pt", Photos) => "Fotos 360°",
        ("pt", SaveTitle) => "Salvar o panorama 360° limpo",
        ("de", PickFirst) => "Erstes 360°-Foto auswählen",
        ("de", PickSecond) => "Zweites 360°-Foto auswählen",
        ("de", Photos) => "360°-Fotos",
        ("de", SaveTitle) => "Bereinigtes 360°-Panorama speichern",
        ("fr", PickFirst) => "Choisissez la première photo 360°",
        ("fr", PickSecond) => "Choisissez la deuxième photo 360°",
        ("fr", Photos) => "Photos 360°",
        ("fr", SaveTitle) => "Enregistrer le panorama 360° nettoyé",
        ("zh", PickFirst) => "选择第一张 360° 照片",
        ("zh", PickSecond) => "选择第二张 360° 照片",
        ("zh", Photos) => "360° 照片",
        ("zh", SaveTitle) => "保存清理后的 360° 全景图",
        ("ja", PickFirst) => "1 枚目の 360° 写真を選択",
        ("ja", PickSecond) => "2 枚目の 360° 写真を選択",
        ("ja", Photos) => "360° 写真",
        ("ja", SaveTitle) => "人物を消した 360° パノラマを保存",
        (_, PickFirst) => "Choose the first 360° photo",
        (_, PickSecond) => "Choose the second 360° photo",
        (_, Photos) => "360° photos",
        (_, SaveTitle) => "Save the clean 360° panorama",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        assert_eq!(supported("pt-BR"), Some("pt"));
        assert_eq!(supported("zh-Hans-CN"), Some("zh"));
        assert_eq!(supported("ru_RU.UTF-8"), Some("ru"));
        assert_eq!(supported("EN"), Some("en"));
        assert_eq!(supported("ko-KR"), None);
        assert_eq!(supported(""), None);
        assert_eq!(resolve("de"), "de");
        assert!(LANGUAGES.contains(&resolve("")));
        assert!(LANGUAGES.contains(&resolve("xx")));
    }

    #[test]
    fn every_language_has_texts() {
        for l in LANGUAGES {
            for t in [Text::PickFirst, Text::PickSecond, Text::Photos, Text::SaveTitle] {
                assert!(!tr(l, t).is_empty());
            }
        }
        assert_ne!(tr("ja", Text::Photos), tr("en", Text::Photos));
    }
}
