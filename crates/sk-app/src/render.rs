//! Generyczny renderer modelu widoku.
//!
//! Moduły nie znają egui — zwracają [`sk_core::view::View`]. Dzięki temu nowy
//! moduł nie dotyka tego pliku, a widok da się przetestować jako JSON.

use eframe::egui;
use egui_plot::{Axis, AxisHints, GridMark, Line, Plot, PlotPoints};
use sk_core::i18n::Translator;
use sk_core::view::{
    Block, Chart, MapCell, NoteLevel, StatusLevel, SurfaceMap, Table, View, series_color,
};

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
        Block::Status {
            label_key,
            value_key,
            level,
        } => {
            // Baner stanu: ramka i duża kolorowana wartość, bo to pierwsza
            // informacja, której szuka technik („czy dysk jest zdrowy”).
            let color = status_color(*level);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(translator.t(label_key)).small().weak());
                    ui.label(
                        egui::RichText::new(translator.t(value_key))
                            .size(22.0)
                            .strong()
                            .color(color),
                    );
                });
            });
        }
        Block::Chart { chart } => chart_ui(ui, chart, translator),
        Block::Map { map } => map_ui(ui, map),
        Block::Group { title_key, blocks } => {
            // Domyślnie otwarta: grupa porządkuje listę, a nie ukrywa odczyty.
            egui::CollapsingHeader::new(egui::RichText::new(translator.t(title_key)).strong())
                .id_salt(ui.next_auto_id())
                .default_open(true)
                .show(ui, |ui| {
                    for block in blocks {
                        block_ui(ui, block, translator);
                        ui.add_space(4.0);
                    }
                });
        }
    }
}

/// Mapa powierzchni: siatka kolorowanych kwadratów jak w HD Tune.
fn map_ui(ui: &mut egui::Ui, map: &SurfaceMap) {
    const CELL: f32 = 8.0;
    const GAP: f32 = 1.0;
    let columns = usize::try_from(map.columns.max(1)).unwrap_or(1);
    let rows = map.cells.len().div_ceil(columns);
    if map.cells.is_empty() {
        ui.label("-");
        return;
    }
    let size = egui::vec2(columns as f32 * (CELL + GAP), rows as f32 * (CELL + GAP));
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    for (index, cell) in map.cells.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let min = rect.min + egui::vec2(column as f32 * (CELL + GAP), row as f32 * (CELL + GAP));
        let color = match cell {
            MapCell::Ok => egui::Color32::from_rgb(60, 150, 90),
            MapCell::Bad => egui::Color32::from_rgb(205, 80, 80),
            MapCell::Pending => egui::Color32::from_rgb(70, 76, 84),
        };
        painter.rect_filled(
            egui::Rect::from_min_size(min, egui::vec2(CELL, CELL)),
            1.0,
            color,
        );
    }
}

/// Kolor stanu — ten sam w GUI i w raporcie HTML, żeby „Dobry” znaczyło to samo
/// w obu miejscach.
pub fn status_color(level: StatusLevel) -> egui::Color32 {
    match level {
        StatusLevel::Ok => egui::Color32::from_rgb(110, 190, 120),
        StatusLevel::Warn => egui::Color32::from_rgb(220, 180, 90),
        StatusLevel::Bad => egui::Color32::from_rgb(220, 100, 100),
        StatusLevel::Unknown => egui::Color32::from_rgb(150, 156, 162),
    }
}

