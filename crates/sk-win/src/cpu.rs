//! Czujniki procesora wymagające ring 0: temperatura rdzeni (AMD Tctl/Tdie,
//! CCD; Intel package), napięcie rdzenia i SoC oraz moc pakietu.
//!
//! AMD czyta SMN przez moduł `AMDFamily17` (rejestry zweryfikowane na Zen 3:
//! `THM_TCON_CUR_TMP` = `0x00059800`, SVI2 = `0x0005A000`), Intel — MSR przez
//! moduł `IntelMSR`. Ścieżka Intela nie była uruchamiana na sprzęcie (brak
//! maszyny w pracowni) — wzory pochodzą z dokumentacji MSR i LHM.
//!
//! Brak sterownika PawnIO albo nieobsługiwany procesor to brak odczytów,
//! nie błąd: moduł pokazuje wtedy notkę.

use std::cell::Cell;
use std::time::Instant;

use sk_core::sensors::{DeviceKind, SensorKind, SensorReading};

use crate::pawnio::{self, PawnIo};

/// `THM_TCON_CUR_TMP` — bieżąca temperatura (Tctl).
const SMN_THM_TCON_CUR_TMP: u64 = 0x0005_9800;
/// `SVI0_TFN_PLANE0/1` — ważność telemetrii napięć.
const SMN_SVI_TFN: u64 = 0x0005_A008;
const SMN_SVI: u64 = 0x0005_A000;
/// `MSRC001_0299` (jednostka energii) i `MSRC001_029B` (licznik energii).
const MSR_PWR_UNIT: u64 = 0xC001_0299;
const MSR_PKG_ENERGY_STAT: u64 = 0xC001_029B;

/// `F17H_TEMP_RANGE_SEL` (bit 19) i `TJ_SEL` (bity 17:16) — na Zen 3 zaznaczają
/// korektę Tctl o −49 °C.
const TEMP_RANGE_SEL_MASK: u32 = 0x0008_0000;
const TEMP_TJ_SEL_MASK: u32 = 0x0003_0000;
/// `F17H_CCD_TEMP_VALID` — bit 11 w rejestrze temperatury CCD.
const CCD_TEMP_VALID: u32 = 0x0800;

const MAX_TEMPERATURE_C: f64 = 125.0;

/// Nazwa procesora z rejestru — WMI w tym miejscu bywa wolniejsze, a wartość
/// jest ta sama, którą pokazuje „Mój komputer”.
const CPU_NAME_PATH: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

/// Czujniki jednego procesora. `None` z [`CpuSensors::probe`] = brak sterownika
/// albo nieznany producent.
pub struct CpuSensors {
    device: String,
    backend: Backend,
}

enum Backend {
    Amd(Amd),
    Intel(Intel),
}

struct Amd {
    module: PawnIo,
    family: u32,
    model: u32,
    /// Poprzedni odczyt licznika energii — różnica daje moc pakietu.
    energy: Cell<Option<(u64, Instant)>>,
}

struct Intel {
    module: PawnIo,
}

impl CpuSensors {
    pub fn probe() -> Option<Self> {
        let identity = cpuid_identity()?;
        let device = cpu_name().unwrap_or_default();
        let backend = match identity.vendor.as_str() {
            "AuthenticAMD" if matches!(identity.family, 0x17 | 0x19 | 0x1A) => {
                let module = PawnIo::load_module(pawnio::AMD_FAMILY17).ok()?;
                Backend::Amd(Amd {
                    module,
                    family: identity.family,
                    model: identity.model,
                    energy: Cell::new(None),
                })
            }
            "GenuineIntel" => {
                let module = PawnIo::load_module(pawnio::INTEL_MSR).ok()?;
                Backend::Intel(Intel { module })
            }
            _ => return None,
        };
        let sensors = Self { device, backend };
        // Samo załadowanie modułu nie znaczy, że procesor odpowiada — sprawdzamy
        // pierwszy odczyt, żeby moduł nie pokazywał „CPU działa” bez danych.
        (!sensors.readings().is_empty()).then_some(sensors)
    }

