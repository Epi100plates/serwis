//! Dane o maszynie, których nie daje WMI: tryb firmware, VRAM z rejestru,
//! surowy blok EDID monitora.

use sk_core::providers::{AdapterMemory, Firmware, MachineInfo};
use windows::Win32::System::SystemInformation::{
    FIRMWARE_TYPE, FirmwareTypeBios, FirmwareTypeUefi, GetFirmwareType,
};

use crate::registry;

/// Klasa urządzeń „Karty graficzne” w rejestrze — trzyma realny rozmiar VRAM.
const GPU_CLASS: &str =
    "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}";
const DISPLAY_ENUM: &str = "SYSTEM\\CurrentControlSet\\Enum";

#[derive(Debug, Default)]
pub struct WinMachine;

impl WinMachine {
    pub fn new() -> Self {
        Self
    }
}

impl MachineInfo for WinMachine {
    fn firmware(&self) -> Firmware {
        firmware_type()
    }

    fn adapter_memory(&self) -> Vec<AdapterMemory> {
        adapter_memory()
    }

    fn display_edid(&self, instance_name: &str) -> Option<Vec<u8>> {
        display_edid(instance_name)
    }
}

/// Czy system wystartował w trybie UEFI czy legacy BIOS.
pub fn firmware_type() -> Firmware {
    let mut kind = FIRMWARE_TYPE::default();
    // SAFETY: GetFirmwareType zapisuje wynik do poprawnego, żywego bufora.
    let status = unsafe { GetFirmwareType(&mut kind) };
    if status.is_err() {
        return Firmware::Unknown;
    }
    if kind == FirmwareTypeUefi {
        Firmware::Uefi
    } else if kind == FirmwareTypeBios {
        Firmware::Bios
    } else {
        Firmware::Unknown
    }
}

/// Pamięć dedykowana każdego adaptera graficznego.
///
/// `Win32_VideoController.AdapterRAM` jest 32-bitowe i obcina VRAM do 4 GiB
/// (potwierdzone: RX 6700 XT 12 GB raportuje 4 293 918 720 B), dlatego czytamy
/// `HardwareInformation.qwMemorySize` z klasy sterownika karty.
pub fn adapter_memory() -> Vec<AdapterMemory> {
    let mut adapters = Vec::new();
    for key_name in registry::subkeys(GPU_CLASS) {
        if key_name.len() != 4 || !key_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let path = format!("{GPU_CLASS}\\{key_name}");
        let Some(name) = registry::read_string(&path, "DriverDesc") else {
            continue;
        };
        let Some(bytes) = registry::read_qword(&path, "HardwareInformation.qwMemorySize") else {
            continue;
        };
        if bytes == 0 {
            continue;
        }
        adapters.push(AdapterMemory { name, bytes });
    }
    adapters
}

/// Blok EDID monitora wskazanego przez `WmiMonitorID.InstanceName`.
///
/// `InstanceName` ma postać `DISPLAY\SAM0928\7&1c9e91e2&0&UID260_0`, a EDID leży
/// pod `Enum\DISPLAY\SAM0928\7&1c9e91e2&0&UID260\Device Parameters` — różni się
/// tylko sufiksem indeksu monitora.
pub fn display_edid(instance_name: &str) -> Option<Vec<u8>> {
    let instance = strip_uid_suffix(instance_name.trim());
    if instance.is_empty() {
        return None;
    }
    let path = format!("{DISPLAY_ENUM}\\{instance}\\Device Parameters");
    registry::read_binary(&path, "EDID")
}

/// Usuwa końcowy `_<cyfry>` z nazwy instancji monitora.
fn strip_uid_suffix(instance_name: &str) -> &str {
    match instance_name.rsplit_once('_') {
        Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => instance_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_monitor_index_suffix() {
        assert_eq!(
            strip_uid_suffix("DISPLAY\\SAM0928\\7&1c9e91e2&0&UID260_0"),
            "DISPLAY\\SAM0928\\7&1c9e91e2&0&UID260"
        );
        assert_eq!(
            strip_uid_suffix("DISPLAY\\GBT2730\\7&1c9e91e2&0&UID260_1"),
            "DISPLAY\\GBT2730\\7&1c9e91e2&0&UID260"
        );
    }

    #[test]
    fn keeps_names_without_numeric_suffix() {
        assert_eq!(
            strip_uid_suffix("DISPLAY\\ABC1234\\x"),
            "DISPLAY\\ABC1234\\x"
        );
        assert_eq!(
            strip_uid_suffix("DISPLAY\\ABC1234\\x_"),
            "DISPLAY\\ABC1234\\x_"
        );
    }

    /// Test sprzętowy: `cargo test -p sk-win -- --ignored`.
    #[test]
    #[ignore = "wymaga prawdziwego rejestru Windows z GPU/EDID"]
    fn reads_real_firmware_and_adapters() {
        let machine = WinMachine::new();
        assert_ne!(machine.firmware(), Firmware::Unknown);
        let adapters = machine.adapter_memory();
        assert!(!adapters.is_empty(), "brak adapterów w rejestrze");
    }
}
