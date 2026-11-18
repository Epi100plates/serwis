//! Raport: zebranie danych ze wszystkich modułów i zapis JSON/HTML.
//!
//! Zbieranie odbywa się **raz**: ten sam `Report` daje JSON (maszynowy)
//! i HTML (do wydruku dla klienta), więc oba pliki zawsze zawierają te same
//! dane. Ten sam kod obsługuje CLI i GUI.

use std::path::Path;

use chrono::{DateTime, Local};
use serde_json::{Value, json};
use sk_core::i18n::Translator;
use sk_core::module::{CollectCtx, ModuleMeta, ModuleOutput};
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

/// Udany moduł razem z metadanymi — HTML potrzebuje nazwy i grupy.
pub struct ModuleEntry {
    pub meta: ModuleMeta,
    pub output: ModuleOutput,
}

/// Moduł, który nie zdołał zebrać danych. Nie przerywa raportu — trafia do
/// sekcji błędów, żeby technik wiedział, czego brakuje i dlaczego.
pub struct ModuleFailure {
    pub module: &'static str,
    pub error: String,
}

/// Wynik jednego przebiegu zbierania.
pub struct Report {
    pub app_version: &'static str,
    pub generated_at: String,
    pub locale_code: &'static str,
    pub elevated: bool,
    pub uptime_secs: u64,
    pub modules: Vec<ModuleEntry>,
    pub errors: Vec<ModuleFailure>,
}

impl Report {
    pub fn collect(
        registry: &ModuleRegistry,
        ctx: &CollectCtx<'_>,
        options: &ReportOptions<'_>,
    ) -> Self {
        let generated_at = DateTime::<Local>::from(ctx.clock.now()).to_rfc3339();
        let mut modules = Vec::new();
        let mut errors = Vec::new();

        for module in registry.modules() {
            let meta = module.meta();

            if let Some(filter) = options.module_filter
                && meta.id != filter
            {
                continue;
            }

            match module.collect(ctx) {
                Ok(output) => modules.push(ModuleEntry { meta, output }),
                Err(error) => errors.push(ModuleFailure {
                    module: meta.id,
                    error: error.to_string(),
                }),
            }
        }

        Self {
            app_version: options.app_version,
            generated_at,
            locale_code: ctx.locale.code(),
            elevated: ctx.host.is_elevated(),
            uptime_secs: ctx.clock.uptime_secs(),
            modules,
            errors,
        }
    }

    pub fn to_json(&self) -> Value {
        let modules: Vec<Value> = self
            .modules
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.meta.id,
                    "group": entry.meta.group,
                    "capability": entry.meta.capability,
                    "experimental": entry.meta.experimental,
                    "data": entry.output.data,
                    "view": entry.output.view,
                })
            })
            .collect();

        let errors: Vec<Value> = self
            .errors
            .iter()
            .map(|failure| {
                json!({
                    "module": failure.module,
                    "error": failure.error,
                })
            })
            .collect();

        json!({
            "schema_version": REPORT_SCHEMA_VERSION,
            "app": {
                "name": "SerwisKit",
                "version": self.app_version,
            },
            "generated_at": self.generated_at,
            "locale": self.locale_code,
            "host": {
                "elevated": self.elevated,
                "uptime_secs": self.uptime_secs,
            },
            "modules": modules,
            "errors": errors,
        })
    }

    pub fn to_html(&self, translator: &Translator) -> String {
        crate::html::render(self, translator)
    }
}

/// Zapisuje raport JSON atomowo: najpierw plik tymczasowy, potem podmiana.
/// Dzięki temu przerwany zapis nie zostawia uszkodzonego raportu.
pub fn write_json(path: &Path, report: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|err| format!("nie udało się zserializować raportu: {err}"))?;
    write_atomic(path, &text)
}

pub fn write_html(path: &Path, html: &str) -> Result<(), String> {
    write_atomic(path, html)
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let mut temp_path = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("niepoprawna nazwa pliku: {}", path.display()))?;
    temp_path.set_file_name(format!("{file_name}.tmp"));

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
    use sk_core::testing::{Harness, MockLive, MockWmi};

    fn harness() -> Harness {
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
                    "Manufacturer": "Micro-Star International Co., Ltd.",
                    "Model": "MS-7C52",
                    "TotalPhysicalMemory": 34_281_689_088u64,
                    "HypervisorPresent": true,
                    "PCSystemType": 1
                })],
            );
        let live = MockLive::new().with_samples(vec![
            sk_core::live::LiveSample::new(0.0)
                .with_cpu_load(10.0)
                .with_ram_used(50.0),
            sk_core::live::LiveSample::new(1.0)
                .with_cpu_load(20.0)
                .with_ram_used(51.0),
        ]);
        Harness::new().with_wmi(wmi).with_live(live)
    }

    #[test]
    fn report_contains_schema_and_module_data() {
        let registry = sk_modules::registry();
        let harness = harness();
        let report =
            Report::collect(&registry, &harness.ctx(), &ReportOptions::default()).to_json();

        assert_eq!(report["schema_version"], REPORT_SCHEMA_VERSION);
        assert_eq!(report["app"]["name"], "SerwisKit");
        assert_eq!(report["locale"], "pl");

        let modules = report["modules"].as_array().expect("tablica modułów");
        let system_info = modules
            .iter()
            .find(|module| module["id"] == "system.info")
            .expect("moduł system.info w raporcie");
        assert_eq!(system_info["data"]["os"]["build_number"], "26300");
        assert_eq!(system_info["group"], "system");
    }

    #[test]
    fn module_filter_keeps_only_requested_module() {
        let registry = sk_modules::registry();
        let harness = harness();
        let options = ReportOptions {
            module_filter: Some("system.info"),
            ..ReportOptions::default()
        };
        let report = Report::collect(&registry, &harness.ctx(), &options).to_json();
        let modules = report["modules"].as_array().expect("tablica modułów");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0]["id"], "system.info");
    }

    #[test]
    fn failing_modules_land_in_errors_not_fatal() {
        let registry = sk_modules::registry();
        let harness = Harness::new();
        let report =
            Report::collect(&registry, &harness.ctx(), &ReportOptions::default()).to_json();
        let errors = report["errors"].as_array().expect("błędy");
        let modules = report["modules"].as_array().expect("moduły");
        // Pusty dostawca: moduły WMI zgłaszają błąd, ale raport powstaje,
        // a moduły degradujące (brak baterii, brak próbek) działają dalej.
        assert!(
            errors.iter().any(|error| error["module"] == "hw.cpu"),
            "brak wpisu błędu dla hw.cpu"
        );
        assert_eq!(errors.len() + modules.len(), registry.len());
        for degraded in ["system.live", "hw.battery"] {
            assert!(
                modules.iter().any(|module| module["id"] == degraded),
                "{degraded} powinien zwrócić widok degradacji, nie błąd"
            );
        }
    }

    #[test]
    fn writes_report_file() {
        let dir = std::env::temp_dir().join(format!("sk-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("katalog tymczasowy");
        let path = dir.join("report.json");

        let registry = sk_modules::registry();
        let harness = harness();
        let report =
            Report::collect(&registry, &harness.ctx(), &ReportOptions::default()).to_json();
        write_json(&path, &report).expect("zapis raportu");

        let written = std::fs::read_to_string(&path).expect("odczyt raportu");
        let parsed: Value = serde_json::from_str(&written).expect("poprawny JSON");
        assert_eq!(parsed["schema_version"], REPORT_SCHEMA_VERSION);

        std::fs::remove_dir_all(&dir).ok();
    }
}
