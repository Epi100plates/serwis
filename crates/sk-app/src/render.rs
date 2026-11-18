//! Generyczny renderer modelu widoku.
//!
//! Moduły nie znają egui — zwracają [`sk_core::view::View`]. Dzięki temu nowy
//! moduł nie dotyka tego pliku, a widok da się przetestować jako JSON.

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use sk_core::i18n::Translator;
use sk_core::view::{Block, Chart, NoteLevel, Table, View};

pub fn view(ui: &mut egui::Ui, view: &View, translator: &Translator) {
    if view.sections.is_empty() {
        ui.label(translator.t("ui.empty_section"));
        return;
    }

    for section in &view.sections {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(translator.t(&section.title_key)).strong());
        if let Some(subtitle) = &section.subtitle {
            ui.label(egui::RichText::new(subtitle).small().weak());
        }
        ui.separator();

        for block in &section.blocks {
            block_ui(ui, block, translator);
            ui.add_space(4.0);
        }
    }
}

fn block_ui(ui: &mut egui::Ui, block: &Block, translator: &Translator) {
    match block {
        Block::KeyValues { rows } => {
            egui::Grid::new(ui.next_auto_id())
                .num_columns(2)
                .spacing([24.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for row in rows {
                        ui.label(translator.t(&row.label_key));
                        // `value_key` ma pierwszeństwo — tak moduł pokazuje
                        // stany logiczne (Tak/Nie) bez wpisywania tekstu.
                        let value = match &row.value_key {
                            Some(key) => translator.t(key),
                            None => row.value.clone(),
                        };
                        ui.monospace(value);
                        ui.end_row();
                    }
                });
        }
        Block::Table { table } => table_ui(ui, table, translator),
        Block::Note { level, text_key } => {
            let color = match level {
                NoteLevel::Info => egui::Color32::from_rgb(130, 170, 210),
                NoteLevel::Warn => egui::Color32::from_rgb(220, 180, 90),
                NoteLevel::Error => egui::Color32::from_rgb(220, 100, 100),
            };
            ui.colored_label(color, translator.t(text_key));
        }
        Block::Badge { text_key } => {
            ui.label(
                egui::RichText::new(format!("[ {} ]", translator.t(text_key)))
                    .small()
                    .color(egui::Color32::from_rgb(140, 180, 230)),
            );
        }
        Block::Chart { chart } => chart_ui(ui, chart, translator),
    }
}

fn table_ui(ui: &mut egui::Ui, table: &Table, translator: &Translator) {
    if table.headers.is_empty() {
        return;
    }

    egui::Grid::new(ui.next_auto_id())
        .num_columns(table.headers.len())
        .spacing([18.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for header in &table.headers {
                ui.label(egui::RichText::new(translator.t(header)).strong());
            }
            ui.end_row();

            for row in &table.rows {
                for index in 0..table.headers.len() {
                    let value = row.get(index).map(String::as_str).unwrap_or("-");
                    ui.monospace(value);
                }
                ui.end_row();
            }
        });
}

/// Wykres liniowy. Osie są stałe (bez zoomu i przesuwania), bo to podgląd
/// telemetrii, a nie narzędzie do analizy — interakcja byłaby tu pułapką.
fn chart_ui(ui: &mut egui::Ui, chart: &Chart, translator: &Translator) {
    ui.label(
        egui::RichText::new(translator.t(&chart.title_key))
            .small()
            .weak(),
    );

    let mut plot = Plot::new(chart.title_key.as_str())
        .height(170.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label(chart.x_label.clone())
        .y_axis_label(chart.y_label.clone());

    if chart.y_label == "%" {
        plot = plot.include_y(0.0);
    }

    plot.show(ui, |plot_ui| {
        for (index, series) in chart.series.iter().enumerate() {
            let points: PlotPoints = series.points.iter().copied().collect();
            // Kolory jak w raporcie HTML — seria ma być rozpoznawalna w obu miejscach.
            let color = [
                egui::Color32::from_rgb(90, 150, 230),
                egui::Color32::from_rgb(230, 140, 70),
                egui::Color32::from_rgb(90, 190, 130),
            ][index % 3];
            plot_ui.line(Line::new(series.label.clone(), points).color(color));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use sk_core::view::{ChartSeries, KeyValue, Section};

    fn sample_view() -> View {
        View::new(vec![
            Section::new("module.system_info.section.os").with_blocks(vec![
                Block::key_values(vec![
                    KeyValue::new(
                        "module.system_info.os.caption",
                        "Microsoft Windows 11 Enterprise",
                    ),
                    KeyValue::state("module.hw_cpu.virtualization", "common.value.yes"),
                ]),
                Block::badge("common.state.elevated"),
            ]),
            Section::new("module.system_info.section.host").with_blocks(vec![
                Block::Table {
                    table: Table::new(
                        vec!["module.system_info.os.version".to_string()],
                        vec![vec!["10.0.26300".to_string()]],
                    ),
                },
                Block::chart(
                    Chart::new("module.system_live.chart.cpu", "s", "%").with_series(vec![
                        ChartSeries::new("CPU", vec![[0.0, 12.0], [1.0, 30.0], [2.0, 18.0]]),
                    ]),
                ),
            ]),
        ])
    }

    #[test]
    fn renders_translated_labels_table_and_badge() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = sample_view();

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(2);

        harness.get_by_label("System operacyjny");
        harness.get_by_label("Nazwa");
        harness.get_by_label("Microsoft Windows 11 Enterprise");
        harness.get_by_label("Wersja");
        harness.get_by_label("10.0.26300");
        harness.get_by_label("[ Administrator ]");
    }

    #[test]
    fn translates_value_key_instead_of_raw_value() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = sample_view();

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(2);

        // Wartość to klucz i18n, więc w UI widać „Tak”, a nie „common.value.yes”.
        harness.get_by_label("Tak");
    }

    #[test]
    fn renders_chart_without_panicking() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = sample_view();

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(3);

        harness.get_by_label("Obciążenie procesora");
    }
}
