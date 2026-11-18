//! Tryb konsolowy i wybór trybu pracy.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use sk_core::i18n::Locale;
use sk_core::module::CollectCtx;
use sk_win::{SystemClock, WinHostEnv, WinWmi};

use crate::report::{self, ReportOptions};

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
    /// Zbiera dane modułów i zapisuje raport JSON (tryb bez GUI).
    Report(ReportArgs),
}

#[derive(Debug, Args)]
struct ReportArgs {
    /// Ścieżka pliku wynikowego JSON.
    #[arg(long, value_name = "PLIK")]
    json: PathBuf,

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

    let wmi = WinWmi::new();
    let clock = SystemClock::new();
    let host = WinHostEnv::new();
    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        locale: args.locale.into(),
    };

    let options = ReportOptions {
        module_filter: args.module.as_deref(),
        app_version: env!("CARGO_PKG_VERSION"),
    };
    let report = report::build(&registry, &ctx, &options);
    report::write_json(&args.json, &report)?;

    let module_count = report["modules"].as_array().map_or(0, Vec::len);
    let error_count = report["errors"].as_array().map_or(0, Vec::len);
    println!(
        "Raport zapisany: {} (modułów: {module_count}, błędów: {error_count})",
        args.json.display()
    );

    if let Some(errors) = report["errors"].as_array() {
        for error in errors {
            println!(
                "  ! {}: {}",
                error["module"].as_str().unwrap_or("?"),
                error["error"]
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_report_subcommand_with_path() {
        let cli = Cli::try_parse_from(["serwiskit", "report", "--json", "raport.json"])
            .expect("poprawne argumenty");
        match cli.command {
            Some(Command::Report(args)) => {
                assert_eq!(args.json, PathBuf::from("raport.json"));
                assert!(args.module.is_none());
                assert!(matches!(args.locale, LocaleArg::Pl));
            }
            None => panic!("oczekiwano podkomendy report"),
        }
    }

    #[test]
    fn parses_module_and_locale_flags() {
        let cli = Cli::try_parse_from([
            "serwiskit",
            "report",
            "--json",
            "out.json",
            "--module",
            "system.info",
            "--locale",
            "en",
        ])
        .expect("poprawne argumenty");
        match cli.command {
            Some(Command::Report(args)) => {
                assert_eq!(args.module.as_deref(), Some("system.info"));
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
    fn rejects_unknown_module() {
        let error = run_report(ReportArgs {
            json: PathBuf::from("nie-uzywany.json"),
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
