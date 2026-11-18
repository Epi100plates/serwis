use serde::{Deserialize, Serialize};

/// Deklaratywny opis tego, co moduł chce pokazać. Moduł nie zna egui — ten
/// model renderuje generyczny widok w `sk-app`, a testy porównują go jako JSON.
///
/// `Eq` nie jest implementowane: punkty wykresów to `f64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
                    for row in rows {
                        out.push(row.label_key.clone());
                        if let Some(value_key) = &row.value_key {
                            out.push(value_key.clone());
                        }
                    }
                }
                Block::Table { table } => out.extend(table.headers.iter().cloned()),
                Block::Note { text_key, .. } | Block::Badge { text_key } => {
                    out.push(text_key.clone());
                }
                // Etykiety serii są danymi („CPU 1”), nie tekstem interfejsu.
                Block::Chart { chart } => out.push(chart.title_key.clone()),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Wykres liniowy — telemetria w czasie (Faza 1: `system.live`).
    Chart { chart: Chart },
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

    pub fn chart(chart: Chart) -> Self {
        Self::Chart { chart }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub label_key: String,
    pub value: String,
    /// Zamiast surowej wartości: klucz i18n (np. „Tak”/„Nie”). Ma pierwszeństwo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_key: Option<String>,
}

impl KeyValue {
    pub fn new(label_key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label_key: label_key.into(),
            value: value.into(),
            value_key: None,
        }
    }

    /// Wartość będąca kluczem i18n — dla stanów logicznych (Tak/Nie/Brak danych).
    pub fn state(label_key: impl Into<String>, value_key: impl Into<String>) -> Self {
        Self {
            label_key: label_key.into(),
            value: String::new(),
            value_key: Some(value_key.into()),
        }
    }
}

/// Wykres liniowy. Osie opisane są jednostkami („s”, „%”, „MHz”) — to dane,
/// nie proza, dlatego nie są kluczami i18n.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Chart {
    pub title_key: String,
    pub x_label: String,
    pub y_label: String,
    pub series: Vec<ChartSeries>,
}

impl Chart {
    pub fn new(
        title_key: impl Into<String>,
        x_label: impl Into<String>,
        y_label: impl Into<String>,
    ) -> Self {
        Self {
            title_key: title_key.into(),
            x_label: x_label.into(),
            y_label: y_label.into(),
            series: Vec::new(),
        }
    }

    pub fn with_series(mut self, series: Vec<ChartSeries>) -> Self {
        self.series = series;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.series.iter().all(|series| series.points.is_empty())
    }
}

/// Jedna linia wykresu. `label` jest daną („CPU”, „RAM”), nie kluczem i18n.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartSeries {
    pub label: String,
    pub points: Vec<[f64; 2]>,
}

impl ChartSeries {
    pub fn new(label: impl Into<String>, points: Vec<[f64; 2]>) -> Self {
        Self {
            label: label.into(),
            points,
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

        let chart =
            Block::chart(
                Chart::new("test.chart", "s", "%").with_series(vec![ChartSeries::new(
                    "CPU",
                    vec![[0.0, 12.5], [1.0, 20.0]],
                )]),
            );
        let json = serde_json::to_value(chart).expect("json");
        assert_eq!(json["kind"], "chart");
        assert_eq!(json["chart"]["series"][0]["points"][1][1], 20.0);
    }

    #[test]
    fn chart_keeps_only_title_as_translation_key() {
        let view = View::new(vec![Section::new("test.chart_section").with_blocks(vec![
            Block::chart(Chart::new("test.chart", "s", "%").with_series(vec![
                ChartSeries::new("CPU", vec![[0.0, 1.0]]),
            ])),
        ])]);
        let keys = view.translation_keys();
        assert!(keys.iter().any(|key| key == "test.chart"));
        assert!(!keys.iter().any(|key| key == "CPU"), "{keys:?}");
    }

    #[test]
    fn value_key_counts_as_translation_key_and_wins_in_json() {
        let row = KeyValue::state("test.label", "common.value.yes");
        let view = View::new(vec![
            Section::new("test.section").with_blocks(vec![Block::key_values(vec![row])]),
        ]);
        assert!(view.uses_key("test.label"));
        assert!(view.uses_key("common.value.yes"));

        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(
            json["sections"][0]["blocks"][0]["rows"][0]["value_key"],
            "common.value.yes"
        );
    }

    #[test]
    fn empty_chart_is_reported() {
        assert!(Chart::new("test.chart", "s", "%").is_empty());
        assert!(
            !Chart::new("test.chart", "s", "%")
                .with_series(vec![ChartSeries::new("CPU", vec![[0.0, 1.0]])])
                .is_empty()
        );
    }
}
