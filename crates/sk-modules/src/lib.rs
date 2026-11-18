//! Moduły SerwisKit.
//!
//! Każdy moduł to jeden plik w `src/modules/`. Nowy moduł dodajesz w
//! `src/modules/mod.rs`: jedna linia `mod`, jedna linia `registry.add::<...>()`.
//! Instrukcja i checklista: `docs/MODULE_GUIDE.md`.

mod modules;

use sk_core::registry::ModuleRegistry;

/// Rejestr wszystkich modułów zbudowanych w tę binarkę.
pub fn registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    modules::register(&mut registry);
    registry.into_sorted()
}