fn table_ui(ui: &mut egui::Ui, table: &Table, translator: &Translator) {
    if table.headers.is_empty() {
        return;
    }

    // Tabele bywają szersze niż okno (SMART ma 8 kolumn, pamięć 10), a egui nie
    // zawija komórek — bez poziomego przewijania ostatnie kolumny były ucięte.
    egui::ScrollArea::horizontal()
        .id_salt(ui.next_auto_id())
        .show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id())
                .num_columns(table.headers.len())
                .spacing([18.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for header in &table.headers {
                        ui.label(egui::RichText::new(translator.t(header)).strong());
                    }
                    ui.end_row();

                    for (row_index, row) in table.rows.iter().enumerate() {
                        for column in 0..table.headers.len() {
                            let value = match table.value_key(row_index, column) {
                                Some(key) => translator.t(key),
                                None => row.get(column).cloned().unwrap_or_else(|| "-".to_string()),
                            };
                            let mut text = egui::RichText::new(value).monospace();
                            if let Some(level) = table.cell_level(row_index, column) {
                                text = text.color(status_color(level));
                            }
                            ui.label(text);
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Wykres liniowy z etykietą. Osie są stałe (bez zoomu i przesuwania), bo to
/// podgląd telemetrii, a nie narzędzie do analizy — interakcja byłaby tu pułapką.
fn chart_ui(ui: &mut egui::Ui, chart: &Chart, translator: &Translator) {
    ui.label(
        egui::RichText::new(translator.t(&chart.title_key))
            .small()
            .weak(),
    );
    chart_plot(ui, chart, 170.0, chart.title_key.as_str(), ChartMode::Live);
}

/// Czy wykres ma reagować na kursor. `Still` wyłącza podpowiedzi, żeby zrzut
/// do pliku nie zależał od położenia myszy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartMode {
    Live,
    Still,
}

/// Sam rysunek wykresu, bez etykiety. Wyciągnięty osobno, bo ten sam wykres
/// pokazuje podgląd zrzutu w GUI (`sk-app::app`) — zapisany obrazek ma być
/// dokładnie tym, co widać na ekranie.
pub fn chart_plot(ui: &mut egui::Ui, chart: &Chart, height: f32, id_salt: &str, mode: ChartMode) {
    // Legenda nad wykresem, nie w rogu: dane dochodzą od prawej krawędzi, więc
    // legenda w narożniku zasłaniałaby najnowsze próbki. Jedna seria ma nazwę
    // w tytule wykresu — powtarzanie jej w legendzie nic nie wnosi.
    if chart.series.len() > 1 {
        chart_legend(ui, chart);
    }

    let mut plot = Plot::new(id_salt)
        .height(height)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label(chart.x_label.clone());

    if let (Some(min), Some(max)) = (chart.y_min, chart.y_max) {
        // Stała skala (np. 0–100 °C): bez niej 50–60 °C zajmuje całą wysokość
        // i każdy ruch wygląda jak skok o dziesiątki stopni.
        plot = plot
            .default_y_bounds(min, max)
            // Podziałka co 20 jednostek, tylko wewnątrz zakresu. Domyślny krok
            // przy stałym zakresie dawał linie gęstsze niż 20 px, a egui_plot
            // pomija etykiety, które się nie mieszczą — na osi zostawała sama
            // kreska bez liczb. Etykiety na krawędziach (0 i 100) nachodzą na
            // opisy osi, więc ich nie rysujemy.
            .y_grid_spacer(|input| {
                const STEP: f64 = 20.0;
                let (min, max) = input.bounds;
                let mut marks = Vec::new();
                let mut index = (min / STEP).ceil() as i64;
                loop {
                    let value = index as f64 * STEP;
                    if value >= max {
                        break;
                    }
                    if value > min {
                        marks.push(GridMark {
                            value,
                            step_size: STEP,
                        });
                    }
                    index += 1;
                }
                marks
            })
            // Własna oś Y: domyślna ma 14 px szerokości, więc dłuższe etykiety
            // („100”) są przycinane do ostatniej cyfry.
            .custom_y_axes(vec![
                AxisHints::new(Axis::Y)
                    .label(chart.y_label.clone())
                    .min_thickness(30.0)
                    .formatter(|mark, _range| format!("{:.0}", mark.value)),
            ]);
    } else {
        plot = plot.y_axis_label(chart.y_label.clone());
        if chart.from_zero || chart.y_label == "%" {
            plot = plot.include_y(0.0);
        }
    }

    // Stały zakres osi X: wykres ma gotowy układ (np. cały dysk w GB) już przy
    // pierwszej próbce, zamiast rozciągać się z każdym punktem.
    if let (Some(min), Some(max)) = (chart.x_min, chart.x_max) {
        plot = plot.default_x_bounds(min, max);
        if !chart.x_label.is_empty() {
            // Wartości bezwzględne (pozycja na dysku) muszą mieć jednostkę
            // przy każdym znaczniku — „250” na osi nic nie mówi.
            let unit = chart.x_label.clone();
            plot = plot.x_axis_formatter(move |mark, _range| {
                // Wartości tuż przy zerze dają „-0” przy zaokrąglaniu.
                let value = if mark.value.abs() < 0.5 {
                    0.0
                } else {
                    mark.value
                };
                format!("{value:.0} {unit}")
            });
        }
    } else if let Some(max) = chart.x_max {
        plot = plot.include_x(max);
    }

    plot.show(ui, |plot_ui| {
        for (index, series) in chart.series.iter().enumerate() {
            let points: PlotPoints = series.points.iter().copied().collect();
            // Kolory z palety `sk_core` — seria ma być rozpoznawalna w GUI
            // i w raporcie HTML, a legenda jest jedynym źródłem przypisania.
            let line = Line::new(series.label.clone(), points).color(series_color32(index));
            let line = if mode == ChartMode::Still {
                line.allow_hover(false)
            } else {
                line
            };
            plot_ui.line(line);
        }
    });
}

/// Próbka koloru i nazwa serii. Ta sama kolejność i paleta co w raporcie HTML.
fn chart_legend(ui: &mut egui::Ui, chart: &Chart) {
    ui.horizontal_wrapped(|ui| {
        for (index, series) in chart.series.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 3.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 1.0, series_color32(index));
            ui.label(egui::RichText::new(&series.label).small());
            ui.add_space(10.0);
        }
    });
}

/// Kolor serii z palety `sk_core` (zapis `#rrggbb`) jako kolor egui.
fn series_color32(index: usize) -> egui::Color32 {
    let hex = series_color(index).trim_start_matches('#');
    let channel = |offset: usize| {
        hex.get(offset..offset + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .unwrap_or(128)
    };
    egui::Color32::from_rgb(channel(0), channel(2), channel(4))
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

    #[test]
    fn renders_status_banner_with_translated_value() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = View::new(vec![
            Section::new("module.storage_smart.section.drive").with_blocks(vec![Block::status(
                "module.storage_smart.drive.health",
                "module.storage_smart.health.good",
                StatusLevel::Ok,
            )]),
        ]);

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(2);

        harness.get_by_label("Stan dysku");
        harness.get_by_label("Dobry");
    }

    #[test]
    fn renders_surface_map_without_panicking() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let maps = [
            // Pełna siatka, pusta mapa i zerowa liczba kolumn — kolumny są
            // dzielnikiem przy układaniu komórek, więc każdy wariant musi
            // dać się narysować.
            SurfaceMap {
                cells: vec![MapCell::Ok, MapCell::Bad, MapCell::Pending],
                columns: 3,
            },
            SurfaceMap {
                cells: Vec::new(),
                columns: 64,
            },
            SurfaceMap {
                cells: vec![MapCell::Pending],
                columns: 0,
            },
        ];
        for map in maps {
            let model = View::new(vec![
                Section::new("module.storage_sectors.section").with_blocks(vec![Block::map(map)]),
            ]);
            let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
            harness.run_steps(2);
            harness.get_by_label("Mapa powierzchni");
        }
    }

    #[test]
    fn renders_group_as_expandable_header_with_contents() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = View::new(vec![Section::new("module.hw_sensors.section").with_blocks(
            vec![Block::group(
                "module.hw_sensors.group.fan",
                vec![Block::Table {
                    table: Table::new(
                        vec!["module.hw_sensors.col.value".to_string()],
                        vec![vec!["1024 RPM".to_string()]],
                    ),
                }],
            )],
        )]);

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(2);

        // Nagłówek grupy i jej zawartość są widoczne bez rozwijania.
        harness.get_by_label("Wentylatory");
        harness.get_by_label("1024 RPM");
    }

    #[test]
    fn chart_legend_names_every_series() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let model = View::new(vec![Section::new("module.hw_sensors.section").with_blocks(
            vec![Block::chart(
                Chart::new("module.hw_sensors.chart.temperature", "s", "°C")
                    .with_y_range(0.0, 100.0)
                    .with_x_range(-120.0, 0.0)
                    .with_series(vec![
                        ChartSeries::new("AMD Ryzen 5 5600", vec![[-1.0, 50.0], [0.0, 55.0]]),
                        ChartSeries::new(
                            "AMD Radeon RX 6700 XT",
                            vec![[-1.0, 48.0], [0.0, 52.0]],
                        ),
                    ]),
            )],
        )]);

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(3);

        // Bez legendy kolor linii nie mówił, który podzespół jest który.
        harness.get_by_label("AMD Ryzen 5 5600");
        harness.get_by_label("AMD Radeon RX 6700 XT");
    }

    #[test]
    fn renders_cells_with_status_level() {
        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let table = Table::new(
            vec!["module.storage_smart.col.health".to_string()],
            vec![vec![String::new()], vec!["86 %".to_string()]],
        )
        .with_value_key(0, 0, "module.storage_smart.health.bad")
        .with_cell_level(0, 0, StatusLevel::Bad)
        .with_cell_level(1, 0, StatusLevel::Warn);
        let model = View::new(vec![
            Section::new("module.storage_smart.section").with_blocks(vec![Block::Table { table }]),
        ]);

        let mut harness = Harness::new_ui(|ui| view(ui, &model, &translator));
        harness.run_steps(2);

        harness.get_by_label("Zły");
        harness.get_by_label("86 %");
    }
}
