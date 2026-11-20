//! Czujniki Super I/O (Nuvoton NCT67xx): wentylatory, temperatury płyty i PWM.
//!
//! Super I/O siedzi za mostkiem LPC, więc dostęp wymaga portów I/O w ring 0 —
//! czytamy je modułem `LpcIO` z PawnIO. Układ wykrywamy po identyfikatorze
//! w rejestrach konfiguracyjnych (0x20/0x21), bazę rejestrów bierzemy z LDN 0x0B,
//! a rejestry pomiarowe czytamy z bankowaniem (`0x4E` = wybór banku).
//!
//! Mapowanie „który wentylator to CPU” jest zależne od płyty — tutaj go nie ma,
//! więc moduł pokazuje numery kanałów, a nie nazwy producenta płyty.

use sk_core::error::Result;
use sk_core::sensors::{DeviceKind, SensorKind, SensorReading};

use crate::pawnio::{self, PawnIo};

/// Porty konfiguracyjne Nuvoton/Fintek/Winbond: pierwszy i drugi slot LPC.
const CONFIG_PORTS: [u16; 2] = [0x2E, 0x4E];
/// Rejestr wyboru urządzenia logicznego i numer urządzenia monitoringu.
const REG_DEVICE_SELECT: u8 = 0x07;
const LDN_HARDWARE_MONITOR: u8 = 0x0B;
/// Para indeks/dane w przestrzeni rejestrów monitoringu (baza + 5 / + 6).
const ADDRESS_OFFSET: u16 = 0x05;
const DATA_OFFSET: u16 = 0x06;
const BANK_SELECT_REGISTER: u8 = 0x4E;
/// Blokada przestrzeni I/O monitoringu — bez wyzerowania bitu 4 odczyty dają 0.
const IO_SPACE_LOCK: u8 = 0x28;
const IO_SPACE_LOCK_BIT: u8 = 0x10;
/// Licznik 13-bitowy: 1,35 MHz / count = obroty.
const FAN_CLOCK: f64 = 1.35e6;
const FAN_MAX_COUNT: u32 = 0x1FFF;
const FAN_MIN_COUNT: u32 = 0x15;

/// Rejestry liczników wentylatorów (13-bit, para high/low).
const FAN_COUNT_REGISTERS: [u16; 6] = [0x04B0, 0x04B2, 0x04B4, 0x04B6, 0x04B8, 0x04BA];
/// Rejestry temperatur: (rejestr, rejestr połówki, bit połówki).
const TEMPERATURES: [(u16, u16, u8); 6] = [
    (0x0075, 0x0076, 7), // CPUTIN
    (0x0077, 0x0078, 7), // SYSTIN
    (0x0079, 0x007A, 7), // AUXTIN0
    (0x007B, 0x007C, 7), // AUXTIN1
    (0x007D, 0x007E, 7), // AUXTIN2
    (0x04A0, 0x049E, 6), // AUXTIN3
];

/// Układ, którego rejestry potrafimy czytać. Numery są zgodne z LHM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Nct6779D,
    Nct6791D,
    Nct6792D,
    Nct6793D,
    Nct6795D,
    Nct6796D,
    Nct6797D,
    Nct6799D,
    Nct6701D,
}

impl Chip {
    /// Identyfikator z rejestru 0x20 (starsze układy mają jeden bajt).
    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0xC5 => Self::Nct6779D,
            0xC8 => Self::Nct6791D,
            0xC9 => Self::Nct6792D,
            0xD1 => Self::Nct6793D,
            0xD3 => Self::Nct6795D,
            0xD4 => Self::Nct6796D,
            0xD5 => Self::Nct6797D,
            0xD8 => Self::Nct6799D,
            0x06 => Self::Nct6701D,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Nct6779D => "Nuvoton NCT6779D",
            Self::Nct6791D => "Nuvoton NCT6791D",
            Self::Nct6792D => "Nuvoton NCT6792D",
            Self::Nct6793D => "Nuvoton NCT6793D",
            Self::Nct6795D => "Nuvoton NCT6795D",
            Self::Nct6796D => "Nuvoton NCT6796D",
            Self::Nct6797D => "Nuvoton NCT6797D",
            Self::Nct6799D => "Nuvoton NCT6799D",
            Self::Nct6701D => "Nuvoton NCT6701D",
        }
    }

    /// Rejestry wypełnienia PWM — nowsze układy mają inne adresy dwóch ostatnich.
    fn pwm_registers(self) -> &'static [u16] {
        match self {
            Self::Nct6796D | Self::Nct6797D | Self::Nct6799D | Self::Nct6701D => {
                &[0x0001, 0x0003, 0x0011, 0x0013, 0x0015, 0x0A09, 0x0B09]
            }
            _ => &[0x0001, 0x0003, 0x0011, 0x0013, 0x0015, 0x0017, 0x0029],
        }
    }
}

