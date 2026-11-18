//! Makiety dostawców danych dla testów modułów (feature `testing`).
//!
//! Dzięki nim moduł testuje się bez WMI, bez sprzętu i bez uprawnień
//! administratora. Makieta zwraca dokładnie to, co poda test.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{ModuleError, Result};
use crate::i18n::Locale;
use crate::live::LiveSample;
use crate::module::CollectCtx;
use crate::providers::{
    AdapterMemory, Clock, Firmware, HostEnv, LiveSource, MachineInfo, PhysicalDrive, SmartData,
    StorageProbe, WmiProvider,
};

/// WMI zwracające przygotowane wiersze. Fragment zapytania dopasowuje się do
/// treści zapytania (np. `"Win32_OperatingSystem"`).
#[derive(Debug, Default)]
pub struct MockWmi {
    responses: Vec<(String, Vec<serde_json::Value>)>,
}

impl MockWmi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rejestruje odpowiedź dla zapytań zawierających `query_fragment`.
    pub fn with_rows(
        mut self,
        query_fragment: impl Into<String>,
        rows: Vec<serde_json::Value>,
    ) -> Self {
        self.responses.push((query_fragment.into(), rows));
        self
    }
}

impl WmiProvider for MockWmi {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>> {
        let haystack = format!("{namespace} {wql}");
        // Najbardziej szczegółowy fragment wygrywa: zapytanie o
        // `Win32_PhysicalMemoryArray` zawiera też `Win32_PhysicalMemory`.
        self.responses
            .iter()
            .filter(|(fragment, _)| haystack.contains(fragment.as_str()))
            .max_by_key(|(fragment, _)| fragment.len())
            .map(|(_, rows)| rows.clone())
            .ok_or_else(|| {
                ModuleError::unavailable(format!("mock WMI: brak odpowiedzi dla `{wql}`"))
            })
    }
}

/// Zegar o stałym czasie — testy nie zależą od daty uruchomienia.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: SystemTime,
    uptime_secs: u64,
}

impl FixedClock {
    /// 1 stycznia 2024, 00:00:00 UTC.
    pub fn new() -> Self {
        Self {
            now: UNIX_EPOCH + Duration::from_secs(1_704_067_200),
            uptime_secs: 3_600,
        }
    }

    pub fn with_uptime(mut self, uptime_secs: u64) -> Self {
        self.uptime_secs = uptime_secs;
        self
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.now
    }

    fn uptime_secs(&self) -> u64 {
        self.uptime_secs
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockHost {
    elevated: bool,
}

impl MockHost {
    pub fn elevated() -> Self {
        Self { elevated: true }
    }

    pub fn userland() -> Self {
        Self { elevated: false }
    }
}

impl HostEnv for MockHost {
    fn is_elevated(&self) -> bool {
        self.elevated
    }
}

/// Dane maszynowe spoza WMI. Domyślnie puste — moduł ma pokazać degradację,
/// nie zgadywać wartości.
#[derive(Debug, Clone, Default)]
pub struct MockMachine {
    firmware: Firmware,
    adapter_memory: Vec<AdapterMemory>,
    edid: std::collections::HashMap<String, Vec<u8>>,
}

impl MockMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_firmware(mut self, firmware: Firmware) -> Self {
        self.firmware = firmware;
        self
    }

    pub fn with_adapter_memory(mut self, name: impl Into<String>, bytes: u64) -> Self {
        self.adapter_memory.push(AdapterMemory {
            name: name.into(),
            bytes,
        });
        self
    }

    pub fn with_edid(mut self, instance_name: impl Into<String>, edid: Vec<u8>) -> Self {
        self.edid.insert(instance_name.into(), edid);
        self
    }
}

impl MachineInfo for MockMachine {
    fn firmware(&self) -> Firmware {
        self.firmware
    }

    fn adapter_memory(&self) -> Vec<AdapterMemory> {
        self.adapter_memory.clone()
    }

