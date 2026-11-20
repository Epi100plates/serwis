//! Raport HTML — ten sam model widoku co GUI, więc nie trzeba pisać szablonów
//! per moduł.
//!
//! Świadomie bez `askama` i bez JavaScriptu: HTML powstaje z `View`, jest
//! samodzielnym plikiem (arkusz stylów w `<style>`), a wykresy rysujemy jako
//! inline SVG. Każda wartość przechodzi przez [`escape`] — raport zawiera dane
//! z WMI, więc nie ufamy im w kwestii znaków specjalnych.

use std::fmt::Write as _;

use sk_core::i18n::Translator;
use sk_core::view::{
    Block, Chart, MapCell, NoteLevel, StatusLevel, SurfaceMap, View, series_color,
};

use crate::report::{ModuleEntry, Report};

const STYLE: &str = r#"
:root { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0 auto; max-width: 1000px; padding: 24px 20px 64px;
       font: 14px/1.5 "Segoe UI", system-ui, sans-serif; color: #1d1f21; background: #fff; }
header h1 { margin: 0 0 4px; font-size: 26px; }
.meta { color: #5b6167; margin: 0 0 16px; }
nav ul { display: flex; flex-wrap: wrap; gap: 6px 14px; list-style: none; padding: 0; margin: 0 0 24px; }
nav a { color: #1a5fb4; text-decoration: none; }
nav a:hover { text-decoration: underline; }
section { border-top: 2px solid #e3e5e8; padding-top: 12px; margin-top: 28px; }
section h2 { margin: 0 0 2px; font-size: 20px; }
section .desc { color: #5b6167; margin: 0 0 6px; }
.badges { display: flex; gap: 8px; margin: 0 0 10px; }
.badge { font-size: 12px; color: #1a5fb4; background: #eaf1fb; border-radius: 10px; padding: 1px 8px; }
h3 { font-size: 15px; margin: 18px 0 6px; }
table { border-collapse: collapse; width: 100%; margin: 4px 0 8px; }
th, td { text-align: left; padding: 4px 10px; border-bottom: 1px solid #eceef1; vertical-align: top; }
thead th { background: #f6f7f9; font-weight: 600; }
td.value, td.mono { font-family: Consolas, "Cascadia Mono", monospace; }
table.kv th { width: 34%; font-weight: 500; color: #33383d; }
.note { margin: 6px 0; padding: 6px 10px; border-left: 3px solid #9bb7d4; background: #f7f9fb; }
.note.warn { border-color: #d7a93b; background: #fdf8ec; }
.note.error { border-color: #cc5f5f; background: #fdf0f0; }
.status { display: flex; align-items: baseline; gap: 10px; margin: 8px 0;
          padding: 6px 12px; border-left: 4px solid #9bb7d4; background: #f7f9fb; }
.status .status-label { font-size: 12px; text-transform: uppercase; letter-spacing: .04em; color: #5b6167; }
.status strong { font-size: 19px; }
.status.lvl-ok { border-color: #2e9e5b; background: #f1faf4; }
.status.lvl-ok strong { color: #1f7a45; }
.status.lvl-warn { border-color: #d7a93b; }
.status.lvl-warn strong { color: #8a6a12; }
.status.lvl-bad { border-color: #cc5f5f; background: #fdf0f0; }
.status.lvl-bad strong { color: #a52f2f; }
.status.lvl-unknown strong { color: #5b6167; }
td.lvl-ok { color: #1f7a45; }
td.lvl-warn { color: #8a6a12; font-weight: 600; }
td.lvl-bad { color: #a52f2f; font-weight: 700; }
figure.chart { margin: 12px 0; }
figure.chart figcaption { font-size: 12px; color: #5b6167; margin-bottom: 2px; }
.chart-legend { display: flex; flex-wrap: wrap; gap: 2px 14px; list-style: none;
                padding: 0; margin: 4px 0 2px; font-size: 12px; color: #33383d; }
.chart-legend .swatch { display: inline-block; width: 12px; height: 3px;
                        margin-right: 6px; vertical-align: middle; border-radius: 1px; }
details { margin: 10px 0 4px; }
details > summary { cursor: pointer; font-weight: 600; font-size: 13px; color: #33383d;
                    padding: 2px 0; }
details > summary:hover { color: #1a5fb4; }
figure.surface-map { margin: 12px 0; }
.map-ok { fill: #2e9e5b; }
.map-bad { fill: #cc5f5f; }
.map-pending { fill: #e3e6ea; }
svg { max-width: 100%; height: auto; }
.errors { border-top-color: #cc5f5f; }
footer { margin-top: 36px; color: #7b8187; font-size: 12px; }
@media print { nav { display: none; } body { max-width: none; } }
"#;

pub fn render(report: &Report, translator: &Translator) -> String {
    let mut out = String::with_capacity(16 * 1024);

    let stamp = short_timestamp(&report.generated_at);
    let title = format!("SerwisKit — {stamp}");
    let privileges = translator.t(if report.elevated {
        "common.state.elevated"
    } else {
        "common.state.userland"
    });

    let _ = writeln!(
        out,
        "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>",
        lang = escape(report.locale_code),
        title = escape(&title),
        STYLE = STYLE,
    );

    let _ = writeln!(
        out,
        "<header>\n<h1>SerwisKit</h1>\n<p class=\"meta\">{} · {} · {} · {} · {} {}</p>\n</header>",
        escape(&stamp),
        escape(&privileges),
        escape(report.locale_code),
        escape(&format!("v{}", report.app_version)),
        escape(&translator.t("ui.uptime")),
        escape(&format_uptime(report.uptime_secs)),
    );

    if !report.modules.is_empty() {
        out.push_str("<nav><ul>\n");
        for entry in &report.modules {
            let _ = writeln!(
                out,
                "<li><a href=\"#{}\">{}</a></li>",
                anchor(entry.meta.id),
                escape(&translator.t(entry.meta.name_key)),
            );
        }
        out.push_str("</ul></nav>\n");
    }

    out.push_str("<main>\n");
    for entry in &report.modules {
        module_html(&mut out, entry, translator);
    }
    out.push_str("</main>\n");

    if !report.errors.is_empty() {
        let _ = writeln!(
            out,
            "<section class=\"errors\" id=\"errors\">\n<h2>{}</h2>\n<ul>",
            escape(&translator.t("ui.error")),
        );
        for failure in &report.errors {
            let _ = writeln!(
                out,
                "<li><strong>{}</strong>: {}</li>",
                escape(failure.module),
                escape(&failure.error),
            );
        }
        out.push_str("</ul>\n</section>\n");
    }

    let _ = writeln!(
        out,
        "<footer>SerwisKit {} — GPL-3.0-only</footer>\n</body>\n</html>",
        escape(&format!("v{}", report.app_version)),
    );

    out
}

fn module_html(out: &mut String, entry: &ModuleEntry, translator: &Translator) {
    let meta = entry.meta;
    let _ = writeln!(
        out,
        "<section id=\"{}\">\n<h2>{}</h2>\n<p class=\"desc\">{}</p>\n<div class=\"badges\">\
         <span class=\"badge\">{}</span><span class=\"badge\">{}</span>",
        anchor(meta.id),
        escape(&translator.t(meta.name_key)),
        escape(&translator.t(meta.desc_key)),
        escape(&translator.t(meta.group.label_key())),
        escape(&translator.t(meta.capability.label_key())),
    );
    if meta.experimental {
        let _ = write!(
            out,
            "<span class=\"badge\">{}</span>",
            escape(&translator.t("ui.experimental"))
        );
    }
    out.push_str("</div>");

    view_html(out, &entry.output.view, translator);
    out.push_str("</section>\n");
}

fn view_html(out: &mut String, view: &View, translator: &Translator) {
    for section in &view.sections {
        let _ = writeln!(
            out,
            "<h3>{}</h3>",
            escape(&translator.t(&section.title_key))
        );
        if let Some(subtitle) = &section.subtitle {
            let _ = writeln!(out, "<p class=\"desc\">{}</p>", escape(subtitle));
        }
        for block in &section.blocks {
            block_html(out, block, translator);
        }
    }
}

fn block_html(out: &mut String, block: &Block, translator: &Translator) {
    match block {
        Block::KeyValues { rows } => {
            out.push_str("<table class=\"kv\">\n<tbody>\n");
            for row in rows {
                let value = match &row.value_key {
                    Some(key) => translator.t(key),
                    None => row.value.clone(),
                };
                let _ = writeln!(
                    out,
                    "<tr><th>{}</th><td class=\"value\">{}</td></tr>",
                    escape(&translator.t(&row.label_key)),
                    escape(&value),
                );
            }
            out.push_str("</tbody>\n</table>\n");
        }
        Block::Table { table } => table_html(out, table, translator),
        Block::Note { level, text_key } => {
            let class = match level {
                NoteLevel::Info => "note",
                NoteLevel::Warn => "note warn",
                NoteLevel::Error => "note error",
            };
            let _ = writeln!(
                out,
                "<p class=\"{class}\">{}</p>",
                escape(&translator.t(text_key))
            );
        }
        Block::Badge { text_key } => {
            let _ = writeln!(
                out,
                "<p><span class=\"badge\">{}</span></p>",
                escape(&translator.t(text_key))
            );
        }
        Block::Status {
            label_key,
            value_key,
            level,
        } => {
            let _ = writeln!(
                out,
                "<p class=\"status {}\"><span class=\"status-label\">{}</span><strong>{}</strong></p>",
                level_class(*level),
                escape(&translator.t(label_key)),
                escape(&translator.t(value_key)),
            );
        }
        Block::Chart { chart } => {
            let _ = writeln!(
                out,
                "<figure class=\"chart\">\n<figcaption>{}</figcaption>\n{}{}</figure>",
                escape(&translator.t(&chart.title_key)),
                chart_legend(chart),
                chart_svg(chart),
            );
        }
        Block::Map { map } => map_html(out, map),
        Block::Group { title_key, blocks } => {
            // `<details open>` — w raporcie rozwijane bez JavaScriptu, a przy
            // druku i wyszukiwaniu treść pozostaje widoczna.
            let _ = writeln!(
                out,
                "<details open>\n<summary>{}</summary>",
                escape(&translator.t(title_key))
            );
            for block in blocks {
                block_html(out, block, translator);
            }
            out.push_str("</details>\n");
        }
    }
}

/// Mapa powierzchni w raporcie: ta sama siatka co w GUI, jako inline SVG.
fn map_html(out: &mut String, map: &SurfaceMap) {
    const CELL: u32 = 10;
    let columns = map.columns.max(1);
    let rows = (map.cells.len() as u32).div_ceil(columns).max(1);
    let width = columns * CELL;
    let height = rows * CELL;
    let _ = writeln!(
        out,
        "<figure class=\"surface-map\"><svg viewBox=\"0 0 {width} {height}\" width=\"{width}\" height=\"{height}\" role=\"img\">"
    );
    for (index, cell) in map.cells.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let class = match cell {
            MapCell::Ok => "map-ok",
            MapCell::Bad => "map-bad",
            MapCell::Pending => "map-pending",
        };
        let _ = writeln!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" class=\"{class}\"/>",
            column * CELL,
            row * CELL,
            CELL - 1,
            CELL - 1,
        );
    }
    out.push_str("</svg></figure>\n");
}

fn table_html(out: &mut String, table: &sk_core::view::Table, translator: &Translator) {
    if table.headers.is_empty() {
        return;
    }
    out.push_str("<table>\n<thead>\n<tr>");
    for header in &table.headers {
        let _ = writeln!(out, "<th>{}</th>", escape(&translator.t(header)));
    }
    out.push_str("</tr>\n</thead>\n<tbody>");
    for (row_index, row) in table.rows.iter().enumerate() {
        out.push_str("<tr>");
        for column in 0..table.headers.len() {
            let value = match table.value_key(row_index, column) {
                Some(key) => translator.t(key),
                None => row.get(column).cloned().unwrap_or_else(|| "-".to_string()),
            };
            let class = match table.cell_level(row_index, column) {
                Some(level) => format!("mono {}", level_class(level)),
                None => "mono".to_string(),
            };
            let _ = writeln!(out, "<td class=\"{class}\">{}</td>", escape(&value));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody>\n</table>\n");
}

/// Nazwa klasy CSS dla poziomu stanu — wspólna dla banera i komórek tabeli.
fn level_class(level: StatusLevel) -> &'static str {
    match level {
        StatusLevel::Ok => "lvl-ok",
        StatusLevel::Warn => "lvl-warn",
        StatusLevel::Bad => "lvl-bad",
        StatusLevel::Unknown => "lvl-unknown",
    }
}

/// Wykres jako inline SVG — bez JavaScriptu, więc raport działa z dysku.
/// Zakres osi bierze z modelu, gdy moduł go ustawił (cały dysk, okno czasu),
/// a poza tym dopasowuje do danych. Procenty zawsze skalujemy 0–100, żeby skala
/// była porównywalna między maszynami.
fn chart_svg(chart: &Chart) -> String {
    const WIDTH: f64 = 640.0;
    const HEIGHT: f64 = 180.0;
    const PAD: f64 = 26.0;

    let mut data_min_x = 0.0_f64;
    let mut data_max_x = 0.0_f64;
    let mut data_max_y = 0.0_f64;
    let mut any_point = false;
    for series in &chart.series {
        for [x, y] in &series.points {
            if x.is_finite() && y.is_finite() {
                any_point = true;
                data_min_x = data_min_x.min(*x);
                data_max_x = data_max_x.max(*x);
                data_max_y = data_max_y.max(*y);
            }
        }
    }
    if !any_point {
        return String::new();
    }

    let min_x = chart.x_min.unwrap_or(data_min_x.min(0.0));
    let max_x = chart.x_max.unwrap_or(data_max_x);
    let min_y = chart.y_min.unwrap_or(0.0);
    let max_y = match chart.y_max {
        Some(max) if max > min_y => max,
        // Procenty zawsze 0–100 (seria samych zer to nie powód, by wykres
        // znikał); dla innych jednostek unikamy dzielenia przez zero.
        _ if chart.y_label == "%" => 100.0_f64.max(data_max_y),
        _ => (data_max_y * 1.05).max(1.0),
    };
    // NaN w danych nie może przejść przez projekcję: oś o zerowej szerokości
    // albo niezdefiniowanym zakresie dałaby dzielenie przez zero.
    if !(max_x - min_x).is_finite()
        || max_x - min_x <= 0.0
        || !(max_y - min_y).is_finite()
        || max_y - min_y <= 0.0
    {
        return String::new();
    }

    let plot_width = WIDTH - 2.0 * PAD;
    let plot_height = HEIGHT - 2.0 * PAD;
    let project = |x: f64, y: f64| {
        let px = PAD + (x - min_x) / (max_x - min_x) * plot_width;
        let py = PAD + plot_height - (y - min_y) / (max_y - min_y) * plot_height;
        (px, py)
    };

    let mut svg = format!(
        "<svg viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"img\" \
         aria-label=\"{}\" xmlns=\"http://www.w3.org/2000/svg\">",
        escape(&chart.title_key)
    );

    // Ramka i podziałka pozioma.
    let _ = write!(
        svg,
        "<rect x=\"{PAD}\" y=\"{PAD}\" width=\"{plot_width}\" height=\"{plot_height}\" \
         fill=\"none\" stroke=\"#e3e5e8\"/>"
    );
    for step in 1..4 {
        let y = PAD + plot_height / 4.0 * f64::from(step);
        let _ = write!(
            svg,
            "<line x1=\"{PAD}\" y1=\"{y:.1}\" x2=\"{x2:.1}\" y2=\"{y:.1}\" stroke=\"#f0f2f4\"/>",
            x2 = PAD + plot_width
        );
    }

    // Podziałka osi: pięć równych kroków z wartościami. Przy ustalonym zakresie
    // (cały dysk, okno czasu) raport pokazuje od razu całą szerokość.
    for step in 0..=4 {
        let fraction = f64::from(step) / 4.0;
        // Wartości tuż przy zerze dają „-0” przy zaokrąglaniu.
        let label = |value: f64| {
            if value.abs() < 0.5 {
                "0".to_string()
            } else {
                format!("{value:.0}")
            }
        };
        let _ = write!(
            svg,
            "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"8\" fill=\"#5b6167\" \
             text-anchor=\"middle\">{}</text>",
            label(min_x + (max_x - min_x) * fraction),
            x = PAD + plot_width * fraction,
            y = PAD + plot_height + 12.0
        );
        let _ = write!(
            svg,
            "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"8\" fill=\"#5b6167\" \
             text-anchor=\"end\">{}</text>",
            label(min_y + (max_y - min_y) * fraction),
            x = PAD - 4.0,
            y = PAD + plot_height - plot_height * fraction + 3.0
        );
    }

    for (index, series) in chart.series.iter().enumerate() {
        let color = series_color(index);
        let points: Vec<String> = series
            .points
            .iter()
            .filter(|[x, y]| x.is_finite() && y.is_finite())
            .map(|[x, y]| {
                let (px, py) = project(*x, *y);
                format!("{px:.1},{py:.1}")
            })
            .collect();
        if points.is_empty() {
            continue;
        }
        let _ = write!(
            svg,
            "<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\" points=\"{}\"/>",
            points.join(" ")
        );
    }

    let _ = write!(
        svg,
        "<text x=\"{PAD}\" y=\"{y:.1}\" font-size=\"10\" fill=\"#5b6167\">{label}</text>",
        y = HEIGHT - 6.0,
        label = escape(&chart.x_label)
    );
    let _ = write!(
        svg,
        "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"10\" fill=\"#5b6167\">{label}</text>",
        x = PAD,
        y = PAD - 8.0,
        label = escape(&chart.y_label)
    );

    svg.push_str("</svg>\n");
    svg
}

/// Legenda wykresu: próbka koloru i nazwa serii — ta sama paleta i kolejność co
/// w GUI. Jedna seria ma nazwę w tytule wykresu, więc legenda byłaby powtórzeniem.
fn chart_legend(chart: &Chart) -> String {
    if chart.series.len() < 2 {
        return String::new();
    }
    let mut legend = String::from("<ul class=\"chart-legend\">");
    for (index, series) in chart.series.iter().enumerate() {
        let _ = write!(
            legend,
            "<li><span class=\"swatch\" style=\"background:{}\"></span>{}</li>",
            series_color(index),
            escape(&series.label)
        );
    }
    legend.push_str("</ul>\n");
    legend
}

fn anchor(id: &str) -> String {
    id.replace(['.', '_'], "-")
}

/// `2026-11-18T19:23:56.327+01:00` → `2026-11-18 19:23` (nagłówek i tytuł).
fn short_timestamp(generated_at: &str) -> String {
    let short: String = generated_at.chars().take(16).collect();
    short.replace('T', " ")
}

fn format_uptime(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    format!("{days}d {hours:02}:{minutes:02}")
}

/// Escapowanie treści HTML. Wszystkie wartości w raporcie pochodzą z WMI,
/// więc traktujemy je jak dane niezaufane.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::ReportOptions;
    use sk_core::live::LiveSample;
    use sk_core::testing::{Harness, MockLive, MockWmi};
    use sk_core::view::{ChartSeries, KeyValue, Section, Table};

    fn report() -> Report {
        let wmi = MockWmi::new()
            .with_rows(
                "Win32_OperatingSystem",
                vec![serde_json::json!({
                    "Caption": "Windows <script>alert(1)</script>",
                    "Version": "10.0.26300",
                    "BuildNumber": "26300",
                    "OSArchitecture": "64-bit"
                })],
            )
            .with_rows(
                "Win32_ComputerSystem",
                vec![serde_json::json!({
                    "Manufacturer": "A&B \"C\"",
                    "Model": "MS-7C52",
                    "TotalPhysicalMemory": 34_281_689_088u64,
                    "HypervisorPresent": false,
                    "PCSystemType": 1
                })],
            );
        let live = MockLive::new().with_samples(vec![
            LiveSample::new(0.0).with_cpu_load(10.0).with_ram_used(40.0),
            LiveSample::new(1.0).with_cpu_load(20.0).with_ram_used(41.0),
        ]);
        let harness = Harness::new().with_wmi(wmi).with_live(live);
        let registry = sk_modules::registry();
        let options = ReportOptions {
            module_filter: Some("system.info"),
            ..ReportOptions::default()
        };
        Report::collect(&registry, &harness.ctx(), &options)
    }

    #[test]
    fn escapes_all_dangerous_characters() {
        assert_eq!(
            escape("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn document_contains_module_and_sections() {
        let translator = Translator::load().expect("tłumaczenia");
        let html = report().to_html(&translator);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("id=\"system-info\""));
        assert!(html.contains("System operacyjny"));
        assert!(html.contains("Administrator") || html.contains("Użytkownik"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn user_data_is_escaped_not_executed() {
        let translator = Translator::load().expect("tłumaczenia");
        let html = report().to_html(&translator);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("A&amp;B &quot;C&quot;"));
    }

    #[test]
    fn json_and_html_describe_the_same_modules() {
        let harness = Harness::new();
        let registry = sk_modules::registry();
        let options = ReportOptions {
            module_filter: Some("system.info"),
            ..ReportOptions::default()
        };
        // JSON z pustym dostawcą: moduł jest w błędach, HTML je pokazuje.
        let json = Report::collect(&registry, &harness.ctx(), &options).to_json();
        let errors = json["errors"].as_array().expect("błędy");
        assert_eq!(errors.len(), 1);

        let report = Report::collect(&registry, &harness.ctx(), &options);
        let translator = Translator::load().expect("tłumaczenia");
        let html = report.to_html(&translator);
        assert!(html.contains("id=\"errors\""));
        assert!(html.contains("system.info"));
    }

    #[test]
    fn chart_becomes_inline_svg() {
        let chart = Chart::new("module.system_live.chart.cpu", "s", "%").with_series(vec![
            ChartSeries::new("CPU", vec![[0.0, 10.0], [1.0, 20.0], [2.0, 15.0]]),
        ]);
        let svg = chart_svg(&chart);
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(svg.contains("<polyline"), "{svg}");
        // Skala 0–100%: punkt (0 s, 10%) pada w (26.0, 141.2) px.
        assert!(svg.contains("26.0,141.2"), "{svg}");
        assert!(svg.contains(">%<"), "oś Y z jednostką: {svg}");
    }

    #[test]
    fn all_zero_percent_series_still_draws() {
        let chart = Chart::new("module.system_live.chart.cpu", "s", "%").with_series(vec![
            ChartSeries::new("CPU", vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]),
        ]);
        let svg = chart_svg(&chart);
        assert!(svg.contains("<polyline"), "{svg}");
        // Skala 0–100%: y = 0 pada na dolną krawędź (26 + 128 = 154 px).
        assert!(svg.contains("26.0,154.0"), "{svg}");
    }

    #[test]
    fn empty_chart_renders_nothing() {
        let chart = Chart::new("module.system_live.chart.cpu", "s", "%");
        assert!(chart_svg(&chart).is_empty());
    }

    #[test]
    fn fixed_x_range_keeps_the_full_disk_scale() {
        let chart = Chart::new("module.storage_bench.chart.speed", "GB", "MB/s")
            .with_x_range(0.0, 100.0)
            .with_series(vec![ChartSeries::new(
                "READ",
                vec![[0.0, 500.0], [10.0, 400.0]],
            )]);
        let svg = chart_svg(&chart);
        // 10 GB z 100 GB to 10 % szerokości pola (588 px) — punkt na 84.8 px,
        // a nie na prawym końcu, jak przy skali dopasowanej do danych.
        assert!(svg.contains("84.8"), "{svg}");
        assert!(svg.contains(">100<"), "podziałka osi X: {svg}");
    }

    #[test]
    fn group_renders_as_expandable_details() {
        let translator = Translator::load().expect("tłumaczenia");
        let view = View::new(vec![Section::new("module.hw_sensors.section").with_blocks(
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
        let mut html = String::new();
        view_html(&mut html, &view, &translator);
        assert!(html.contains("<details open>"), "{html}");
        assert!(html.contains("<summary>Wentylatory</summary>"), "{html}");
        assert!(html.contains("1024 RPM"), "{html}");
    }

    #[test]
    fn chart_legend_matches_series_colors() {
        let chart = Chart::new("module.hw_sensors.chart.temperature", "s", "°C").with_series(vec![
            ChartSeries::new("AMD Ryzen 5 5600", vec![[-1.0, 50.0], [0.0, 55.0]]),
            ChartSeries::new("AMD Radeon RX 6700 XT", vec![[-1.0, 48.0], [0.0, 52.0]]),
        ]);
        let legend = chart_legend(&chart);
        assert!(legend.contains(">AMD Ryzen 5 5600<"), "{legend}");
        assert!(legend.contains(">AMD Radeon RX 6700 XT<"), "{legend}");
        // Kolory z jednej palety — ten sam podzespół ma ten sam kolor w SVG.
        let svg = chart_svg(&chart);
        for color in [series_color(0), series_color(1)] {
            assert!(legend.contains(&format!("background:{color}")), "{legend}");
            assert!(svg.contains(&format!("stroke=\"{color}\"")), "{svg}");
        }
    }

    #[test]
    fn single_series_chart_has_no_legend() {
        let chart = Chart::new("module.system_live.chart.cpu", "s", "%").with_series(vec![
            ChartSeries::new("CPU", vec![[0.0, 10.0], [1.0, 20.0]]),
        ]);
        assert!(chart_legend(&chart).is_empty());
    }

    #[test]
    fn fixed_y_range_scales_the_temperature_axis() {
        let chart = Chart::new("module.hw_sensors.chart.temperature", "s", "°C")
            .with_y_range(0.0, 100.0)
            .with_series(vec![ChartSeries::new(
                "CPU",
                vec![[-1.0, 50.0], [0.0, 60.0]],
            )]);
        let svg = chart_svg(&chart);
        // 50 °C na skali 0–100 pada w połowie wysokości pola (26 + 64 = 90 px).
        assert!(svg.contains(",90.0"), "{svg}");
        assert!(svg.contains(">100<"), "górna podziałka osi Y: {svg}");
        assert!(svg.contains(">0<"), "dolna podziałka osi Y: {svg}");
    }

    #[test]
    fn time_window_puts_the_newest_sample_on_the_right_edge() {
        let chart = Chart::new("module.hw_sensors.chart.temperature", "s", "°C")
            .with_x_range(-120.0, 0.0)
            .with_series(vec![ChartSeries::new(
                "CPU",
                vec![[-120.0, 50.0], [-60.0, 55.0], [0.0, 60.0]],
            )]);
        let svg = chart_svg(&chart);
        // Najnowsza próbka (0 s) trafia na prawą krawędź pola: 26 + 588 px.
        assert!(svg.contains("614.0"), "{svg}");
        assert!(svg.contains(">-120<"), "lewa krawędź okna: {svg}");
        assert!(svg.contains(">0<"), "prawa krawędź okna: {svg}");
    }

    #[test]
    fn values_use_translation_keys() {
        let translator = Translator::load().expect("tłumaczenia");
        let view = View::new(vec![Section::new("module.hw_monitor.section").with_blocks(
            vec![Block::key_values(vec![KeyValue::state(
                "module.hw_cpu.virtualization",
                "common.value.yes",
            )])],
        )]);
        let mut html = String::new();
        view_html(&mut html, &view, &translator);
        assert!(html.contains(">Tak<"), "{html}");
    }

    #[test]
    fn status_block_renders_colored_banner() {
        let translator = Translator::load().expect("tłumaczenia");
        let view = View::new(vec![
            Section::new("module.storage_smart.section.drive").with_blocks(vec![Block::status(
                "module.storage_smart.drive.health",
                "module.storage_smart.health.bad",
                StatusLevel::Bad,
            )]),
        ]);
        let mut html = String::new();
        view_html(&mut html, &view, &translator);
        assert!(html.contains("<p class=\"status lvl-bad\">"), "{html}");
        assert!(html.contains("<strong>Zły</strong>"), "{html}");
        assert!(html.contains("Stan dysku"), "{html}");
    }

    #[test]
    fn table_cells_with_level_get_css_class() {
        let translator = Translator::load().expect("tłumaczenia");
        let table = Table::new(
            vec!["module.storage_smart.ata.col.name".to_string()],
            vec![vec!["x".to_string()], vec!["y".to_string()]],
        )
        .with_cell_level(0, 0, StatusLevel::Warn);
        let mut html = String::new();
        table_html(&mut html, &table, &translator);
        assert!(
            html.contains("<td class=\"mono lvl-warn\">x</td>"),
            "{html}"
        );
        assert!(html.contains("<td class=\"mono\">y</td>"), "{html}");
    }

    #[test]
    fn map_block_becomes_svg_grid_with_cell_classes() {
        let translator = Translator::load().expect("tłumaczenia");
        let view = View::new(vec![
            Section::new("module.storage_sectors.section").with_blocks(vec![Block::map(
                SurfaceMap {
                    cells: vec![MapCell::Ok, MapCell::Bad, MapCell::Pending, MapCell::Ok],
                    columns: 2,
                },
            )]),
        ]);
        let mut html = String::new();
        view_html(&mut html, &view, &translator);

        assert_eq!(html.matches("<rect").count(), 4, "{html}");
        assert_eq!(html.matches("map-ok").count(), 2, "{html}");
        assert_eq!(html.matches("map-bad").count(), 1, "{html}");
        assert_eq!(html.matches("map-pending").count(), 1, "{html}");
    }

    #[test]
    fn empty_map_and_zero_columns_do_not_panic() {
        // Pusta mapa: samo SVG, bez komórek.
        let mut html = String::new();
        map_html(
            &mut html,
            &SurfaceMap {
                cells: Vec::new(),
                columns: 64,
            },
        );
        assert!(html.contains("<svg"), "{html}");
        assert_eq!(html.matches("<rect").count(), 0, "{html}");

        // Zerowa szerokość siatki nie może wywrócić układania komórek.
        let mut flat = String::new();
        map_html(
            &mut flat,
            &SurfaceMap {
                cells: vec![MapCell::Ok],
                columns: 0,
            },
        );
        assert_eq!(flat.matches("<rect").count(), 1, "{flat}");
        assert!(flat.contains("map-ok"), "{flat}");
    }

    #[test]
    fn short_timestamp_trims_nanoseconds_and_offset() {
        assert_eq!(
            short_timestamp("2026-11-18T19:23:56.327456400+01:00"),
            "2026-11-18 19:23"
        );
        assert_eq!(short_timestamp(""), "");
    }

    #[test]
    fn anchors_replace_dots() {
        assert_eq!(anchor("hw.disk_inventory"), "hw-disk-inventory");
    }
}
