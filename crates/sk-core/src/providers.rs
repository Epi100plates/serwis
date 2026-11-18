use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::live::LiveSample;

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

/// Tryb pracy firmware — istotny przy migracji systemu (MBR vs GPT/EFI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Firmware {
    #[default]
    Unknown,
    Bios,
    Uefi,
}

/// Pamięć dedykowana adaptera graficznego, odczytana poza WMI
/// (`Win32_VideoController.AdapterRAM` jest 32-bitowe i obcina VRAM do 4 GiB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterMemory {
    pub name: String,
    pub bytes: u64,
}

/// Dane o maszynie, których nie daje WMI: firmware, VRAM z rejestru, EDID monitora.
///
/// Wszystkie metody mają domyślne implementacje, więc makieta w testach nie musi
/// ich znać — brak danych jest poprawnym stanem, nie błędem.
pub trait MachineInfo: Send + Sync {
    fn firmware(&self) -> Firmware {
        Firmware::Unknown
    }

    fn adapter_memory(&self) -> Vec<AdapterMemory> {
        Vec::new()
    }

    /// Surowy blok EDID monitora wskazanego przez `WmiMonitorID.InstanceName`.
    fn display_edid(&self, instance_name: &str) -> Option<Vec<u8>> {
        let _ = instance_name;
        None
    }
}

/// Historia próbek telemetrii. Wątek roboczy dopisuje próbki, moduł je czyta —
/// dzięki temu moduł pozostaje bezstanowy.
pub trait LiveSource: Send + Sync {
    fn samples(&self) -> Vec<LiveSample>;
}