    pub fn readings(&self) -> Vec<SensorReading> {
        match &self.backend {
            Backend::Amd(amd) => amd.readings(&self.device),
            Backend::Intel(intel) => intel.readings(&self.device),
        }
    }
}

impl Amd {
    fn readings(&self, device: &str) -> Vec<SensorReading> {
        let mut readings = Vec::new();

        if let Ok(raw) = self.smn(SMN_THM_TCON_CUR_TMP)
            && let Some(celsius) = decode_tctl(raw)
        {
            readings.push(reading(device, SensorKind::Temperature, celsius));
        }

        let mut ccds = Vec::new();
        self.read_ccds(&mut |channel, celsius| ccds.push((channel, celsius)));
        for (channel, celsius) in ccds {
            let reading = reading(device, SensorKind::CcdTemp, celsius);
            readings.push(match channel {
                Some(channel) => reading.with_channel(channel),
                None => reading,
            });
        }

        let tfn = self.smn(SMN_SVI_TFN).unwrap_or_default();
        let (plane0, plane1) = self.svi_planes();
        if tfn & 0x01 == 0
            && let Ok(raw) = self.smn(plane0)
            && let Some(volts) = decode_svi2(raw)
        {
            readings.push(reading(device, SensorKind::Voltage, volts));
        }
        if let Ok(raw) = self.smn(plane1)
            && let Some(volts) = decode_svi2(raw)
        {
            readings.push(reading(device, SensorKind::SocVoltage, volts));
        }

        if let Some(watts) = self.package_power() {
            readings.push(reading(device, SensorKind::Power, watts));
        }
        readings
    }

    /// Offsety telemetrii SVI2 zależą od generacji — tak samo jak w LHM.
    fn svi_planes(&self) -> (u64, u64) {
        match self.model {
            0x31 => (SMN_SVI + 0x14, SMN_SVI + 0x10),
            0x61 | 0x71 | 0x21 | 0x44 => (SMN_SVI + 0x10, SMN_SVI + 0x0C),
            _ => (SMN_SVI + 0x0C, SMN_SVI + 0x10),
        }
    }

    fn read_ccds(&self, push: &mut impl FnMut(Option<u8>, f64)) {
        let (base, count) = ccd_layout(self.family, self.model);
        let mut seen = Vec::new();
        for index in 0..count {
            let Ok(raw) = self.smn(base + u64::from(index) * 4) else {
                continue;
            };
            if let Some(celsius) = decode_ccd(raw) {
                seen.push(celsius);
            }
        }
        let multiple = seen.len() > 1;
        for (index, celsius) in seen.into_iter().enumerate() {
            let channel = multiple.then(|| u8::try_from(index + 1).unwrap_or(1));
            push(channel, celsius);
        }
    }

    fn smn(&self, offset: u64) -> sk_core::error::Result<u32> {
        self.module
            .execute_one("ioctl_read_smn", &[offset])
            .map(|value| value as u32)
    }

    fn read_msr(&self, index: u64) -> sk_core::error::Result<u64> {
        self.module.execute_one("ioctl_read_msr", &[index])
    }

    /// Moc pakietu z licznika energii: jednostka to 2^−ESU dżuli, więc waty
    /// liczymy z różnicy licznika i czasu między próbkami.
    fn package_power(&self) -> Option<f64> {
        let unit = self.read_msr(MSR_PWR_UNIT).ok()? as u32;
        let energy = self.read_msr(MSR_PKG_ENERGY_STAT).ok()?;
        let now = Instant::now();
        let previous = self.energy.replace(Some((energy, now)))?;
        let (previous_energy, previous_time) = previous;

        let delta = energy.wrapping_sub(previous_energy) as f64;
        let seconds = now.duration_since(previous_time).as_secs_f64();
        if seconds <= 0.0 {
            return None;
        }
        let exponent = ((unit >> 8) & 0x1F) as i32;
        let watts = delta * 2f64.powi(-exponent) / seconds;
        (watts.is_finite() && watts > 0.0 && watts < 500.0).then_some(watts)
    }
}

impl Intel {
    fn readings(&self, device: &str) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        let tj_max = self
            .module
            .execute_one("ioctl_read_msr", &[0x1A2])
            .ok()
            .map(|value| ((value >> 16) & 0xFF) as i32);

