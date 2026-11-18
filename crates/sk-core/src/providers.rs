use std::time::SystemTime;

use crate::error::Result;

/// Przestrzeń nazw WMI z inwentarzem sprzętu i systemu.
pub const NS_CIMV2: &str = "ROOT\\CIMV2";
/// Przestrzeń nazw WMI z klasami sterowników (m.in. bateria, ProcessorPerformance).
pub const NS_WMI: &str = "ROOT\\WMI";

/// Odczyt WMI. Implementacja realna w `sk-win`, makieta w `sk_core::testing`.
///
/// Zapytanie zwraca wiersze jako JSON, żeby trait był obiektowy (`dyn WmiProvider`)
/// i żeby moduły mogły deserializować tylko te pola, których potrzebują.
pub trait WmiProvider: Send + Sync {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>>;
}

/// Zegar wstrzykiwany, żeby testy były deterministyczne.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn uptime_secs(&self) -> u64;
}

/// Środowisko procesu: czy mamy uprawnienia administratora.
pub trait HostEnv: Send + Sync {
    fn is_elevated(&self) -> bool;
}
