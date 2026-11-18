//! `system.live` — telemetria w czasie: obciążenie CPU, taktowanie, zużycie RAM.
//!
//! Moduł nie próbkuje sam. Wątek roboczy aplikacji dopisuje próbki do bufora
//! pierścieniowego (`LiveSource`), a moduł zamienia je na wykresy. Dzięki temu
//! `collect()` jest natychmiastowe i nie blokuje UI.

use sk_core::prelude::*;

/// Ile próbek pokazujemy w wykresie (przy 1 s = 5 minut historii).
const MAX_CHART_POINTS: usize = 300;

#[derive(Debug, Default)]
pub struct Live;

impl Module for Live {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "system.live",
            name_key: "module.system_live.name",
            desc_key: "module.system_live.desc",
            group: Group::System,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let samples = ctx.live.samples();
        let window: Vec<LiveSample> = samples
            .iter()
            .rev()
            .take(MAX_CHART_POINTS)
            .rev()
            .copied()
            .collect();

        if window.is_empty() {
            return Ok(ModuleOutput {
                data: serde_json::json!({
                    "schema_version": 1,
                    "sample_count": 0,
                    "samples": [],
                }),
                view: View::new(vec![
                    Section::new("module.system_live.section").with_blocks(vec![
                        Block::note(NoteLevel::Info, "module.system_live.note.waiting"),
                        Block::note(NoteLevel::Info, "module.system_live.note.interval"),
                    ]),
                ]),
            });
        }

        let last = window.last().copied().unwrap_or(LiveSample::new(0.0));
        // Faktyczna kadencja bywa inna niż nominalna 1 s — zależy od czasu
        // odpowiedzi WMI, więc pokazujemy zmierzoną, a nie obiecywaną.
        let average_interval = match (window.first(), window.last()) {
            (Some(first), Some(last)) if window.len() > 1 => {
                Some((last.elapsed_secs - first.elapsed_secs) / (window.len() - 1) as f64)
            }
            _ => None,
        };
        let mut blocks = vec![Block::key_values(vec![
            KeyValue::new(
                "module.system_live.samples",
                format!("{} / {}", window.len(), samples.len()),
            ),
            KeyValue::new(
                "module.system_live.window",
                format!("{:.0} s", window.last().map_or(0.0, |s| s.elapsed_secs)),
            ),
            KeyValue::new(
                "module.system_live.interval",
                average_interval
                    .map_or_else(|| "-".to_string(), |interval| format!("{interval:.2} s")),
            ),
            KeyValue::new(
                "module.system_live.cpu_load",
                percent_string(last.cpu_load_percent),
            ),
            KeyValue::new(
                "module.system_live.cpu_frequency",
                last.cpu_frequency_mhz
                    .map_or_else(|| "-".to_string(), |mhz| format!("{mhz:.0} MHz")),
            ),
            KeyValue::new(
                "module.system_live.ram_used",
                percent_string(last.ram_used_percent),
            ),
        ])];

        let cpu_points = points(&window, |sample| sample.cpu_load_percent);
        if cpu_points.len() >= 2 {
            blocks.push(Block::chart(
                Chart::new("module.system_live.chart.cpu", "s", "%")
                    .with_series(vec![ChartSeries::new("CPU", cpu_points)]),
            ));
        }

        let ram_points = points(&window, |sample| sample.ram_used_percent);
        if ram_points.len() >= 2 {
            blocks.push(Block::chart(
                Chart::new("module.system_live.chart.ram", "s", "%")
                    .with_series(vec![ChartSeries::new("RAM", ram_points)]),
            ));
        }

        let frequency_points = points(&window, |sample| sample.cpu_frequency_mhz);
        if frequency_points.len() >= 2 {
            blocks.push(Block::chart(
                Chart::new("module.system_live.chart.frequency", "s", "MHz")
                    .with_series(vec![ChartSeries::new("CPU", frequency_points)]),
            ));
        }

        blocks.push(Block::note(
            NoteLevel::Info,
            "module.system_live.note.interval",
        ));