        if let (Some(tj_max), Ok(status)) =
            (tj_max, self.module.execute_one("ioctl_read_msr", &[0x1B1]))
            && let Some(celsius) = decode_intel_temperature(status as u32, tj_max)
        {
            readings.push(SensorReading::new(
                device.to_string(),
                DeviceKind::Cpu,
                SensorKind::Temperature,
                celsius,
            ));
        }

        if let Ok(status) = self.module.execute_one("ioctl_read_msr", &[0x198])
            && let Some(volts) = decode_intel_voltage(status)
        {
            readings.push(SensorReading::new(
                device.to_string(),
                DeviceKind::Cpu,
                SensorKind::Voltage,
                volts,
            ));
        }
        readings
    }
}

/// Tctl: bity [31:21] to temperatura w ⅛ °C, a `RANGE_SEL`/`TJ_SEL` oznaczają
/// korektę o −49 °C (Zen 3 z chłodzeniem fabrycznym).
pub fn decode_tctl(raw: u32) -> Option<f64> {
    let offset = raw & TEMP_RANGE_SEL_MASK != 0 || raw & TEMP_TJ_SEL_MASK == TEMP_TJ_SEL_MASK;
    let mut celsius = f64::from(raw >> 21) * 0.125;
    if offset {
        celsius -= 49.0;
    }
    (celsius > 0.0 && celsius < MAX_TEMPERATURE_C).then_some(celsius)
}

/// Temperatura CCD: bity [11:0] w ⅛ °C, przesunięte o 305 °C; bit 11 = obecność.
pub fn decode_ccd(raw: u32) -> Option<f64> {
    let value = raw & 0x0FFF;
    if value == 0 || raw & CCD_TEMP_VALID == 0 {
        return None;
    }
    let celsius = f64::from(value) * 0.125 - 305.0;
    (celsius > 0.0 && celsius < MAX_TEMPERATURE_C).then_some(celsius)
}

/// SVI2: kod napięcia w bitach [23:16], krok 6,25 mV od 1,55 V.
pub fn decode_svi2(raw: u32) -> Option<f64> {
    let code = (raw >> 16) & 0xFF;
    let volts = 1.550 - 0.006_25 * f64::from(code);
    (0.2..=1.6).contains(&volts).then_some(volts)
}

/// Intel: `IA32_PACKAGE_THERM_STATUS` — odczyt w bitach [22:16] jako odległość
/// od TjMax.
pub fn decode_intel_temperature(status: u32, tj_max: i32) -> Option<f64> {
    let offset = ((status >> 16) & 0x7F) as i32;
    if offset == 0 {
        return None;
    }
    let celsius = f64::from(tj_max - offset);
    (celsius > 0.0 && celsius < MAX_TEMPERATURE_C).then_some(celsius)
}

/// Intel: `IA32_PERF_STATUS`, napięcie w bitach [47:32] w krokach 1/8192 V.
pub fn decode_intel_voltage(perf_status: u64) -> Option<f64> {
    let raw = ((perf_status >> 32) & 0xFFFF) as f64;
    let volts = raw / 8192.0;
    (volts > 0.2 && volts < 2.0).then_some(volts)
}

fn ccd_layout(family: u32, model: u32) -> (u64, u32) {
    if family == 0x1A {
        (0x0005_99F0, 16)
    } else if matches!(model, 0x61 | 0x44) {
        (0x0005_9B08, 8)
    } else {
        (0x0005_9954, 8)
    }
}

/// Jeden odczyt CPU — wszystkie mają to samo urządzenie (nazwę procesora).
fn reading(device: &str, kind: SensorKind, value: f64) -> SensorReading {
    SensorReading::new(device.to_string(), DeviceKind::Cpu, kind, value)
}

struct Identity {
    vendor: String,
    family: u32,
    model: u32,
}

