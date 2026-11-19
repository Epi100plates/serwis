//! Konsola i okna komunikatów.
//!
//! Wydanie ma podsystem GUI (`#![windows_subsystem = "windows"]` w `sk-app`),
//! więc Windows nie tworzy okna konsoli obok aplikacji. Dwa zachowania wymagają
//! wtedy obsługi:
//!
//! * tryb CLI (`serwiskit report …`) musi mieć gdzie wydrukować wynik — dlatego
//!   [`ensure_console`] dołącza do konsoli, z której proces uruchomiono, a gdy
//!   to się nie uda (np. podniesienie przez UAC tworzy nowy proces bez konsoli
//!   rodzica) tworzy własną, tak jak dotąd;
//! * błąd startu GUI nie ma gdzie się wypisać — dlatego [`show_error`] pokazuje
//!   go okienkiem.

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetConsoleWindow, GetStdHandle,
    STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_TOPMOST, MessageBoxW};
use windows::core::PCWSTR;

/// Zapewnia, że `stdout` i `stderr` mają dokąd pisać, i zwraca `true`, gdy tak jest.
///
/// Kolejność prób jest istotna: przekierowanie wyjścia (`serwiskit report … >
/// log.txt`) daje poprawne uchwyty już na starcie i wtedy **nic** nie zmieniamy —
/// inaczej wydruk uciekłby do konsoli zamiast do pliku. Dopiero gdy uchwyty są
/// puste, dołączamy do konsoli rodzica, a w ostateczności tworzymy własną.
pub fn ensure_console() -> bool {
    if std_handle_is_open(STD_OUTPUT_HANDLE) && std_handle_is_open(STD_ERROR_HANDLE) {
        return true;
    }
    if console_window_is_open() {
        return true;
    }

    // SAFETY: wywołania nie przyjmują wskaźników; `bind_console_handles` samo
    // pilnuje, żeby nie podmienić uchwytów przekierowanych do pliku.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() || AllocConsole().is_ok() {
            bind_console_handles();
        }
    }

    std_handle_is_open(STD_OUTPUT_HANDLE) || std_handle_is_open(STD_ERROR_HANDLE)
}

/// Pokazuje modalne okno z komunikatem błędu.
///
/// Wyłącznie dla buildów bez konsoli: `MB_OK | MB_ICONERROR` wystarcza, żeby
/// użytkownik zobaczył, dlaczego aplikacja się nie uruchomiła.
pub fn show_error(title: &str, message: &str) {
    let text = wide(message);
    let caption = wide(title);

    // SAFETY: oba bufory są zakończone zerem i żyją dłużej niż wywołanie;
    // `MessageBoxW` nie trzyma wskaźników po powrocie.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONERROR | MB_TOPMOST,
        );
    }
}

/// Przypina `stdout`/`stderr` do świeżo dołączonej konsoli.
///
/// SAFETY: wywołujący musi mieć konsolę (po `AttachConsole`/`AllocConsole`).
/// Uchwyt `CONOUT$` zostaje otwarty do końca życia procesu — zamykać go nie
/// wolno, bo wskazuje na niego `stdout`.
unsafe fn bind_console_handles() {
    let opened = unsafe {
        CreateFileW(
            windows::core::w!("CONOUT$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let Ok(console) = opened else {
        return;
    };

    for std in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        if !std_handle_is_open(std) {
            unsafe {
                let _ = SetStdHandle(std, console);
            }
        }
    }
}

/// Czy dany uchwyt standardowy istnieje (NULL i `INVALID_HANDLE_VALUE` to brak).
fn std_handle_is_open(std: windows::Win32::System::Console::STD_HANDLE) -> bool {
    // SAFETY: `GetStdHandle` czyta wyłącznie stan procesu.
    match unsafe { GetStdHandle(std) } {
        Ok(handle) => !handle.is_invalid(),
        Err(_) => false,
    }
}

/// Czy proces ma już okno konsoli (build deweloperski albo uruchomienie z terminala).
fn console_window_is_open() -> bool {
    // SAFETY: `GetConsoleWindow` zwraca uchwyt okna albo NULL i nic nie zmienia.
    !unsafe { GetConsoleWindow() }.is_invalid()
}

/// Serializuje tytuł okna do formatu wymaganego przez `MessageBoxW`.
///
/// Wydzielone, żeby test mógł sprawdzić zakończenie zerem bez pokazywania okna.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_console_keeps_redirected_handles() {
        // Testy `cargo test` mają uchwyty `stdout`/`stderr` przekierowane przez
        // cargo, więc funkcja musi zwrócić `true` bez tworzenia okna konsoli.
        assert!(ensure_console());
        assert!(ensure_console(), "drugie wywołanie też jest bezpieczne");
    }

    #[test]
    fn wide_text_ends_with_nul() {
        let text = wide("Raport zapisany");
        assert_eq!(text.last(), Some(&0));
        assert_eq!(text.len(), "Raport zapisany".encode_utf16().count() + 1);
    }
}
