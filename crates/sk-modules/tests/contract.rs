//! Testy kontraktu: sprawdzają każdy zarejestrowany moduł, także przyszły.
//! Nic tu nie dopisujesz przy dodawaniu modułu — ten plik ma zostać zielony.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use sk_core::i18n::{Locale, Translator};
use sk_core::testing::Harness;
use sk_modules::registry;

#[test]
fn registry_is_not_empty() {
    assert!(
        !registry().is_empty(),
        "żaden moduł nie jest zarejestrowany"
    );
}

#[test]
fn module_ids_are_unique_and_dotted() {
    let registry = registry();
    let mut seen = HashSet::new();
    for module in registry.modules() {
        let meta = module.meta();
        assert!(seen.insert(meta.id), "zduplikowane id modułu: {}", meta.id);
        assert!(
            meta.id.contains('.'),
            "id modułu musi być kropkowane: {}",
            meta.id
        );
        assert!(!meta.id.is_empty(), "puste id modułu");
    }
}

#[test]
fn metadata_keys_are_translated_in_both_locales() {
    let translator = Translator::load().expect("tłumaczenia się wczytują");
    let registry = registry();
    for module in registry.modules() {
        let meta = module.meta();
        let keys = [
            meta.name_key,
            meta.desc_key,
            meta.capability.label_key(),
            meta.group.label_key(),
        ];
        for locale in Locale::ALL {
            for key in keys {
                assert!(
                    translator.has(locale, key),
                    "moduł {}: brak klucza {key} w języku {}",
                    meta.id,
                    locale.code()
                );
            }
        }
    }
}

#[test]
fn modules_are_sorted_by_group_then_id() {
    let registry = registry();
    let keys: Vec<(u8, &str)> = registry
        .modules()
        .iter()
        .map(|module| {
            let meta = module.meta();
            (meta.group.rank(), meta.id)
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "kolejność modułów jest niestabilna");
}

#[test]
fn collect_never_panics_without_providers_data() {
    let harness = Harness::new();
    let registry = registry();
    for module in registry.modules() {
        let id = module.meta().id;
        let ctx = harness.ctx();
        let result = catch_unwind(AssertUnwindSafe(|| module.collect(&ctx)));
        assert!(
            result.is_ok(),
            "moduł {id} panikował, gdy dostawca nie ma danych"
        );
    }
}

#[test]
fn every_module_declares_a_known_capability() {
    let registry = registry();
    for module in registry.modules() {
        let meta = module.meta();
        // Destrukcyjne moduły muszą jawnie oznaczyć się jako takie — brak
        // potwierdzenia w UI jest błędem bezpieczeństwa, nie kosmetyką.
        assert!(
            matches!(
                meta.capability,
                sk_core::module::Capability::Userland
                    | sk_core::module::Capability::Elevated
                    | sk_core::module::Capability::Destructive
            ),
            "moduł {} ma nieznane uprawnienia",
            meta.id
        );
    }
}
