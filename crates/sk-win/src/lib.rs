//! Warstwa Windows: WMI, zegar, środowisko procesu, rejestr, EDID.
//!
//! To **jedyne** miejsce w projekcie, w którym wolno używać `unsafe`.
//! Moduły nie wołają API systemowego bezpośrednio — dostają traity z `sk-core`.

mod clock;
mod console;
mod env;
mod machine;
mod registry;
mod storage;
mod surface;
mod wmi;

pub use clock::SystemClock;
pub use console::{ensure_console, show_error};
pub use env::WinHostEnv;
pub use machine::WinMachine;
pub use storage::WinStorage;
pub use surface::scan_surface;
pub use wmi::{CacheStats, QueryCache, WinWmi};
