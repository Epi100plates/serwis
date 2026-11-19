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
                Block::Table { table } => {
                    out.extend(table.headers.iter().cloned());
                    for cells in &table.value_keys {
                        out.extend(cells.iter().flatten().cloned());
                    }
                }
                Block::Note { text_key, .. } | Block::Badge { text_key } => {
                    out.push(text_key.clone());
                }
                Block::Status {
                    label_key,
                    value_key,
                    ..
                } => {
                    out.push(label_key.clone());
                    out.push(value_key.clone());
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
    /// Duży, kolorowany stan sekcji — np. „Stan dysku: Dobry”. Ma być pierwszą
    /// rzeczą, którą widzi technik, dlatego nie jest zwykłym wierszem tabeli.
    Status {
        label_key: String,
        value_key: String,
        level: StatusLevel,
    },
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

    /// Stan sekcji: etykieta („Stan dysku”), wartość jako klucz i18n („Dobry”)
    /// i poziom, który decyduje o kolorze w GUI i w raporcie HTML.
    pub fn status(
        label_key: impl Into<String>,
        value_key: impl Into<String>,
        level: StatusLevel,
    ) -> Self {
        Self::Status {
            label_key: label_key.into(),
            value_key: value_key.into(),
            level,
        }
    }

    pub fn chart(chart: Chart) -> Self {
        Self::Chart { chart }
    }
}

/// Poziom stanu do wyróżnienia w widoku. To wyłącznie warstwa prezentacji —
/// moduł sam decyduje, kiedy stan jest ostrzeżeniem, a kiedy awarią (np.
/// `storage.smart` liczy go z progów SMART i liczników błędów).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StatusLevel {
    /// Wszystko w porządku.
    Ok,
    /// Odchylenie: zużycie, liczniki błędów, obniżony zapas.
    Warn,
    /// Stan przedawaryjny lub awaria.
    Bad,
    /// Brak danych do oceny — nie zgadujemy.
    #[default]
    Unknown,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_zero: bool,
    /// Stały początek osi X; `None` = oś zaczyna się od danych.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_min: Option<f64>,
    /// Stały koniec osi X. Ustawiony z góry (np. rozmiar dysku w GB), więc
    /// wykres rysuje się w gotowym układzie, zanim pojawi się choćby jedna
    /// próbka — zamiast rozciągać się z każdym nowym punktem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_max: Option<f64>,
    pub series: Vec<ChartSeries>,
}

fn is_false(value: &bool) -> bool {
    !*value
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
            from_zero: false,
            x_min: None,
            x_max: None,
            series: Vec::new(),
        }
    }

    /// Skala osi Y zaczyna się od zera. Dla przepustowości to konieczne —
    /// bez tego 2 % różnicy między blokami rysuje się jak góra i dolina.
    pub fn with_zero_base(mut self) -> Self {
        self.from_zero = true;
        self
    }

    /// Ustawia stały zakres osi X. Wykres ma wtedy gotowy układ od pierwszego
    /// odświeżenia: oś pokazuje całą szerokość (np. cały dysk), a punkty
    /// dochodzą na swoje miejsce.
    pub fn with_x_range(mut self, min: f64, max: f64) -> Self {
        if min.is_finite() && max.is_finite() && max > min {
            self.x_min = Some(min);
            self.x_max = Some(max);
        }
        self
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
    /// Klucze i18n dla pojedynczych komórek — dla stanów („Cyfrowe”, „Tak”).
    /// Puste, gdy wszystkie komórki to dane; wtedy nie zaśmieca JSON-u.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_keys: Vec<Vec<Option<String>>>,
    /// Poziom stanu pojedynczych komórek — renderer koloruje wtedy wartość.
    /// Puste, gdy tabela nie ma wyróżnień; wtedy nie zaśmieca JSON-u.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_levels: Vec<Vec<Option<StatusLevel>>>,
}