/// Wykryty układ monitoringu wraz z bazą rejestrów.
pub struct SuperIo {
    session: PawnIo,
    base: u16,
    chip: Chip,
}

impl SuperIo {
    /// Wykrywa układ na obu slotach LPC. `None` = brak Super I/O (laptop, płyta
    /// bez monitoringu) albo brak PawnIO.
    pub fn probe() -> Option<Self> {
        let session = PawnIo::load_module(pawnio::LPC_IO).ok()?;
        for (slot, port) in CONFIG_PORTS.into_iter().enumerate() {
            let slot = u64::try_from(slot).ok()?;
            let _ = session.execute("ioctl_select_slot", &[slot], 0);
            let _ = session.execute("ioctl_find_bars", &[], 0);
            if let Some(chip) = detect(&session, port) {
                return Some(Self {
                    session,
                    base: chip.1,
                    chip: chip.0,
                });
            }
        }
        None
    }

    pub fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        let device = self.chip.name().to_string();
        let mut fan_channels = Vec::new();

        for (index, register) in FAN_COUNT_REGISTERS.iter().enumerate() {
            let Ok(high) = self.read(*register) else {
                continue;
            };
            let Ok(low) = self.read(register + 1) else {
                continue;
            };
            if let Some(rpm) = decode_fan_rpm(high, low) {
                let channel = u8::try_from(index + 1).unwrap_or(1);
                fan_channels.push(channel);
                readings.push(
                    SensorReading::new(device.clone(), DeviceKind::Board, SensorKind::FanRpm, rpm)
                        .with_channel(channel),
                );
            }
        }

        for (index, (register, half, bit)) in TEMPERATURES.iter().enumerate() {
            let (Ok(value), Ok(half_value)) = (self.read(*register), self.read(*half)) else {
                continue;
            };
            if let Some(celsius) = decode_temperature(value, half_value, *bit) {
                let channel = u8::try_from(index + 1).unwrap_or(1);
                readings.push(
                    SensorReading::new(
                        device.clone(),
                        DeviceKind::Board,
                        SensorKind::Temperature,
                        celsius,
                    )
                    .with_channel(channel),
                );
            }
        }

        // PWM tylko dla podłączonych wentylatorów: wypełnienie nagłówka bez
        // tachometru to szum, nie odczyt.
        for (index, register) in self.chip.pwm_registers().iter().enumerate() {
            let channel = u8::try_from(index + 1).unwrap_or(1);
            if !fan_channels.contains(&channel) {
                continue;
            }
            let Ok(value) = self.read(*register) else {
                continue;
            };
            readings.push(
                SensorReading::new(
                    device.clone(),
                    DeviceKind::Board,
                    SensorKind::FanPercent,
                    f64::from(value) / 2.55,
                )
                .with_channel(channel),
            );
        }
        readings
    }

    fn read(&self, address: u16) -> Result<u8> {
        let bank = u8::try_from(address >> 8).unwrap_or(0);
        let register = u8::try_from(address & 0xFF).unwrap_or(0);
        self.write_port(self.base + ADDRESS_OFFSET, BANK_SELECT_REGISTER)?;
        self.write_port(self.base + DATA_OFFSET, bank)?;
        self.write_port(self.base + ADDRESS_OFFSET, register)?;
        self.read_port(self.base + DATA_OFFSET)
    }

    fn read_port(&self, port: u16) -> Result<u8> {
        let value = self
            .session
            .execute_one("ioctl_pio_inb", &[u64::from(port)])?;
        Ok(value as u8)
    }

    fn write_port(&self, port: u16, value: u8) -> Result<()> {
        self.session
            .execute("ioctl_pio_outb", &[u64::from(port), u64::from(value)], 0)?;
        Ok(())
    }
}

/// Wejście w tryb konfiguracji, odczyt ID, baza rejestrów i wyjście.
fn detect(session: &PawnIo, port: u16) -> Option<(Chip, u16)> {
    for _ in 0..2 {
        session
            .execute("ioctl_pio_outb", &[u64::from(port), 0x87], 0)
            .ok()?;
    }

    let detected = inspect(session);
    // Wyjście z trybu konfiguracji musi wykonać się na każdej ścieżce: chip
    // pozostawiony w tym trybie przestaje odpowiadać na kolejne `0x87`.
    let _ = session.execute("ioctl_pio_outb", &[u64::from(port), 0xAA], 0);
    detected
}

