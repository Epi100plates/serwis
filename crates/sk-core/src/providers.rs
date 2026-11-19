use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::live::LiveSample;
use crate::sensors::{SensorReading, SensorSample};

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

/// Bieżące odczyty czujników i ich historia. Transport (ADL/NVML/WMI) jest
/// w `sk-win`, a próbki zbiera wątek roboczy aplikacji — moduł tylko je czyta.
///
/// Domyślne implementacje zwracają pusty zestaw: brak czujników na tej maszynie
/// jest poprawnym stanem, a nie błędem.
pub trait SensorsSource: Send + Sync {
    /// Odczyty z ostatniej próbki.
    fn readings(&self) -> Vec<SensorReading> {
        Vec::new()
    }

    /// Próbki od otwarcia modułu — do minimum, maksimum i średniej.
    fn history(&self) -> Vec<SensorSample> {
        Vec::new()
    }
}

/// Magistrala, do której podłączony jest dysk (`STORAGE_BUS_TYPE`).
///
/// WMI podaje `InterfaceType` niedokładnie (NVMe widzi jako SCSI), dlatego
/// rozpoznanie bierzemy z deskryptora urządzenia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusKind {
    #[default]
    Unknown,
    Nvme,
    Sata,
    Ata,
    Scsi,
    Usb,
    Raid,
    Other,
}

/// Rodzaj nośnika. Ustalany po `IncursSeekPenalty` z deskryptora — WMI twierdzi,
/// że każdy dysk to „Fixed hard disk media”, także SSD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveKind {
    #[default]
    Unknown,
    Hdd,
    Ssd,
}

/// Fizyczny dysk widziany przez `\\.\PhysicalDriveN`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalDrive {
    pub index: u32,
    pub bus: BusKind,
    pub kind: DriveKind,
    pub removable: bool,
}

impl PhysicalDrive {
    pub fn new(index: u32, bus: BusKind, kind: DriveKind) -> Self {
        Self {
            index,
            bus,
            kind,
            removable: false,
        }
    }

    pub fn with_removable(mut self, removable: bool) -> Self {
        self.removable = removable;
        self
    }
}

/// Surowe bloki SMART. Parsowanie należy do modułów (`sk-modules/src/smart.rs`),
/// żeby dało się je przetestować bez dysku i bez uprawnień administratora.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum SmartData {
    /// Strona logu zdrowia NVMe (SMART/Health Information, log page 0x02), 512 B.
    Nvme { log_page: Vec<u8> },
    /// 512 B atrybutów ATA SMART (SMART READ DATA) i 512 B progów.
    Ata {
        attributes: Vec<u8>,
        thresholds: Vec<u8>,
    },
}

/// Dostęp do dysków fizycznych: magistrala, rodzaj nośnika i surowe SMART.
///
/// Wymaga uprawnień administratora — bez nich `\\.\PhysicalDriveN` zwraca
/// odmowę dostępu. Implementacja realna w `sk-win`, makieta w `sk_core::testing`.
pub trait StorageProbe: Send + Sync {
    /// Magistrala i rodzaj nośnika dla dysku o indeksie z `Win32_DiskDrive.Index`.
    fn describe(&self, index: u32) -> Result<PhysicalDrive> {
        Err(crate::error::ModuleError::unavailable(format!(
            "opis dysku {index}"
        )))
    }

    /// SMART jednego dysku w postaci surowej.
    fn smart(&self, index: u32) -> Result<SmartData> {
        Err(crate::error::ModuleError::unavailable(format!(
            "SMART dysku {index}"
        )))
    }
}