        let view = View::new(vec![
            Section::new("module.system_live.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "sample_count": samples.len(),
            "window_samples": window.len(),
            "nominal_interval_secs": 1,
            "avg_interval_secs": average_interval,
            "samples": window
                .iter()
                .map(|sample| serde_json::json!({
                    "t": sample.elapsed_secs,
                    "cpu_load_percent": sample.cpu_load_percent,
                    "cpu_frequency_mhz": sample.cpu_frequency_mhz,
                    "ram_used_percent": sample.ram_used_percent,
                }))
                .collect::<Vec<_>>(),
        });

        Ok(ModuleOutput { data, view })
    }
}

fn points(window: &[LiveSample], field: impl Fn(&LiveSample) -> Option<f64>) -> Vec<[f64; 2]> {
    window
        .iter()
        .filter_map(|sample| field(sample).map(|value| [sample.elapsed_secs, value]))
        .collect()
}

fn percent_string(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |percent| format!("{percent:.0} %"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockLive};

    fn samples(count: usize) -> Vec<LiveSample> {
        (0..count)
            .map(|index| {
                LiveSample::new(index as f64)
                    .with_cpu_load(10.0 + index as f64)
                    .with_cpu_frequency(3000.0 + index as f64)
                    .with_ram_used(40.0 + index as f64)
            })
            .collect()
    }

    fn harness(count: usize) -> Harness {
        Harness::new().with_live(MockLive::new().with_samples(samples(count)))
    }

    #[test]
    fn empty_history_asks_for_data_instead_of_failing() {
        let output = Live
            .collect(&Harness::new().ctx())
            .expect("brak próbek to nie błąd");
        assert_eq!(output.data["sample_count"], 0);
        assert!(output.view.uses_key("module.system_live.note.waiting"));
    }

    #[test]
    fn single_sample_does_not_draw_chart() {
        let output = Live.collect(&harness(1).ctx()).expect("zbieranie danych");
        assert!(!output.view.uses_key("module.system_live.chart.cpu"));
        assert!(output.view.uses_key("module.system_live.cpu_load"));
    }

    #[test]
    fn builds_three_charts_from_history() {
        let output = Live.collect(&harness(5).ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.system_live.chart.cpu"));
        assert!(output.view.uses_key("module.system_live.chart.ram"));
        assert!(output.view.uses_key("module.system_live.chart.frequency"));
        assert_eq!(output.data["window_samples"], 5);
    }

    #[test]
    fn chart_points_carry_time_and_value() {
        let output = Live.collect(&harness(3).ctx()).expect("zbieranie danych");
        let chart = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Chart { chart } if chart.title_key == "module.system_live.chart.cpu" => {
                    Some(chart)
                }
                _ => None,
            })
            .expect("wykres CPU");
        assert_eq!(chart.series.len(), 1);
        assert_eq!(chart.series[0].points.len(), 3);
        assert_eq!(chart.series[0].points[2], [2.0, 12.0]);
        assert_eq!(chart.y_label, "%");
    }

    #[test]
    fn caps_chart_to_last_samples() {
        let output = Live
            .collect(&harness(MAX_CHART_POINTS + 50).ctx())
            .expect("zbieranie danych");
        assert_eq!(output.data["sample_count"], MAX_CHART_POINTS + 50);
        assert_eq!(output.data["window_samples"], MAX_CHART_POINTS);
        let first = &output.data["samples"][0]["t"];
        assert_eq!(first, 50.0);
    }

    #[test]
    fn reports_measured_sample_interval() {
        let output = Live.collect(&harness(5).ctx()).expect("zbieranie danych");
        assert_eq!(output.data["avg_interval_secs"], 1.0);
        assert_eq!(output.data["nominal_interval_secs"], 1);
        assert!(output.view.uses_key("module.system_live.interval"));
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        for harness in [Harness::new(), harness(3)] {
            let output = Live.collect(&harness.ctx()).expect("zbieranie danych");
            for key in output.view.translation_keys() {
                for locale in Locale::ALL {
                    assert!(
                        translator.has(locale, &key),
                        "brak klucza {key} w języku {}",
                        locale.code()
                    );
                }
            }
        }
    }
}
