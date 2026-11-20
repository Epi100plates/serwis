//! Model czujników: temperatura, wentylatory, napięcia, moc, taktowanie i obciążenie.
//!
//! Moduł nie odczytuje sprzętu sam — dostaje gotowe odczyty przez
//! [`crate::providers::SensorsSource`]. Transport żyje w `sk-win` (ADL, NVML, WMI),
//! a interpretacja (jednostki, progi, wykresy) w module. Dzięki temu nowy czujnik
//! nie wymaga zmiany kontraktu, a testy nie potrzebują sterownika karty.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Urządzenie, którego dotyczy odczyt. Decyduje o kolejności w tabeli i o tym,
/// czy odczyt trafia na wykres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Cpu,
    Gpu,
    Board,
    Drive,
    Battery,
}

impl DeviceKind {
    pub fn rank(self) -> u8 {
        match self {
            Self::Cpu => 0,
            Self::Gpu => 1,
            Self::Board => 2,
            Self::Drive => 3,
            Self::Battery => 4,
        }
    }
}

/// Rodzaj wielkości fizycznej. Decyduje o jednostce i etykiecie w widoku.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// Temperatura główna urządzenia (rdzeń GPU, pakiet CPU, dysk, strefa ACPI).
    Temperature,
    /// Najgorętszy punkt GPU (junction/hotspot) — wyższy niż temperatura rdzenia.
    Hotspot,
    /// Temperatura pamięci GPU albo modułów VRM.
    MemoryTemp,
    /// Obroty wentylatora w RPM.
    FanRpm,
    /// Wypełnienie PWM wentylatora w procentach.
    FanPercent,
    /// Temperatura matrycy CCD (AMD) — osobny czujnik od Tctl/Tdie.
    CcdTemp,
    /// Napięcie rdzenia GPU albo CPU.
    Voltage,
    /// Napięcie SoC (AMD) — osobne od napięcia rdzenia.
    SocVoltage,
    Power,
    Load,
    /// Taktowanie rdzenia w MHz.
    CoreClock,
    /// Taktowanie pamięci w MHz.
    MemoryClock,
}

impl SensorKind {
    pub fn unit(self) -> SensorUnit {
        match self {
            Self::Temperature | Self::Hotspot | Self::MemoryTemp | Self::CcdTemp => {
                SensorUnit::Celsius
            }
            Self::FanRpm => SensorUnit::Rpm,
            Self::FanPercent | Self::Load => SensorUnit::Percent,
            Self::Voltage | Self::SocVoltage => SensorUnit::Volt,
            Self::Power => SensorUnit::Watt,
            Self::CoreClock | Self::MemoryClock => SensorUnit::Megahertz,
        }
    }

    /// Czy odczyt jest temperaturą — tylko takie serie trafiają na wykres.
    pub fn is_temperature(self) -> bool {
        matches!(
            self,
            Self::Temperature | Self::Hotspot | Self::MemoryTemp | Self::CcdTemp
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorUnit {
    Celsius,
    Rpm,
    Percent,
    Volt,
    Watt,
    Megahertz,
}

impl SensorUnit {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Rpm => "RPM",
            Self::Percent => "%",
            Self::Volt => "V",
            Self::Watt => "W",
            Self::Megahertz => "MHz",
        }
    }
}

/// Jeden odczyt czujnika. `device` jest daną (nazwa modelu, np. „AMD Radeon
/// RX 6700 XT”), a etykieta rodzaju jest kluczem i18n w module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    pub device: String,
    pub device_kind: DeviceKind,
    pub kind: SensorKind,
    /// Numer kanału, gdy urządzenie ma kilka czujników tego samego rodzaju
    /// (rdzenie CPU, czujniki NVMe, strefy ACPI). `None` = pojedynczy czujnik.
    pub channel: Option<u8>,
    pub value: f64,
}

impl SensorReading {
    pub fn new(
        device: impl Into<String>,
        device_kind: DeviceKind,
        kind: SensorKind,
        value: f64,
    ) -> Self {
        Self {
            device: device.into(),
            device_kind,
            kind,
            channel: None,
            value,
        }
    }

    pub fn with_channel(mut self, channel: u8) -> Self {
        self.channel = Some(channel);
        self
    }

    pub fn unit(&self) -> SensorUnit {
        self.kind.unit()
    }

    /// Ten sam czujnik w kolejnej próbce — tożsamość bez wartości, żeby dało się
    /// policzyć minimum, maksimum i średnią z historii.
    pub fn same_sensor(&self, other: &Self) -> bool {
        self.device == other.device
            && self.device_kind == other.device_kind
            && self.kind == other.kind
            && self.channel == other.channel
    }
}

/// Komplet odczytów z jednej chwili.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSample {
    pub elapsed_secs: f64,
    pub readings: Vec<SensorReading>,
}

impl SensorSample {
    pub fn new(elapsed_secs: f64, readings: Vec<SensorReading>) -> Self {
        Self {
            elapsed_secs,
            readings,
        }
    }
}

/// Bufor pierścieniowy próbek czujników. Wątek roboczy aplikacji dopisuje próbki,
/// a moduł czyta ostatnią i liczy statystyki — tak samo jak [`crate::live`].
#[derive(Debug, Clone)]
pub struct SensorStore {
    capacity: usize,
    samples: VecDeque<SensorSample>,
    probe: crate::providers::SensorProbe,
}

