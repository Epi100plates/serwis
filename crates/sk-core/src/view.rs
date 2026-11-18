use serde::{Deserialize, Serialize};

/// Deklaratywny opis tego, co moduł chce pokazać. Moduł nie zna egui — ten
/// model renderuje generyczny widok w `sk-app`, a testy porównują go jako JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct View {
    pub sections: Vec<Section>,
}

impl View {
    pub fn new(sections: Vec<Section>) -> Self {
        Self { sections }
    }

    /// Czy widok używa danego klucza tłumaczenia (używane przez testy kontraktu).
    pub fn uses_key(&self, key: &str) -> bool {
        self.sections.iter().any(|section| section.uses_key(key))
    }

    /// Wszystkie klucze tłumaczeń użyte w widoku — do testu kompletności i18n.
    pub fn translation_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for section in &self.sections {
            section.collect_keys(&mut keys);
        }
        keys
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Section {
    pub title_key: String,
    pub subtitle: Option<String>,
    pub blocks: Vec<Block>,
}

impl Section {
    pub fn new(title_key: impl Into<String>) -> Self {
        Self {
            title_key: title_key.into(),
            subtitle: None,
            blocks: Vec::new(),
        }
    }

    pub fn with_blocks(mut self, blocks: Vec<Block>) -> Self {
        self.blocks = blocks;
        self
    }

    pub fn uses_key(&self, key: &str) -> bool {
        let mut keys = Vec::new();
        self.collect_keys(&mut keys);
        keys.iter().any(|candidate| candidate == key)
    }

    fn collect_keys(&self, out: &mut Vec<String>) {
        out.push(self.title_key.clone());
        for block in &self.blocks {
            match block {
                Block::KeyValues { rows } => {
                    out.extend(rows.iter().map(|row| row.label_key.clone()));
                }
                Block::Table { table } => out.extend(table.headers.iter().cloned()),
                Block::Note { text_key, .. } | Block::Badge { text_key } => {
                    out.push(text_key.clone());
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    /// Lista „etykieta → wartość”. Etykieta to klucz i18n, wartość to dana.
    KeyValues { rows: Vec<KeyValue> },
    /// Tabela z nagłówkami jako kluczami i18n.
    Table { table: Table },
    /// Komunikat: brak danych, brak uprawnień, degradacja.
    Note { level: NoteLevel, text_key: String },
    /// Znacznik stanu, np. „wymaga administratora”.
    Badge { text_key: String },
}

impl Block {
    pub fn key_values(rows: Vec<KeyValue>) -> Self {
        Self::KeyValues { rows }
    }

    pub fn note(level: NoteLevel, text_key: impl Into<String>) -> Self {
        Self::Note {
            level,
            text_key: text_key.into(),
        }
    }

    pub fn badge(text_key: impl Into<String>) -> Self {
        Self::Badge {
            text_key: text_key.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub label_key: String,
    pub value: String,
}

impl KeyValue {
    pub fn new(label_key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label_key: label_key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self { headers, rows }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteLevel {
    Info,
    Warn,
    Error,
}

/// Znacznik widoczny przy sekcji (np. „wymaga administratora”).
pub type Badge = String;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_view() -> View {
        View::new(vec![
            Section::new("test.section").with_blocks(vec![
                Block::key_values(vec![KeyValue::new("test.host", "PC-01")]),
                Block::note(NoteLevel::Warn, "test.warn"),
            ]),
            Section::new("test.other").with_blocks(vec![Block::badge("test.badge")]),
        ])
    }

    #[test]
    fn collects_keys_from_all_blocks() {
        let view = sample_view();
        let keys = view.translation_keys();
        for expected in [
            "test.section",
            "test.host",
            "test.warn",
            "test.other",
            "test.badge",
        ] {
            assert!(
                keys.iter().any(|key| key == expected),
                "brak {expected} w {keys:?}"
            );
        }
    }

    #[test]
    fn uses_key_finds_nested_label() {
        assert!(sample_view().uses_key("test.host"));
        assert!(!sample_view().uses_key("test.missing"));
    }

    #[test]
    fn view_roundtrips_through_json() {
        let view = sample_view();
        let json = serde_json::to_string(&view).expect("serializacja widoku");
        let back: View = serde_json::from_str(&json).expect("deserializacja widoku");
        assert_eq!(view, back);
    }

    #[test]
    fn block_json_has_kind_tag() {
        let json = serde_json::to_value(Block::key_values(vec![])).expect("json");
        assert_eq!(json["kind"], "key_values");
    }
}
