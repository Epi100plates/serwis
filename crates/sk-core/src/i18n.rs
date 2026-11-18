use std::collections::BTreeMap;

use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};

/// Wszystkie tłumaczenia są wkompilowane w binarkę — narzędzie działa offline
/// i nie potrzebuje plików obok `.exe`. Jeden plik JSON na rodzinę modułów,
/// żeby dwa moduły pisane równolegle nie edytowały tego samego pliku.
static LOCALES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/locales");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    Pl,
    En,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::Pl, Locale::En];

    pub fn code(self) -> &'static str {
        match self {
            Self::Pl => "pl",
            Self::En => "en",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "pl" => Some(Self::Pl),
            "en" => Some(Self::En),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum I18nError {
    #[error("brak katalogu tłumaczeń: {0}")]
    MissingLocale(&'static str),
    #[error("niepoprawny plik tłumaczeń {file}: {message}")]
    Parse { file: String, message: String },
    #[error("zduplikowany klucz tłumaczenia {key} w pliku {file}")]
    DuplicateKey { key: String, file: String },
}

#[derive(Debug, Clone)]
pub struct Translator {
    tables: BTreeMap<Locale, BTreeMap<String, String>>,
    locale: Locale,
}

impl Translator {
    /// Wczytuje i scala wszystkie pliki JSON z `locales/<kod języka>/`.
    pub fn load() -> Result<Self, I18nError> {
        let mut tables = BTreeMap::new();
        for locale in Locale::ALL {
            let dir = LOCALES
                .get_dir(locale.code())
                .ok_or(I18nError::MissingLocale(locale.code()))?;
            let mut table: BTreeMap<String, String> = BTreeMap::new();
            for file in dir.files() {
                let path = file.path().display().to_string();
                if file.path().extension().is_some_and(|ext| ext == "json") {
                    let text = file.contents_utf8().ok_or_else(|| I18nError::Parse {
                        file: path.clone(),
                        message: "plik nie jest poprawnym UTF-8".to_string(),
                    })?;
                    let parsed: BTreeMap<String, String> =
                        serde_json::from_str(text).map_err(|err| I18nError::Parse {
                            file: path.clone(),
                            message: err.to_string(),
                        })?;
                    for (key, value) in parsed {
                        if table.insert(key.clone(), value).is_some() {
                            return Err(I18nError::DuplicateKey { key, file: path });
                        }
                    }
                }
            }
            tables.insert(locale, table);
        }
        Ok(Self {
            tables,
            locale: Locale::Pl,
        })
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    /// Tłumaczenie klucza. Brakujący klucz zwraca sam klucz — widoczny błąd
    /// zamiast cichego pustego tekstu; test kontraktu nie pozwala mu przejść CI.
    pub fn t(&self, key: &str) -> String {
        self.tables
            .get(&self.locale)
            .and_then(|table| table.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    pub fn has(&self, locale: Locale, key: &str) -> bool {
        self.tables
            .get(&locale)
            .is_some_and(|table| table.contains_key(key))
    }

    /// Klucze dostępne dla języka — do testu kompletności `pl` vs `en`.
    pub fn keys(&self, locale: Locale) -> Vec<&str> {
        self.tables
            .get(&locale)
            .map(|table| table.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_both_locales() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        for locale in Locale::ALL {
            assert!(
                !translator.keys(locale).is_empty(),
                "pusty język {}",
                locale.code()
            );
        }
    }

    #[test]
    fn poland_and_english_have_identical_key_sets() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let pl = translator.keys(Locale::Pl);
        let en = translator.keys(Locale::En);
        let missing_in_en: Vec<&&str> = pl.iter().filter(|key| !en.contains(key)).collect();
        let missing_in_pl: Vec<&&str> = en.iter().filter(|key| !pl.contains(key)).collect();
        assert!(missing_in_en.is_empty(), "brak w EN: {missing_in_en:?}");
        assert!(missing_in_pl.is_empty(), "brak w PL: {missing_in_pl:?}");
    }

    #[test]
    fn missing_key_falls_back_to_key() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        assert_eq!(
            translator.t("nie.ma.takiego.klucza"),
            "nie.ma.takiego.klucza"
        );
    }

    #[test]
    fn translates_common_keys() {
        let mut translator = Translator::load().expect("tłumaczenia się wczytują");
        translator.set_locale(Locale::Pl);
        assert_eq!(translator.t("common.capability.elevated"), "Administrator");
        translator.set_locale(Locale::En);
        assert_eq!(translator.t("common.capability.elevated"), "Administrator");
    }
}
