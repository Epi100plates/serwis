//! Lista modułów w tej kompilacji.
//!
//! Dodanie modułu = dwie linie w tym pliku:
//! 1. `mod nazwa_pliku;`
//! 2. `registry.add::<nazwa_pliku::NazwaTypu>();` w funkcji `register`.
//!
//! Dopisuj alfabetycznie. Przy konflikcie scalania zachowaj obie pary linii.

use sk_core::registry::ModuleRegistry;

mod system_info;

pub(crate) fn register(registry: &mut ModuleRegistry) {
    registry.add::<system_info::SystemInfo>();
}
