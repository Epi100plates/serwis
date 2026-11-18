use crate::module::Module;

/// Jawny rejestr modułów.
///
/// Świadomie nie używamy rejestracji przez linkowanie (`inventory`): na Windows
/// z linkerm MSVC statyki z nieodwoływanych crate'ów bywają odrzucane, co kończy
/// się pustą listą modułów bez żadnego błędu kompilacji. Jawna lista jest
/// deterministyczna i widoczna w jednym pliku (`sk-modules/src/modules/mod.rs`).
#[derive(Default)]
pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dodaje moduł. Typ musi mieć `Default` (moduły są bezstanowe).
    pub fn add<M: Module + Default>(&mut self) -> &mut Self {
        self.modules.push(Box::new(M::default()));
        self
    }

    /// Sortuje po grupie, a w grupie po `id` — stabilna kolejność w UI i raportach.
    pub fn into_sorted(mut self) -> Self {
        self.modules.sort_by(|left, right| {
            let left_meta = left.meta();
            let right_meta = right.meta();
            left_meta
                .group
                .rank()
                .cmp(&right_meta.group.rank())
                .then_with(|| left_meta.id.cmp(right_meta.id))
        });
        self
    }

    pub fn modules(&self) -> &[Box<dyn Module>] {
        &self.modules
    }

    pub fn find(&self, id: &str) -> Option<&dyn Module> {
        self.modules
            .iter()
            .find(|module| module.meta().id == id)
            .map(AsRef::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Capability, CollectCtx, Group, ModuleMeta, ModuleOutput};
    use crate::view::View;

    #[derive(Default)]
    struct Alpha;
    #[derive(Default)]
    struct Beta;

    impl Module for Alpha {
        fn meta(&self) -> ModuleMeta {
            ModuleMeta {
                id: "z.alpha",
                name_key: "alpha.name",
                desc_key: "alpha.desc",
                group: Group::Tools,
                capability: Capability::Userland,
                experimental: false,
            }
        }

        fn collect(&self, _ctx: &CollectCtx<'_>) -> crate::error::Result<ModuleOutput> {
            Ok(ModuleOutput {
                data: serde_json::Value::Null,
                view: View::default(),
            })
        }
    }

    impl Module for Beta {
        fn meta(&self) -> ModuleMeta {
            ModuleMeta {
                id: "a.beta",
                name_key: "beta.name",
                desc_key: "beta.desc",
                group: Group::Hardware,
                capability: Capability::Elevated,
                experimental: false,
            }
        }

        fn collect(&self, _ctx: &CollectCtx<'_>) -> crate::error::Result<ModuleOutput> {
            Ok(ModuleOutput {
                data: serde_json::Value::Null,
                view: View::default(),
            })
        }
    }

    #[test]
    fn sorts_by_group_then_id() {
        let mut registry = ModuleRegistry::new();
        registry.add::<Alpha>().add::<Beta>();
        let registry = registry.into_sorted();
        let ids: Vec<&str> = registry
            .modules()
            .iter()
            .map(|module| module.meta().id)
            .collect();
        assert_eq!(ids, vec!["a.beta", "z.alpha"]);
    }

    #[test]
    fn finds_module_by_id() {
        let mut registry = ModuleRegistry::new();
        registry.add::<Beta>();
        assert!(registry.find("a.beta").is_some());
        assert!(registry.find("nie.ma").is_none());
    }
}