impl Table {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self {
            headers,
            rows,
            value_keys: Vec::new(),
            cell_levels: Vec::new(),
        }
    }

    /// Oznacza komórkę `(row, column)` kluczem i18n; wartość z `rows` jest wtedy
    /// pomijana przez renderer (tak samo jak `KeyValue::state`).
    pub fn with_value_key(mut self, row: usize, column: usize, key: impl Into<String>) -> Self {
        while self.value_keys.len() <= row {
            self.value_keys.push(Vec::new());
        }
        let cells = &mut self.value_keys[row];
        while cells.len() <= column {
            cells.push(None);
        }
        cells[column] = Some(key.into());
        self
    }

    /// Nadaje komórce poziom stanu (kolor wartości). Niezależne od `value_key`:
    /// stan może być daną („86 %”) i nadal być wyróżniony kolorem.
    pub fn with_cell_level(mut self, row: usize, column: usize, level: StatusLevel) -> Self {
        while self.cell_levels.len() <= row {
            self.cell_levels.push(Vec::new());
        }
        let cells = &mut self.cell_levels[row];
        while cells.len() <= column {
            cells.push(None);
        }
        cells[column] = Some(level);
        self
    }

    pub fn value_key(&self, row: usize, column: usize) -> Option<&str> {
        self.value_keys
            .get(row)
            .and_then(|cells| cells.get(column))
            .and_then(Option::as_deref)
    }

    pub fn cell_level(&self, row: usize, column: usize) -> Option<StatusLevel> {
        self.cell_levels
            .get(row)
            .and_then(|cells| cells.get(column))
            .copied()
            .flatten()
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
    fn table_value_keys_are_collected_as_translation_keys() {
        let table = Table::new(
            vec!["test.col".to_string()],
            vec![vec!["surowe".to_string()]],
        )
        .with_value_key(0, 0, "common.value.digital");
        let view = View::new(vec![Section::new("test.section").with_blocks(vec![
            Block::Table {
                table: table.clone(),
            },
        ])]);

        assert!(view.uses_key("test.section"));
        assert!(view.uses_key("test.col"));
        assert!(view.uses_key("common.value.digital"));
        assert_eq!(table.value_key(0, 0), Some("common.value.digital"));
        assert_eq!(table.value_key(1, 0), None);

        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(
            json["sections"][0]["blocks"][0]["table"]["value_keys"][0][0],
            "common.value.digital"
        );
    }

    #[test]
    fn empty_table_value_keys_are_not_serialized() {
        let table = Table::new(vec!["test.col".to_string()], vec![vec!["x".to_string()]]);
        let json = serde_json::to_value(table).expect("json");
        assert!(json.get("value_keys").is_none(), "{json}");
    }

    #[test]
    fn status_block_collects_both_keys_and_has_kind_tag() {
        let view = View::new(vec![Section::new("test.section").with_blocks(vec![
            Block::status("test.status.label", "test.status.value", StatusLevel::Bad),
        ])]);

        assert!(view.uses_key("test.status.label"));
        assert!(view.uses_key("test.status.value"));

        let json = serde_json::to_value(&view).expect("json");
        assert_eq!(json["sections"][0]["blocks"][0]["kind"], "status");
        assert_eq!(json["sections"][0]["blocks"][0]["level"], "bad");
        assert_eq!(
            json["sections"][0]["blocks"][0]["value_key"],
            "test.status.value"
        );
    }

    #[test]
    fn status_level_serializes_as_snake_case_and_defaults_to_unknown() {
        for (level, expected) in [
            (StatusLevel::Ok, "ok"),
            (StatusLevel::Warn, "warn"),
            (StatusLevel::Bad, "bad"),
            (StatusLevel::Unknown, "unknown"),
        ] {
            let json = serde_json::to_value(level).expect("json");
            assert_eq!(json, expected);
            let back: StatusLevel = serde_json::from_value(json).expect("odczyt poziomu");
            assert_eq!(back, level);
        }
        assert_eq!(StatusLevel::default(), StatusLevel::Unknown);
    }

    #[test]
    fn cell_levels_color_single_cells_and_stay_out_of_json_when_unused() {
        let table = Table::new(vec!["test.col".to_string()], vec![vec!["86 %".to_string()]])
            .with_cell_level(0, 0, StatusLevel::Warn);
        assert_eq!(table.cell_level(0, 0), Some(StatusLevel::Warn));
        assert_eq!(table.cell_level(0, 1), None);
        assert_eq!(table.cell_level(3, 0), None);

        let json = serde_json::to_value(&table).expect("json");
        assert_eq!(json["cell_levels"][0][0], "warn");

        let plain = Table::new(vec!["test.col".to_string()], vec![vec!["x".to_string()]]);
        let json = serde_json::to_value(&plain).expect("json");
        assert!(json.get("cell_levels").is_none(), "{json}");
    }

    #[test]
    fn zero_base_chart_serializes_flag_only_when_set() {
        let plain = Chart::new("test.chart", "%", "MB/s");
        assert!(!plain.from_zero);
        let json = serde_json::to_value(&plain).expect("json");
        assert!(json.get("from_zero").is_none(), "{json}");

        let chart = Chart::new("test.chart", "%", "MB/s").with_zero_base();
        assert!(chart.from_zero);
        let json = serde_json::to_value(&chart).expect("json");
        assert_eq!(json["from_zero"], true);
        let back: Chart = serde_json::from_value(json).expect("odczyt wykresu");
        assert!(back.from_zero);
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

    #[test]
    fn chart_x_range_is_serialized_only_when_valid() {
        let plain = Chart::new("test.chart", "GB", "MB/s");
        let json = serde_json::to_value(&plain).expect("json");
        assert!(json.get("x_min").is_none(), "{json}");
        assert!(json.get("x_max").is_none(), "{json}");

        let chart = Chart::new("test.chart", "GB", "MB/s").with_x_range(0.0, 931.5);
        let json = serde_json::to_value(&chart).expect("json");
        assert_eq!(json["x_min"], 0.0);
        assert_eq!(json["x_max"], 931.5);
        let back: Chart = serde_json::from_value(json).expect("odczyt wykresu");
        assert_eq!(back.x_max, Some(931.5));

        // Odwrócony albo nierozstrzygalny zakres nie może trafić do modelu —
        // renderer dostałby oś o ujemnej szerokości.
        let broken = Chart::new("test.chart", "GB", "MB/s").with_x_range(10.0, 10.0);
        assert_eq!(broken.x_min, None);
        assert_eq!(broken.x_max, None);
        let nan = Chart::new("test.chart", "GB", "MB/s").with_x_range(0.0, f64::NAN);
        assert_eq!(nan.x_max, None);
    }
}
