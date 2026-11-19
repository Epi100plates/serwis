//! Kontrakt modułów SerwisKit.
//!
//! Ten crate nie zna Windows ani GUI: zawiera typy domenowe, rejestr modułów,
//! model widoku i tłumaczenia. Moduły dostają dostęp do świata wyłącznie przez
//! traity z [`providers`], dzięki czemu testują się bez sprzętu.

pub mod error;
pub mod i18n;
pub mod live;
pub mod module;
pub mod providers;
pub mod queries;
pub mod registry;
pub mod surface;
pub mod view;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Typy najczęściej używane przez moduły.
pub mod prelude {
    pub use crate::error::{ModuleError, Result};
    pub use crate::i18n::{Locale, Translator};
    pub use crate::live::{LiveHistory, LiveSample, percent_of};
    pub use crate::module::{Capability, CollectCtx, Group, Module, ModuleMeta, ModuleOutput};
    pub use crate::providers::{
        AdapterMemory, BusKind, Clock, DriveKind, Firmware, HostEnv, LiveSource, MachineInfo,
        NS_CIMV2, NS_WMI, PhysicalDrive, SmartData, StorageProbe, WmiProvider,
    };
    pub use crate::queries;
    pub use crate::registry::ModuleRegistry;
    pub use crate::surface::{
        IdleSurface, SurfaceAccess, SurfaceParams, SurfaceReadError, SurfaceSample, SurfaceScan,
        SurfaceSource, SurfaceState, SurfaceStore,
    };
    pub use crate::view::{
        Badge, Block, Chart, ChartSeries, KeyValue, NoteLevel, Section, StatusLevel, Table, View,
    };
}