    fn display_edid(&self, instance_name: &str) -> Option<Vec<u8>> {
        self.edid.get(instance_name).cloned()
    }
}

/// Historia telemetrii dla testów — moduł dostaje dokładnie podane próbki.
#[derive(Debug, Clone, Default)]
pub struct MockLive {
    samples: Vec<LiveSample>,
}

impl MockLive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_samples(mut self, samples: Vec<LiveSample>) -> Self {
        self.samples = samples;
        self
    }
}

impl LiveSource for MockLive {
    fn samples(&self) -> Vec<LiveSample> {
        self.samples.clone()
    }
}

/// Jeden dysk w makiecie: dane SMART albo błąd, który naprawdę może się zdarzyć.
#[derive(Debug, Clone)]
enum MockSmart {
    Data(SmartData),
    Denied,
    Unavailable(String),
}

/// Dyski fizyczne i SMART dla testów — moduł dostaje dokładnie to, co poda test.
///
/// Domyślnie brak dysków, więc makieta bez konfiguracji zachowuje się jak proces
/// bez uprawnień administratora: moduł ma pokazać notkę, nie panikę.
#[derive(Debug, Clone, Default)]
pub struct MockStorage {
    drives: std::collections::HashMap<u32, PhysicalDrive>,
    smart: std::collections::HashMap<u32, MockSmart>,
}

impl MockStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(mut self, drive: PhysicalDrive, entry: MockSmart) -> Self {
        self.smart.insert(drive.index, entry);
        self.drives.insert(drive.index, drive);
        self
    }

    /// Dysk NVMe z surową stroną logu zdrowia (512 B).
    pub fn with_nvme(self, drive: PhysicalDrive, log_page: Vec<u8>) -> Self {
        self.insert(drive, MockSmart::Data(SmartData::Nvme { log_page }))
    }

    /// Dysk ATA/SATA z surowymi atrybutami i progami (po 512 B).
    pub fn with_ata(self, drive: PhysicalDrive, attributes: Vec<u8>, thresholds: Vec<u8>) -> Self {
        self.insert(
            drive,
            MockSmart::Data(SmartData::Ata {
                attributes,
                thresholds,
            }),
        )
    }

    /// Dysk, do którego nie ma dostępu (proces bez uprawnień administratora):
    /// nie znamy nawet magistrali, tak jak w rzeczywistości.
    pub fn with_denied(mut self, drive: PhysicalDrive) -> Self {
        self.smart.insert(drive.index, MockSmart::Denied);
        self
    }

    /// Dysk podłączony przez mostek/kontroler, który nie przekazuje SMART.
    pub fn with_unavailable(mut self, drive: PhysicalDrive, reason: impl Into<String>) -> Self {
        self.smart
            .insert(drive.index, MockSmart::Unavailable(reason.into()));
        self.drives.insert(drive.index, drive);
        self
    }
}

impl StorageProbe for MockStorage {
    fn describe(&self, index: u32) -> Result<PhysicalDrive> {
        self.drives.get(&index).cloned().ok_or_else(|| {
            if matches!(self.smart.get(&index), Some(MockSmart::Denied)) {
                ModuleError::access_denied(format!("\\\\.\\PhysicalDrive{index}"))
            } else {
                ModuleError::unavailable(format!("brak dysku {index} w makiecie"))
            }
        })
    }

    fn smart(&self, index: u32) -> Result<SmartData> {
        match self.smart.get(&index) {
            Some(MockSmart::Data(data)) => Ok(data.clone()),
            Some(MockSmart::Denied) => Err(ModuleError::access_denied(format!(
                "\\\\.\\PhysicalDrive{index}"
            ))),
            Some(MockSmart::Unavailable(reason)) => Err(ModuleError::unavailable(reason.clone())),
            None => Err(ModuleError::unavailable(format!(
                "brak dysku {index} w makiecie"
            ))),
        }
    }
}

