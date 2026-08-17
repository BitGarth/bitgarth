/// Application locale (English-only for now; i18n will be re-added later).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Locale {
    #[default]
    English,
}

impl Locale {
    pub fn code(&self) -> &'static str {
        "en"
    }

    pub fn try_from_code(code: &str) -> Option<Locale> {
        match code.to_lowercase().as_str() {
            "en" => Some(Locale::English),
            _ => None,
        }
    }

    pub fn from_locale_string(_locale_str: &str) -> Locale {
        Locale::English
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_locale_code() {
        assert_eq!(Locale::English.code(), "en");
    }

    #[test]
    fn test_from_locale_string_always_returns_english() {
        assert_eq!(Locale::from_locale_string("en"), Locale::English);
        assert_eq!(Locale::from_locale_string("nl-NL"), Locale::English);
        assert_eq!(Locale::from_locale_string(""), Locale::English);
    }

    #[test]
    fn test_try_from_code() {
        assert_eq!(Locale::try_from_code("en"), Some(Locale::English));
        assert_eq!(Locale::try_from_code("nl"), None);
    }

    #[test]
    fn test_default_is_english() {
        assert_eq!(Locale::default(), Locale::English);
    }
}
