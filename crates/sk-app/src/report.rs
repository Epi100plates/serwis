//! Raport: zebranie danych ze wszystkich modułów i zapis JSON.
//!
//! Ten sam kod obsługuje CLI i GUI, więc raport z GUI i z konsoli mają
//! identyczny schemat.

use std::path::Path;

use chrono::{DateTime, Local};
use serde_json::{Value, json};
use sk_core::module::CollectCtx;
use sk_core::registry::ModuleRegistry;

/// Wersja schematu raportu. Podnosi się przy zmianie łamiącej odbiorców.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

pub struct ReportOptions<'a> {
    /// Tylko jeden moduł (id); `None` = wszystkie.
    pub module_filter: Option<&'a str>,
    pub app_version: &'static str,
}

impl Default for ReportOptions<'_> {
    fn default() -> Self {
        Self {
            module_filter: None,
            app_version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// Buduje raport jako JSON. Błędy modułów nie przerywają raportu — trafiają
/// do sekcji `errors`, żeby technik wiedział, czego brakuje.
pub fn build(
    registry: &ModuleRegistry,
    ctx: &CollectCtx<'_>,
    options: &ReportOptions<'_>,
) -> serde_json::Value {
    let generated_at = DateTime::<Local>::from(ctx.clock.now()).to_rfc3339();

    let mut modules: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();

    for module in registry.modules() {
        let meta = module.meta();

        if let Some(filter) = options.module_filter
            && meta.id != filter
        {
            continue;
        }

        match module.collect(ctx) {
            Ok(output) => modules.push(json!({
                "id": meta.id,
                "group": meta.group,
                "capability": meta.capability,
                "experimental": meta.experimental,
                "data": output.data,
                "view": output.view,
            })),
            Err(error) => errors.push(json!({
                "module": meta.id,
                "error": error.to_string(),
            })),
        }
    }

    json!({
        "schema_version": REPORT_SCHEMA_VERSION,
        "app": {
            "name": "SerwisKit",
            "version": options.app_version,
        },
        "generated_at": generated_at,
        "host": {
            "elevated": ctx.host.is_elevated(),
            "uptime_secs": ctx.clock.uptime_secs(),
        },
        "modules": modules,
        "errors": errors,
    })
}

/// Zapisuje raport atomowo: najpierw plik tymczasowy, potem podmiana.
/// Dzięki temu przerwany zapis nie zostawia uszkodzonego raportu.
pub fn write_json(path: &Path, report: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|err| format!("nie udało się zserializować raportu: {err}"))?;

    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, text)
        .map_err(|err| format!("nie udało się zapisać {}: {err}", temp_path.display()))?;
    std::fs::rename(&temp_path, path).map_err(|err| {
        format!(
            "nie udało się podmienić {} na {}: {err}",
            temp_path.display(),
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockWmi};

    fn harness_with_system_info() -> Harness {
        let wmi = MockWmi::new()
            .with_rows(
                "Win32_OperatingSystem",
                vec![json!({
                    "Caption": "Microsoft Windows 11 Enterprise",
                    "Version": "10.0.26300",
                    "BuildNumber": "26300",
                    "OSArchitecture": "64-bit"
                })],
            )
            .with_rows(
                "Win32_ComputerSystem",
                vec![json!({
                    "Manufacturer": "ASUSTeK COMPUTER INC.",
                    "Model": "PRIME B450M-A",
                    "TotalPhysicalMemory": 34_359_738_368u64
                })],
            );
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn report_contains_schema_and_module_data() {
        let registry = sk_modules::registry();
        let harness = harness_with_system_info();
        let report = build(&registry, &harness.ctx(), &ReportOptions::default());

        assert_eq!(report["schema_version"], REPORT_SCHEMA_VERSION);
        assert_eq!(report["app"]["name"], "SerwisKit");

        let modules = report["modules"].as_array().expect("tablica modułów");
        let system_info = modules
            .iter()
            .find(|module| module["id"] == "system.info")
            .expect("moduł system.info w raporcie");
        assert_eq!(system_info["data"]["os"]["build_number"], "26300");
        assert_eq!(system_info["group"], "system");
        assert_eq!(
            report["errors"].as_array().expect("tablica błędów").len(),
            0
        );
    }

    #[test]
    fn module_filter_keeps_only_requested_module() {
        let registry = sk_modules::registry();
        let harness = harness_with_system_info();
        let options = ReportOptions {
            module_filter: Some("system.info"),
            ..ReportOptions::default()
        };
        let report = build(&registry, &harness.ctx(), &options);
        let modules = report["modules"].as_array().expect("tablica modułów");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0]["id"], "system.info");
    }

    #[test]
    fn failing_module_lands_in_errors_not_fatal() {
        let registry = sk_modules::registry();
        let harness = Harness::new(); // brak danych WMI
        let report = build(&registry, &harness.ctx(), &ReportOptions::default());
        assert_eq!(report["modules"].as_array().expect("moduły").len(), 0);
        let errors = report["errors"].as_array().expect("błędy");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["module"], "system.info");
    }

    #[test]
    fn writes_report_file() {
        let dir = std::env::temp_dir().join(format!("sk-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("katalog tymczasowy");
        let path = dir.join("report.json");

        let registry = sk_modules::registry();
        let harness = harness_with_system_info();
        let report = build(&registry, &harness.ctx(), &ReportOptions::default());
        write_json(&path, &report).expect("zapis raportu");

        let written = std::fs::read_to_string(&path).expect("odczyt raportu");
        let parsed: Value = serde_json::from_str(&written).expect("poprawny JSON");
        assert_eq!(parsed["schema_version"], REPORT_SCHEMA_VERSION);

        std::fs::remove_dir_all(&dir).ok();
    }
}
