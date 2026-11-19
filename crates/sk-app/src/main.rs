//! SerwisKit — punkt wejścia.
//!
//! Bez argumentów uruchamia GUI; z podkomendą działa w trybie konsolowym
//! (raport JSON i/lub HTML). Logika modułów jest wspólna dla obu trybów.

mod app;
mod bench;
mod cli;
mod html;
mod live;
mod render;
mod report;

#[cfg(test)]
mod tests_support;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("błąd: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}
