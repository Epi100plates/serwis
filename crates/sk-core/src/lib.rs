//! Kontrakt modułów SerwisKit.
//!
//! Ten crate nie zna Windows ani GUI: zawiera typy domenowe, rejestr modułów,
//! model widoku i tłumaczenia. Moduły dostają dostęp do świata wyłącznie przez
//! traity z [`providers`], dzięki czemu testują się bez sprzętu.

pub mod error;
pub mod i18n;
pub mod module;
pub mod providers;
pub mod registry;
pub mod view;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Typy najczęściej używane przez moduły.
pub mod prelude {
    pub use crate::error::{ModuleError, Result};
    pub use crate::i18n::{Locale, Translator};
    pub use crate::module::{Capability, CollectCtx, Group, Module, ModuleMeta, ModuleOutput};
    pub use crate::providers::{Clock, HostEnv, NS_CIMV2, NS_WMI, WmiProvider};
    pub use crate::registry::ModuleRegistry;
    pub use crate::view::{Badge, Block, KeyValue, NoteLevel, Section, Table, View};
}
