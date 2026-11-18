use sk_core::providers::HostEnv;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Sprawdza, czy proces działa z podniesionymi uprawnieniami.
#[derive(Debug, Default)]
pub struct WinHostEnv;

impl WinHostEnv {
    pub fn new() -> Self {
        Self
    }
}

impl HostEnv for WinHostEnv {
    fn is_elevated(&self) -> bool {
        let mut token = HANDLE::default();
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned_size = 0u32;

        // SAFETY: uchwyt procesu jest ważny przez cały czas życia procesu;
        // `elevation` to poprawnie zainicjalizowana struktura o znanym rozmiarze,
        // a jej wskaźnik żyje dłużej niż wywołanie GetTokenInformation.
        unsafe {
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }

            let result = GetTokenInformation(
                token,
                TokenElevation,
                Some(std::ptr::from_mut(&mut elevation).cast()),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned_size,
            );

            let _ = CloseHandle(token);
            let _ = returned_size;

            result.is_ok() && elevation.TokenIsElevated != 0
        }
    }
}
