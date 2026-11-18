//! SerwisKit — punkt wejścia.
//!
//! Bez argumentów uruchamia GUI; z podkomendą działa w trybie konsolowym
//! (raport JSON). Logika modułów jest wspólna dla obu trybów.

mod app;
mod cli;
mod render;
mod report;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("błąd: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}
