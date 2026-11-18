//! Manifest uprawnień wstawiamy tylko do buildu release.
//!
//! Wersja dla serwisu musi startować jako administrator (SMART i testy dysków
//! wymagają uchwytu do `\\.\PhysicalDriveN`), ale build deweloperski nie może
//! wywoływać UAC przy każdym `cargo run` i w testach.
//!
//! Używamy `/MANIFESTUAC`, a nie pliku `.manifest` z `/MANIFESTINPUT`: linker
//! dokłada własny fragment manifestu z `asInvoker` i scalanie dwóch fragmentów
//! kończy się błędem `manifest authoring error ... Values of attribute "level"
//! not equal`. `/MANIFESTUAC` zastępuje ten fragment, zamiast się z nim scalać.
//!
//! Świadomość DPI ustawia w runtime winit (używa `SetProcessDpiAwarenessContext`),
//! a `longPathAware` dołożymy przez zasób RT_MANIFEST dopiero wtedy, gdy migracja
//! plików (Faza 4) zacznie pracować na ścieżkach dłuższych niż 260 znaków.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }

    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
}