fn inspect(session: &PawnIo) -> Option<(Chip, u16)> {
    let id = session.execute_one("ioctl_superio_inb", &[0x20]).ok()? as u8;
    let chip = Chip::from_id(id)?;

    // `ioctl_find_bars` rejestruje zakresy BAR (m.in. bazę monitoringu) w stanie
    // modułu i tylko do nich przepuszcza `ioctl_pio_*`. Musi być wołane **po**
    // wejściu w tryb konfiguracji — inaczej moduł zwraca `DEVICE_NOT_READY`,
    // a odczyty rejestrów kończą się `ACCESS_DENIED`.
    let _ = session.execute("ioctl_find_bars", &[], 0);
    let _ = session.execute(
        "ioctl_superio_outb",
        &[
            u64::from(REG_DEVICE_SELECT),
            u64::from(LDN_HARDWARE_MONITOR),
        ],
        0,
    );
    let selected = session
        .execute_one("ioctl_superio_inb", &[u64::from(REG_DEVICE_SELECT)])
        .ok()? as u8;
    if selected != LDN_HARDWARE_MONITOR {
        return None;
    }

    let first = session.execute_one("ioctl_superio_inw", &[0x60]).ok()?;
    let second = session.execute_one("ioctl_superio_inw", &[0x60]).ok()?;

    // Blokada przestrzeni I/O zeruje odczyty monitoringu na NCT679x.
    if let Ok(lock) = session.execute_one("ioctl_superio_inb", &[u64::from(IO_SPACE_LOCK)])
        && lock as u8 & IO_SPACE_LOCK_BIT != 0
    {
        let _ = session.execute(
            "ioctl_superio_outb",
            &[
                u64::from(IO_SPACE_LOCK),
                u64::from(lock as u8 & !IO_SPACE_LOCK_BIT),
            ],
            0,
        );
    }

    if first != second {
        return None;
    }
    let base = first as u16;
    if base < 0x100 || base == 0xFFFF {
        return None;
    }
    Some((chip, base))
}

/// 13-bitowy licznik obrotów: `(high << 5) | (low & 0x1F)`, obroty = 1,35 MHz / count.
pub fn decode_fan_rpm(high: u8, low: u8) -> Option<f64> {
    let count = (u32::from(high) << 5) | u32::from(low & 0x1F);
    if !(FAN_MIN_COUNT..FAN_MAX_COUNT).contains(&count) {
        return None;
    }
    Some(FAN_CLOCK / f64::from(count))
}

/// Temperatura: starszy bajt ze znakiem plus bit połówki z drugiego rejestru.
pub fn decode_temperature(value: u8, half: u8, bit: u8) -> Option<f64> {
    let integer = i16::from(value as i8);
    if integer == 0 {
        return None;
    }
    let fraction = f64::from((half >> bit) & 1);
    let celsius = f64::from(integer) + 0.5 * fraction;
    (celsius > 0.0 && celsius < 125.0).then_some(celsius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_ids_map_to_names() {
        assert_eq!(Chip::from_id(0xD3), Some(Chip::Nct6795D));
        assert_eq!(Chip::Nct6795D.name(), "Nuvoton NCT6795D");
        assert_eq!(Chip::from_id(0xC5), Some(Chip::Nct6779D));
        assert_eq!(Chip::from_id(0x1F), None);
    }

    #[test]
    fn fan_rpm_uses_13_bit_counter() {
        // 0x4B0/0x4B1 z tej płyty: 38 / 30 → count 1246 → 1083 RPM.
        let rpm = decode_fan_rpm(38, 30).unwrap();
        assert!((rpm - 1083.5).abs() < 0.5, "{rpm}");
        // Licznik na maksimum = wentylator niepodłączony.
        assert_eq!(decode_fan_rpm(0xFF, 0x1F), None);
        // Poniżej progu minimalnego = brak tachometru.
        assert_eq!(decode_fan_rpm(0, 0x10), None);
    }

    #[test]
    fn temperatures_add_half_degree_and_skip_zero() {
        assert_eq!(decode_temperature(51, 0, 7), Some(51.0));
        assert_eq!(decode_temperature(40, 0x80, 7), Some(40.5));
        assert_eq!(decode_temperature(0, 0, 7), None);
        // Ujemne (uszkodzony czujnik) odrzucamy.
        assert_eq!(decode_temperature(0xF0, 0, 7), None);
    }

    #[test]
    fn newer_chips_use_different_pwm_registers() {
        assert_eq!(Chip::Nct6795D.pwm_registers()[5], 0x0017);
        assert_eq!(Chip::Nct6799D.pwm_registers()[5], 0x0A09);
    }

    /// Wymaga PawnIO i płyty z Super I/O (desktop).
    #[test]
    #[ignore = "wymaga PawnIO i płyty z Super I/O"]
    fn hardware_superio_reports_fans_and_temperatures() {
        let board = SuperIo::probe().expect("Super I/O na tej płycie");
        let readings = board.readings();
        println!("Super I/O {}: {readings:#?}", board.chip.name());
        assert!(
            readings
                .iter()
                .any(|reading| reading.kind == SensorKind::Temperature),
            "brak temperatury płyty"
        );
        assert!(
            readings
                .iter()
                .any(|reading| reading.kind == SensorKind::FanRpm && reading.value > 100.0),
            "brak obrotów wentylatora"
        );
    }
}
