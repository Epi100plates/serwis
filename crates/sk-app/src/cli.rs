//! Tryb konsolowy i wybór trybu pracy.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use sk_core::i18n::{Locale, Translator};
use sk_core::module::CollectCtx;
use sk_core::surface::IdleSurface;
use sk_win::{SystemClock, WinHostEnv, WinMachine, WinSensors, WinStorage, WinWmi};

use crate::report::{self, Report, ReportOptions};

#[derive(Debug, Parser)]
#[command(
    name = "serwiskit",
    version,
    about = "SerwisKit — diagnostyka sprzętu, testy dysków i migracja danych dla serwisu komputerowego",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Zbiera dane modułów i zapisuje raport JSON i/lub HTML (tryb bez GUI).
    Report(ReportArgs),
}

#[derive(Debug, Args)]
struct ReportArgs {
    /// Ścieżka pliku wynikowego JSON.
    #[arg(long, value_name = "PLIK")]
    json: Option<PathBuf>,

    /// Ścieżka pliku wynikowego HTML (do wydruku lub wysłania klientowi).
    #[arg(long, value_name = "PLIK")]
    html: Option<PathBuf>,

    /// Tylko jeden moduł, np. `system.info`.
    #[arg(long, value_name = "ID")]
    module: Option<String>,

    /// Język raportu.
    #[arg(long, value_enum, default_value_t = LocaleArg::Pl)]
    locale: LocaleArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LocaleArg {
    Pl,
    En,
}

impl From<LocaleArg> for Locale {
    fn from(value: LocaleArg) -> Self {
        match value {
            LocaleArg::Pl => Locale::Pl,
            LocaleArg::En => Locale::En,
        }
    }
}

/// Uruchamia aplikację: GUI bez argumentów, tryb konsolowy z podkomendą.
pub fn run() -> Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Report(args)) => run_report(args),
        None => crate::app::run_gui(),
    }
}

fn run_report(args: ReportArgs) -> Result<(), String> {
    if args.json.is_none() && args.html.is_none() {
        return Err(
            "podaj przynajmniej jedno wyjście: --json <PLIK> lub --html <PLIK>".to_string(),
        );
    }

    let registry = sk_modules::registry();

    if let Some(filter) = args.module.as_deref()
        && registry.find(filter).is_none()
    {
        let available: Vec<&str> = registry
            .modules()
            .iter()
            .map(|module| module.meta().id)
            .collect();
        return Err(format!(
            "nie ma modułu `{filter}`; dostępne: {}",
            available.join(", ")
        ));
    }

    let locale: Locale = args.locale.into();
    let wmi = WinWmi::new();
    let clock = SystemClock::new();
    let host = WinHostEnv::new();
    let machine = WinMachine::new();
    let storage = WinStorage::new();
    let live = sk_core::live::LiveHistory::new(1);
    let sensor_probe = WinSensors::new();
    let mut sensors = sk_core::sensors::SensorStore::new(1);
    let sectors = sk_core::sector::IdleSectors;
    // Benchmark nie jest częścią raportu: trwa minuty i wymaga jawnego startu
    // w GUI, dlatego raport widzi stan bezczynny.
    let surface = IdleSurface;
    wmi.begin_round();
    sensors.push(0.0, sensor_probe.readings());

    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        machine: &machine,
        storage: &storage,
        live: &live,
        sensors: &sensors,
        sectors: &sectors,
        surface: &surface,
        locale,
    };

    let options = ReportOptions {
        module_filter: args.module.as_deref(),
        app_version: env!("CARGO_PKG_VERSION"),
    };
    let report =
        Report::collect(&registry, &ctx, &options).with_wmi_stats(wmi.cache_stats().into());

    if let Some(path) = args.json.as_deref() {
        report::write_json(path, &report.to_json())?;
    }
    if let Some(path) = args.html.as_deref() {
        let mut translator = Translator::load().map_err(|err| format!("tłumaczenia: {err}"))?;
        translator.set_locale(locale);
        report::write_html(path, &report.to_html(&translator))?;
    }

    let mut targets: Vec<String> = Vec::new();
    if let Some(path) = args.json {
        targets.push(path.display().to_string());
    }
    if let Some(path) = args.html {
        targets.push(path.display().to_string());
    }
    println!(
        "Raport zapisany: {} (modułów: {}, błędów: {})",
        targets.join(", "),
        report.modules.len(),
        report.errors.len()
    );

    for failure in &report.errors {
        println!("  ! {}: {}", failure.module, failure.error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_report_subcommand_with_json_path() {
        let cli = Cli::try_parse_from(["serwiskit", "report", "--json", "raport.json"])
            .expect("poprawne argumenty");
        match cli.command {
            Some(Command::Report(args)) => {
                assert_eq!(args.json, Some(PathBuf::from("raport.json")));
                assert!(args.html.is_none());
                assert!(args.module.is_none());
                assert!(matches!(args.locale, LocaleArg::Pl));
            }
            None => panic!("oczekiwano podkomendy report"),
        }
    }

    #[test]
    fn parses_html_output_and_module_filter() {
        let cli = Cli::try_parse_from([
            "serwiskit",
            "report",
            "--html",
            "raport.html",
            "--module",
            "hw.cpu",
            "--locale",
            "en",
        ])
        .expect("poprawne argumenty");
        match cli.command {
            Some(Command::Report(args)) => {
                assert_eq!(args.html, Some(PathBuf::from("raport.html")));
                assert_eq!(args.module.as_deref(), Some("hw.cpu"));
                assert_eq!(Locale::from(args.locale), Locale::En);
            }
            None => panic!("oczekiwano podkomendy report"),
        }
    }

    #[test]
    fn no_subcommand_means_gui() {
        let cli = Cli::try_parse_from(["serwiskit"]).expect("brak argumentów jest poprawny");
        assert!(cli.command.is_none());
    }

    #[test]
    fn requires_at_least_one_output() {
        let error = run_report(ReportArgs {
            json: None,
            html: None,
            module: Some("system.info".to_string()),
            locale: LocaleArg::Pl,
        })
        .expect_err("brak wyjścia to błąd");
        assert!(error.contains("--json"), "{error}");
    }

    #[test]
    fn rejects_unknown_module() {
        let error = run_report(ReportArgs {
            json: Some(PathBuf::from("nie-uzywany.json")),
            html: None,
            module: Some("nie.ma.takiego".to_string()),
            locale: LocaleArg::Pl,
        })
        .expect_err("nieznany moduł to błąd");
        assert!(error.contains("nie.ma.takiego"), "{error}");
        assert!(
            error.contains("system.info"),
            "błąd powinien podać dostępne moduły: {error}"
        );
    }
}