fn cpuid_identity() -> Option<Identity> {
    let leaf0 = std::arch::x86_64::__cpuid(0);
    let mut bytes = Vec::with_capacity(12);
    for register in [leaf0.ebx, leaf0.edx, leaf0.ecx] {
        bytes.extend_from_slice(&register.to_le_bytes());
    }
    let vendor = String::from_utf8(bytes).ok()?;

    let leaf1 = std::arch::x86_64::__cpuid(1);
    let base_family = (leaf1.eax >> 8) & 0xF;
    let ext_family = (leaf1.eax >> 20) & 0xFF;
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let base_model = (leaf1.eax >> 4) & 0xF;
    let ext_model = (leaf1.eax >> 16) & 0xF;
    let model = if matches!(family, 0x6 | 0xF) {
        base_model | (ext_model << 4)
    } else {
        base_model
    };
    Some(Identity {
        vendor,
        family,
        model,
    })
}

fn cpu_name() -> Option<String> {
    let name = crate::registry::read_string(CPU_NAME_PATH, "ProcessorNameString")?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tctl_applies_minus_49_offset() {
        // 55,1 °C przed korektą: 0x683B0000 → 833 * 0,125 = 104,125 − 49.
        assert!((decode_tctl(0x683B_0000).unwrap() - 55.125).abs() < 0.001);
        // Bez bitów korekty: 0x0000_0000 to 0 °C, więc odczyt jest odrzucany.
        assert_eq!(decode_tctl(0), None);
        // 40 °C bez korekty: 320 * 0,125.
        assert_eq!(decode_tctl(320 << 21), Some(40.0));
    }

    #[test]
    fn ccd_temperature_uses_305_offset_and_valid_bit() {
        assert_eq!(decode_ccd(0x0000_0B44), Some(55.5));
        // Bit 11 wyzerowany = brak CCD.
        assert_eq!(decode_ccd(0x0000_0344), None);
        assert_eq!(decode_ccd(0), None);
    }

    #[test]
    fn svi2_decodes_kernel_voltage() {
        let volts = decode_svi2(0x013B_0000).unwrap();
        assert!((volts - 1.181_25).abs() < 0.000_1, "{volts}");
        assert_eq!(decode_svi2(0), Some(1.55));
        assert_eq!(decode_svi2(0x00FF_0000), None);
    }

    #[test]
    fn intel_temperature_and_voltage_math() {
        // TjMax 95, odczyt 37 stopni poniżej → 58 °C.
        assert_eq!(decode_intel_temperature(37 << 16, 95), Some(58.0));
        assert_eq!(decode_intel_temperature(0, 95), None);
        // 1,20 V = 9830 / 8192.
        let volts = decode_intel_voltage(9830 << 32).unwrap();
        assert!((volts - 1.199_95).abs() < 0.000_1, "{volts}");
        assert_eq!(decode_intel_voltage(0), None);
    }

    #[test]
    fn ccd_layout_follows_family_and_model() {
        assert_eq!(ccd_layout(0x19, 0x21), (0x0005_9954, 8));
        assert_eq!(ccd_layout(0x19, 0x61), (0x0005_9B08, 8));
        assert_eq!(ccd_layout(0x1A, 0x08), (0x0005_99F0, 16));
    }

    /// Wymaga zainstalowanego PawnIO i procesora AMD Zen lub Intela.
    #[test]
    #[ignore = "wymaga PawnIO i prawdziwego procesora"]
    fn hardware_cpu_sensors_are_plausible() {
        let _serial = crate::pawnio::PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sensors = CpuSensors::probe().expect("PawnIO i obsługiwany procesor");
        let first = sensors.readings();
        println!("CPU: {first:#?}");
        assert!(
            first
                .iter()
                .any(|reading| reading.kind == SensorKind::Temperature),
            "brak temperatury CPU"
        );
        for reading in &first {
            match reading.unit() {
                sk_core::sensors::SensorUnit::Celsius => {
                    assert!((10.0..110.0).contains(&reading.value), "{reading:?}");
                }
                sk_core::sensors::SensorUnit::Volt => {
                    assert!((0.2..1.6).contains(&reading.value), "{reading:?}");
                }
                _ => {}
            }
        }

        // Moc pakietu pojawia się od drugiej próbki (licznik energii).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = sensors.readings();
        println!("CPU po sekundzie: {second:#?}");
    }
}
