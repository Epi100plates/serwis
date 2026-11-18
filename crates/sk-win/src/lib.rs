//! Warstwa Windows: WMI, zegar, środowisko procesu, rejestr, EDID.
//!
//! To **jedyne** miejsce w projekcie, w którym wolno używać `unsafe`.
//! Moduły nie wołają API systemowego bezpośrednio — dostają traity z `sk-core`.

mod clock;
mod env;
mod machine;
mod registry;
mod wmi;

pub use clock::SystemClock;
pub use env::WinHostEnv;
pub use machine::WinMachine;
pub use wmi::{CacheStats, QueryCache, WinWmi};
