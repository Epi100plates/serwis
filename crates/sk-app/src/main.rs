//! SerwisKit — punkt wejścia.
//!
//! Bez argumentów uruchamia GUI; z podkomendą działa w trybie konsolowym
//! (raport JSON i/lub HTML). Logika modułów jest wspólna dla obu trybów.
//!
//! Podsystem GUI włączamy **tylko w wydaniu**: wtedy Windows nie tworzy okna
//! cmd obok aplikacji. Build deweloperski zostaje przy konsoli, żeby `cargo run`
//! i komunikaty paniki były widoczne. Tryb CLI w wydaniu sam dba o konsolę
//! (`sk_win::ensure_console`), a błąd, którego nie ma gdzie wypisać, pokazuje
//! okienkiem (`sk_win::show_error`).

#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

mod app;
mod bench;
mod chart_image;
mod cli;
mod html;
mod live;
mod render;
mod report;
mod sector;

#[cfg(test)]
mod tests_support;

/// Nazwa produktu — ta sama w obu językach i w tytule okna GUI.
const APP_NAME: &str = "SerwisKit";

fn main() -> std::process::ExitCode {
    // Brak argumentu = GUI; cokolwiek innego (podkomenda, `--help`) to CLI.
    let gui_mode = std::env::args_os().nth(1).is_none();
    // W CLI upewniamy się, że jest gdzie pisać; w GUI każde wyjście na konsolę
    // byłoby błędem, bo w wydaniu jej nie ma.
    let console = !gui_mode && sk_win::ensure_console();

    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            // Bez konsoli (`eprintln!` panikuje na błędzie zapisu) zostaje okienko.
            if console {
                eprintln!("błąd: {message}");
            } else {
                sk_win::show_error(APP_NAME, &message);
            }
            std::process::ExitCode::FAILURE
        }
    }
}
