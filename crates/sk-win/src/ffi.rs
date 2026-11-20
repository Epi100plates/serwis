//! Wspólne helpery FFI dla warstwy Windows.
//!
//! Wyłącznie drobne funkcje, które inaczej byłyby kopiowane w każdym transporcie
//! (ADL, NVML, PawnIO).

use std::ffi::CString;
use std::mem::size_of;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::core::PCSTR;

/// Wskaźnik na funkcję z biblioteki DLL załadowanej przez `LoadLibraryW`.
///
/// Wołający podaje typ zgodny z deklaracją w nagłówku biblioteki; wszystkie
/// wskaźniki funkcji mają ten sam rozmiar co `FARPROC`.
pub(crate) fn symbol<T: Copy>(module: HMODULE, name: &str) -> Option<T> {
    let name = CString::new(name).ok()?;
    // SAFETY: `name` jest zakończone NUL; uchwyt biblioteki jest prawidłowy.
    let procedure = unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }?;
    debug_assert_eq!(size_of::<T>(), size_of_val(&procedure));
    // SAFETY: rozmiar wskaźnika funkcji jest sprawdzony powyżej, a wołający
    // deklaruje sygnaturę zgodną z nagłówkiem biblioteki.
    Some(unsafe { std::mem::transmute_copy(&procedure) })
}
