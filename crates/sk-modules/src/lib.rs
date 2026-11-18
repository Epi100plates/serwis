//! Moduły SerwisKit.
//!
//! Każdy moduł to jeden plik w `src/modules/`. Nowy moduł dodajesz w
//! `src/modules/mod.rs`: jedna linia `mod`, jedna linia `registry.add::<...>()`.
//! Instrukcja i checklista: `docs/MODULE_GUIDE.md`.

/// Wczytuje fixture odpowiedzi WMI z `docs/fixtures/wmi/` jako wiersze JSON.
/// Fixtures to prawdziwe odpowiedzi z maszyny serwisowej (po anonimizacji).
#[cfg(test)]
macro_rules! fixture_rows {
    ($name:literal) => {
        serde_json::from_str::<Vec<serde_json::Value>>(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/",
            $name
        )))
        .expect("fixture WMI musi być poprawnym JSON-em")
    };
}

mod edid;
mod modules;
mod smart;
mod util;

use sk_core::registry::ModuleRegistry;

/// Rejestr wszystkich modułów zbudowanych w tę binarkę.
pub fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    modules::register(&mut registry);
    registry.into_sorted()
}