impl SensorStore {
    /// Pojemność mniejsza od 1 jest traktowana jak 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::new(),
            probe: crate::providers::SensorProbe::default(),
        }
    }

    /// Stan źródeł z transportu — moduł czyta go, żeby dobrać notkę.
    pub fn set_probe(&mut self, probe: crate::providers::SensorProbe) {
        self.probe = probe;
    }

    pub fn push(&mut self, elapsed_secs: f64, readings: Vec<SensorReading>) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples
            .push_back(SensorSample::new(elapsed_secs, readings));
    }

    pub fn samples(&self) -> &VecDeque<SensorSample> {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

impl Default for SensorStore {
    fn default() -> Self {
        Self::new(600)
    }
}

impl crate::providers::SensorsSource for SensorStore {
    fn readings(&self) -> Vec<SensorReading> {
        self.samples
            .back()
            .map(|sample| sample.readings.clone())
            .unwrap_or_default()
    }

    fn history(&self) -> Vec<SensorSample> {
        self.samples.iter().cloned().collect()
    }

    fn probe(&self) -> crate::providers::SensorProbe {
        self.probe
    }
}

/// Formatuje wartość czujnika z jednostką. Miejsca po przecinku zależą od
/// wielkości: temperatury i moce mają jedno, napięcia trzy, reszta jest całkowita.
pub fn format_reading(value: f64, unit: SensorUnit) -> String {
    if !value.is_finite() {
        return "-".to_string();
    }
    match unit {
        SensorUnit::Celsius => format!("{value:.1} °C"),
        SensorUnit::Volt => format!("{value:.3} V"),
        SensorUnit::Watt => format!("{value:.1} W"),
        SensorUnit::Megahertz => format!("{value:.0} MHz"),
        SensorUnit::Rpm => format!("{value:.0} RPM"),
        SensorUnit::Percent => format!("{value:.0} %"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::SensorsSource;

    fn reading(kind: SensorKind, value: f64) -> SensorReading {
        SensorReading::new("GPU", DeviceKind::Gpu, kind, value)
    }

    #[test]
    fn units_follow_kind() {
        assert_eq!(
            reading(SensorKind::Temperature, 57.0).unit(),
            SensorUnit::Celsius
        );
        assert_eq!(reading(SensorKind::FanRpm, 663.0).unit(), SensorUnit::Rpm);
        assert_eq!(reading(SensorKind::Power, 37.0).unit(), SensorUnit::Watt);
        assert_eq!(
            reading(SensorKind::CoreClock, 502.0).unit(),
            SensorUnit::Megahertz
        );
        assert!(SensorKind::Hotspot.is_temperature());
        assert!(!SensorKind::FanPercent.is_temperature());
    }

    #[test]
    fn formats_values_with_matching_precision() {
        assert_eq!(format_reading(57.04, SensorUnit::Celsius), "57.0 °C");
        assert_eq!(format_reading(0.868, SensorUnit::Volt), "0.868 V");
        assert_eq!(format_reading(36.9, SensorUnit::Watt), "36.9 W");
        assert_eq!(format_reading(663.0, SensorUnit::Rpm), "663 RPM");
        assert_eq!(format_reading(f64::NAN, SensorUnit::Celsius), "-");
    }

    #[test]
    fn store_evicts_oldest_and_readings_are_the_last_sample() {
        let mut store = SensorStore::new(2);
        assert!(store.readings().is_empty());
        store.push(0.0, vec![reading(SensorKind::Temperature, 40.0)]);
        store.push(1.0, vec![reading(SensorKind::Temperature, 45.0)]);
        store.push(2.0, vec![reading(SensorKind::Temperature, 50.0)]);

        assert_eq!(store.len(), 2);
        let readings = store.readings();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].value, 50.0);
        assert_eq!(store.history()[0].elapsed_secs, 1.0);
    }

    #[test]
    fn sensor_identity_ignores_value_but_not_channel() {
        let first = reading(SensorKind::Temperature, 40.0);
        let second = reading(SensorKind::Temperature, 60.0);
        assert!(first.same_sensor(&second));

        let first_channel = first.clone().with_channel(1);
        assert!(!first.same_sensor(&first_channel));
        assert!(!first.same_sensor(&SensorReading::new(
            "Inny GPU",
            DeviceKind::Gpu,
            SensorKind::Temperature,
            40.0
        )));
    }

    #[test]
    fn new_sensor_kinds_keep_thermals_and_volts_apart() {
        assert_eq!(
            reading(SensorKind::CcdTemp, 55.5).unit(),
            SensorUnit::Celsius
        );
        assert!(SensorKind::CcdTemp.is_temperature());
        assert_eq!(
            reading(SensorKind::SocVoltage, 1.0).unit(),
            SensorUnit::Volt
        );
        assert!(!SensorKind::SocVoltage.is_temperature());
    }

    #[test]
    fn store_exposes_probe_state() {
        use crate::providers::{SensorProbe, SensorsSource};

        let mut store = SensorStore::new(1);
        assert_eq!(store.probe(), SensorProbe::default());
        store.set_probe(SensorProbe {
            ring0: true,
            cpu: true,
            board: false,
        });
        assert!(store.probe().ring0);
        assert_eq!(store.samples().len(), 0);
    }
}
