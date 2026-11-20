//! Klient podpisanego sterownika PawnIO — jedyna droga do odczytu MSR, SMN
//! i Super I/O bez własnego sterownika jądra.
//!
//! Sterownik instaluje technik (pawnio.eu); aplikacja go **nie** pobiera ani nie
//! instaluje — brak sterownika to brak odczytów, nie błąd modułu. Moduły PawnIO
//! (`.bin`, LGPL-2.1) są wkompilowane, bo instalator sterownika ich nie zawiera,
//! a podpisany sterownik wykonuje wyłącznie podpisane moduły.
//!
//! API biblioteki: `PawnIOLib.h` z instalacji PawnIO (`pawnio_open`,
//! `pawnio_load`, `pawnio_execute`, `pawnio_close`). Wywołania zwracają HRESULT.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;

use sk_core::error::{ModuleError, Result};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::core::{PCSTR, PCWSTR};

use crate::ffi::symbol;

/// Podpisane moduły PawnIO (wydanie 0.2.12, LGPL-2.1; treść licencji w
/// `assets/pawnio/COPYING`).
pub const AMD_FAMILY17: &[u8] = include_bytes!("../assets/pawnio/AMDFamily17.bin");
pub const INTEL_MSR: &[u8] = include_bytes!("../assets/pawnio/IntelMSR.bin");
pub const LPC_IO: &[u8] = include_bytes!("../assets/pawnio/LpcIO.bin");

const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO";
const DEFAULT_LIBRARY: &str = r"C:\Program Files\PawnIO\PawnIOLib.dll";

type OpenFn = unsafe extern "system" fn(*mut *mut std::ffi::c_void) -> i32;
type LoadFn = unsafe extern "system" fn(*mut std::ffi::c_void, *const u8, usize) -> i32;
type ExecuteFn = unsafe extern "system" fn(
    *mut std::ffi::c_void,
    PCSTR,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type CloseFn = unsafe extern "system" fn(*mut std::ffi::c_void) -> i32;

/// Ścieżka `PawnIOLib.dll` z rejestru instalatora, z fallbackiem na katalog
/// domyślny. `None` = sterownik nie jest zainstalowany.
fn library_path() -> Option<PathBuf> {
    if let Some(directory) = crate::registry::read_string(UNINSTALL_KEY, "InstallLocation") {
        let path = Path::new(directory.trim_end_matches(['\\', '/'])).join("PawnIOLib.dll");
        if path.is_file() {
            return Some(path);
        }
    }
    let fallback = PathBuf::from(DEFAULT_LIBRARY);
    fallback.is_file().then_some(fallback)
}

/// Czy sterownik PawnIO jest zainstalowany (sama obecność biblioteki; działanie
/// potwierdza dopiero `PawnIo::load_module`).
pub fn is_installed() -> bool {
    library_path().is_some()
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn hr_error(call: &str, hr: i32) -> ModuleError {
    let code = hr as u32;
    if code == 0x8007_0005 {
        ModuleError::access_denied(format!("PawnIO: {call}"))
    } else {
        ModuleError::provider("PawnIO", format!("{call}: HRESULT 0x{code:08X}"))
    }
}

/// Sesja PawnIO z jednym załadowanym modułem.
///
/// Uchwyt sterownika jest używany wyłącznie z wątku, który sesję utworzył
/// (wątek roboczy aplikacji albo CLI), więc typ nie jest `Send`/`Sync`.
pub struct PawnIo {
    module: HMODULE,
    handle: *mut std::ffi::c_void,
    execute: ExecuteFn,
    close: CloseFn,
}

impl PawnIo {
    /// Ładuje bibliotekę, otwiera sesję i wgrywa moduł `blob`.
    pub fn load_module(blob: &[u8]) -> Result<Self> {
        let path = library_path()
            .ok_or_else(|| ModuleError::unavailable("sterownik PawnIO (pawnio.eu)"))?;
        let wide = wide(&path);
        // SAFETY: ścieżka zakończona NUL i żyje do końca wywołania.
        let module = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }
            .map_err(|error| ModuleError::provider("PawnIO", error.message()))?;

        let result = Self::open(module, blob);
        if result.is_err() {
            // SAFETY: biblioteka została załadowana i nie jest już potrzebna.
            unsafe {
                let _ = FreeLibrary(module);
            }
        }
        result
    }

    fn open(module: HMODULE, blob: &[u8]) -> Result<Self> {
        let open: OpenFn = symbol(module, "pawnio_open")
            .ok_or_else(|| ModuleError::provider("PawnIO", "brak pawnio_open"))?;
        let load: LoadFn = symbol(module, "pawnio_load")
            .ok_or_else(|| ModuleError::provider("PawnIO", "brak pawnio_load"))?;
        let execute: ExecuteFn = symbol(module, "pawnio_execute")
            .ok_or_else(|| ModuleError::provider("PawnIO", "brak pawnio_execute"))?;
        let close: CloseFn = symbol(module, "pawnio_close")
            .ok_or_else(|| ModuleError::provider("PawnIO", "brak pawnio_close"))?;

        let mut handle: *mut std::ffi::c_void = ptr::null_mut();
        // SAFETY: `open` pochodzi z załadowanej biblioteki, a `handle` to bufor
        // wyjściowy o prawidłowym typie.
        let hr = unsafe { open(&mut handle) };
        if hr < 0 || handle.is_null() {
            return Err(hr_error("pawnio_open", hr));
        }

        // SAFETY: `handle` jest prawidłowy; `blob` żyje w pamięci statycznej
        // (`include_bytes!`), a jego długość jest przekazana jawnie.
        let hr = unsafe { load(handle, blob.as_ptr(), blob.len()) };
        if hr < 0 {
            // SAFETY: sesja została otwarta, ale modułu nie udało się wgrać.
            unsafe {
                let _ = close(handle);
            }
            return Err(hr_error("pawnio_load", hr));
        }

        Ok(Self {
            module,
            handle,
            execute,
            close,
        })
    }

    /// Woła funkcję modułu. `args` to argumenty całkowite, `out_len` — oczekiwana
    /// liczba wyników (moduł zwraca mniej, gdy funkcja nic nie pisze).
    pub fn execute(&self, name: &str, args: &[u64], out_len: usize) -> Result<Vec<u64>> {
        let c_name =
            CString::new(name).map_err(|_| ModuleError::internal("pusta nazwa funkcji PawnIO"))?;
        let mut out = vec![0u64; out_len];
        let mut returned: usize = 0;

        // SAFETY: `c_name` jest zakończone NUL; bufory wejścia i wyjścia są
        // prawidłowe, a ich długości zgadzają się z przekazanymi wartościami.
        let hr = unsafe {
            (self.execute)(
                self.handle,
                PCSTR(c_name.as_ptr().cast()),
                args.as_ptr(),
                args.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut returned,
            )
        };
        if hr < 0 {
            return Err(hr_error(name, hr));
        }
        out.truncate(returned.min(out_len));
        Ok(out)
    }

    /// Funkcja zwracająca dokładnie jedną wartość.
    pub fn execute_one(&self, name: &str, args: &[u64]) -> Result<u64> {
        let out = self.execute(name, args, 1)?;
        out.first()
            .copied()
            .ok_or_else(|| ModuleError::provider("PawnIO", format!("{name}: brak wyniku")))
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        // SAFETY: `handle` należy do tej sesji i nie był zamykany wcześniej.
        unsafe {
            let _ = (self.close)(self.handle);
            let _ = FreeLibrary(self.module);
        }
    }
}