/// Zestaw makiet + wygodne `ctx()`.
#[derive(Debug)]
pub struct Harness {
    pub wmi: MockWmi,
    pub clock: FixedClock,
    pub host: MockHost,
    pub machine: MockMachine,
    pub live: MockLive,
    pub storage: MockStorage,
    pub locale: Locale,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            wmi: MockWmi::new(),
            clock: FixedClock::new(),
            host: MockHost::default(),
            machine: MockMachine::new(),
            live: MockLive::new(),
            storage: MockStorage::new(),
            locale: Locale::Pl,
        }
    }

    pub fn with_wmi(mut self, wmi: MockWmi) -> Self {
        self.wmi = wmi;
        self
    }

    pub fn with_clock(mut self, clock: FixedClock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_host(mut self, host: MockHost) -> Self {
        self.host = host;
        self
    }

    pub fn with_machine(mut self, machine: MockMachine) -> Self {
        self.machine = machine;
        self
    }

    pub fn with_live(mut self, live: MockLive) -> Self {
        self.live = live;
        self
    }

    pub fn with_storage(mut self, storage: MockStorage) -> Self {
        self.storage = storage;
        self
    }

    pub fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    pub fn ctx(&self) -> CollectCtx<'_> {
        CollectCtx {
            wmi: &self.wmi,
            clock: &self.clock,
            host: &self.host,
            machine: &self.machine,
            live: &self.live,
            storage: &self.storage,
            locale: self.locale,
        }
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_wmi_matches_by_fragment() {
        let wmi =
            MockWmi::new().with_rows("Win32_BIOS", vec![serde_json::json!({"SerialNumber": "X"})]);
        let rows = wmi
            .query("ROOT\\CIMV2", "SELECT * FROM Win32_BIOS")
            .expect("trafienie");
        assert_eq!(rows.len(), 1);
        assert!(wmi.query("ROOT\\CIMV2", "SELECT * FROM Win32_Fan").is_err());
    }

    #[test]
    fn most_specific_fragment_wins() {
        let wmi = MockWmi::new()
            .with_rows(
                "Win32_PhysicalMemory",
                vec![serde_json::json!({"Source": "modules"})],
            )
            .with_rows(
                "Win32_PhysicalMemoryArray",
                vec![serde_json::json!({"Source": "array"})],
            );
        let rows = wmi
            .query(
                crate::providers::NS_CIMV2,
                "SELECT MaxCapacity FROM Win32_PhysicalMemoryArray",
            )
            .expect("trafienie");
        assert_eq!(rows[0]["Source"], "array");
    }

    #[test]
    fn harness_builds_ctx() {
        let harness = Harness::new().with_host(MockHost::elevated());
        let ctx = harness.ctx();
        assert!(ctx.host.is_elevated());
        assert_eq!(ctx.clock.uptime_secs(), 3_600);
    }

    #[test]
    fn mock_storage_returns_configured_data_and_errors() {
        let nvme = PhysicalDrive::new(
            0,
            crate::providers::BusKind::Nvme,
            crate::providers::DriveKind::Ssd,
        );
        let sata = PhysicalDrive::new(
            1,
            crate::providers::BusKind::Sata,
            crate::providers::DriveKind::Ssd,
        );
        let denied = PhysicalDrive::new(
            2,
            crate::providers::BusKind::Unknown,
            crate::providers::DriveKind::Unknown,
        );
        let storage = MockStorage::new()
            .with_nvme(nvme, vec![1; 512])
            .with_ata(sata, vec![2; 512], vec![3; 512])
            .with_denied(denied);

        assert_eq!(
            storage.describe(0).expect("opis NVMe").bus,
            crate::providers::BusKind::Nvme
        );
        assert!(matches!(
            storage.smart(0),
            Ok(SmartData::Nvme { ref log_page }) if log_page.len() == 512
        ));
        assert!(matches!(storage.smart(1), Ok(SmartData::Ata { .. })));
        // Brak dostępu: nie znamy nawet magistrali.
        assert!(matches!(
            storage.describe(2),
            Err(ModuleError::AccessDenied { .. })
        ));
        assert!(storage.smart(9).is_err());
    }

    #[test]
    fn empty_mock_storage_is_a_valid_state() {
        let storage = MockStorage::new();
        assert!(storage.describe(0).is_err());
        assert!(storage.smart(0).is_err());
    }
}
