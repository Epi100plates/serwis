//! Parsery SMART: strona logu zdrowia NVMe i atrybuty ATA.
//!
//! Ten plik nie dotyka systemu — dostaje surowe bajty z `sk-win` i zwraca dane
//! domenowe. Dzięki temu każdy przypadek (dysk zużyty, sektory oczekujące,
//! temperatura w kelwinach) testuje się bez dysku i bez uprawnień.

use std::fmt;

/// Ile bajtów ma sektor danych SMART.
pub const SECTOR: usize = 512;

/// Atrybuty ATA, których nazwy mamy przetłumaczone. Reszta trafia do raportu
/// z numerem — dysk może zwrócić atrybut producenta, którego nikt nie opisze.
pub const KNOWN_ATTRIBUTE_IDS: &[u8] = &[
    1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13, 22, 160, 183, 184, 187, 188, 189, 190, 191, 192, 193,
    194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 220, 221,
    222, 223, 224, 225, 226, 230, 231, 232, 233, 234, 235, 240, 241, 242, 250, 251,
];

/// Atrybuty, których niezerowa wartość surowa oznacza realny problem: przealokowane
/// sektory, ponowienia rozruchu, błędy end-to-end, niekorygowalne błędy, przekroczenia
/// czasu polecenia, zdarzenia realokacji, sektory oczekujące/offline i błędy CRC transmisji.
pub const SUSPICIOUS_ATTRIBUTE_IDS: &[u8] = &[5, 10, 184, 187, 188, 196, 197, 198, 199];

/// Błąd parsowania danych SMART — zawsze z powodem, bo „nie działa” nic nie mówi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartParseError {
    /// Bufor krótszy niż wymagany przez format.
    TooShort { expected: usize, actual: usize },
    /// Dysk zwrócił sektor, w którym nie ma ani jednego atrybutu.
    NoAttributes,
}

impl fmt::Display for SmartParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { expected, actual } => write!(
                formatter,
                "dane SMART mają {actual} B, a format wymaga {expected} B"
            ),
            Self::NoAttributes => write!(
                formatter,
                "sektor atrybutów ATA nie zawiera żadnego atrybutu"
            ),
        }
    }
}

impl std::error::Error for SmartParseError {}

/// Ocena stanu nośnika. `Bad` nie znaczy „dysk już nie działa”, tylko „dane
/// mówią o awarii lub końcu zapasu”.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Warning,
    Bad,
}

/// Strona logu zdrowia NVMe (SMART/Health Information, log page 0x02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    /// Temperatura złożona w °C; `None`, gdy kontroler jej nie raportuje.
    pub temperature_c: Option<i16>,
    pub available_spare: u8,
    pub spare_threshold: u8,
    pub percentage_used: u8,
    /// Liczniki NVMe są 128-bitowe; w praktyce mieszczą się w `u64`.
    pub data_read_bytes: u128,
    pub data_written_bytes: u128,
    pub power_on_hours: u64,
    pub power_cycles: u64,
    pub controller_busy_minutes: u64,
    pub unsafe_shutdowns: u64,
    pub media_errors: u64,
    pub error_log_entries: u64,
    pub warning_temp_minutes: u32,
    pub critical_temp_minutes: u32,
    /// Czujniki 1–8 w °C; `None` to brak pomiaru (kontroler zwrócił zero).
    pub sensors_c: Vec<Option<i16>>,
}

impl NvmeHealth {
    /// Bity `Critical Warning`, które mają znaczenie dla technika.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        for (bit, warning) in [
            (0, Warning::SpareBelowThreshold),
            (1, Warning::Temperature),
            (2, Warning::Reliability),
            (3, Warning::ReadOnly),
            (4, Warning::VolatileBackup),
            (5, Warning::PersistentReadOnly),
        ] {
            if self.critical_warning & (1 << bit) != 0 {
                warnings.push(warning);
            }
        }
        warnings
    }
}

/// Znaczenie bitów `Critical Warning` ze strony logu zdrowia NVMe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    SpareBelowThreshold,
    Temperature,
    Reliability,
    ReadOnly,
    VolatileBackup,
    PersistentReadOnly,
}

impl Warning {
    /// Klucz tłumaczenia — ten sam dla GUI i raportu HTML.
    pub fn key(self) -> &'static str {
        match self {
            Self::SpareBelowThreshold => "module.storage_smart.warn.spare",
            Self::Temperature => "module.storage_smart.warn.temperature",
            Self::Reliability => "module.storage_smart.warn.reliability",
            Self::ReadOnly => "module.storage_smart.warn.read_only",
            Self::VolatileBackup => "module.storage_smart.warn.volatile_backup",
            Self::PersistentReadOnly => "module.storage_smart.warn.persistent_read_only",
        }
    }
}

/// Zamienia kelwiny z NVMe na stopnie Celsjusza. Zero oznacza „brak pomiaru”.
fn kelvin_to_celsius(kelvin: u16) -> Option<i16> {
    if kelvin == 0 {
        return None;
    }
    Some((i32::from(kelvin) - 273).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
}

fn u128_at(data: &[u8], offset: usize) -> u128 {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&data[offset..offset + 16]);
    u128::from_le_bytes(bytes)
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&data[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}

/// Liczniki 128-bitowe NVMe bywają ogromne; do raportu wystarczy `u64`.
fn saturating_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Parsuje stronę logu zdrowia NVMe. Wymaga pełnych 512 B — taką porcję
/// dokumentacja Microsoftu każe pobierać (`NVMeDataTypeLogPage`).
pub fn parse_nvme_health(data: &[u8]) -> Result<NvmeHealth, SmartParseError> {
    if data.len() < SECTOR {
        return Err(SmartParseError::TooShort {
            expected: SECTOR,
            actual: data.len(),
        });
    }

    let temperature_kelvin = u16::from_le_bytes([data[1], data[2]]);
    let sensors_c = (200..216)
        .step_by(2)
        .map(|offset| {
            let kelvin = u16::from_le_bytes([data[offset], data[offset + 1]]);
            kelvin_to_celsius(kelvin)
        })
        .collect();

    Ok(NvmeHealth {
        critical_warning: data[0],
        temperature_c: kelvin_to_celsius(temperature_kelvin),
        available_spare: data[3],
        spare_threshold: data[4],
        percentage_used: data[5],
        // Jednostka NVMe to 1000 sektorów po 512 B.
        data_read_bytes: u128_at(data, 32).saturating_mul(1000 * SECTOR as u128),
        data_written_bytes: u128_at(data, 48).saturating_mul(1000 * SECTOR as u128),
        controller_busy_minutes: saturating_u64(u128_at(data, 96)),
        power_cycles: saturating_u64(u128_at(data, 112)),
        power_on_hours: saturating_u64(u128_at(data, 128)),
        unsafe_shutdowns: saturating_u64(u128_at(data, 144)),
        media_errors: saturating_u64(u128_at(data, 160)),
        error_log_entries: saturating_u64(u128_at(data, 176)),
        warning_temp_minutes: u32_at(data, 192),
        critical_temp_minutes: u32_at(data, 196),
        sensors_c,
    })
}

/// Ocena stanu dysku NVMe na podstawie logu zdrowia.
///
/// `Bad` = kontroler zgłasza degradację niezawodności albo nośnik jest tylko do
/// odczytu. `Warning` = zapas bloków pod progiem, temperatura, błędy medium,
/// zużycie od 80 %. Zużycie 100 % to **wyczerpany zapas projektowy**, nie awaria:
/// specyfikacja NVMe pozwala przekroczyć 100 i dysk może nadal pracować, więc
/// taki stan pozostaje ostrzeżeniem.
pub fn nvme_health(health: &NvmeHealth) -> Health {
    let reliability_lost = health.critical_warning & 0b0000_1100 != 0;
    if reliability_lost {
        return Health::Bad;
    }
    if health.critical_warning != 0
        || health.media_errors > 0
        || health.percentage_used >= 80
        || health.available_spare <= health.spare_threshold
    {
        return Health::Warning;
    }
    Health::Good
}

/// Jeden atrybut ATA SMART, już z progiem z osobnego sektora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtaAttribute {
    pub id: u8,
    pub flags: u16,
    /// Wartość znormalizowana (im wyżej, tym lepiej).
    pub value: u8,
    pub worst: u8,
    /// Próg producenta; 0 = atrybut nie ma progu (np. licznik godzin).
    pub threshold: u8,
    /// Surowe 6 bajtów jako liczba. Dla godzin, cykli i LBA to wartość
    /// bezpośrednia; dla temperatur zwykle bajt 0.
    pub raw: u64,
}

impl AtaAttribute {
    /// Czy atrybut przekroczył próg producenta (stan przedawaryjny).
    pub fn threshold_exceeded(&self) -> bool {
        self.threshold != 0 && self.value <= self.threshold
    }
}

/// Parsuje sektor atrybutów ATA SMART (`SMART READ DATA`).
///
/// Układ: bajt 0 = rewizja, bajt 1 = rezerwa, od bajtu 2 trzydzieści wpisów
/// po 12 bajtów: `id`, `flags` (2 B), `value`, `worst`, `raw` (6 B), rezerwa.
pub fn parse_ata_attributes(
    attributes_sector: &[u8],
    thresholds_sector: &[u8],
) -> Result<Vec<AtaAttribute>, SmartParseError> {
    if attributes_sector.len() < SECTOR {
        return Err(SmartParseError::TooShort {
            expected: SECTOR,
            actual: attributes_sector.len(),
        });
    }
    let thresholds = parse_ata_thresholds(thresholds_sector);

    let mut attributes = Vec::new();
    for slot in 0..30 {
        let entry = 2 + slot * 12;
        let id = attributes_sector[entry];
        if id == 0 {
            continue;
        }
        let mut raw_bytes = [0u8; 8];
        raw_bytes[..6].copy_from_slice(&attributes_sector[entry + 5..entry + 11]);
        attributes.push(AtaAttribute {
            id,
            flags: u16::from_le_bytes([attributes_sector[entry + 1], attributes_sector[entry + 2]]),
            value: attributes_sector[entry + 3],
            worst: attributes_sector[entry + 4],
            threshold: thresholds
                .iter()
                .find(|(threshold_id, _)| *threshold_id == id)
                .map_or(0, |(_, threshold)| *threshold),
            raw: u64::from_le_bytes(raw_bytes),
        });
    }

    if attributes.is_empty() {
        return Err(SmartParseError::NoAttributes);
    }
    Ok(attributes)
}

/// Progi producenta: ten sam układ co atrybuty, ale zamiast wartości i surowych
/// bajtów jest jeden bajt progu.
fn parse_ata_thresholds(sector: &[u8]) -> Vec<(u8, u8)> {
    if sector.len() < SECTOR {
        return Vec::new();
    }
    (0..30)
        .filter_map(|slot| {
            let entry = 2 + slot * 12;
            let id = sector[entry];
            (id != 0).then_some((id, sector[entry + 1]))
        })
        .collect()
}

/// Ocena stanu dysku ATA.
///
/// Przekroczony próg producenta to od razu `Bad` — to sygnał, którym firmware
/// mówi „atrybut przedawaryjny”. Surowe liczniki (przealokowane sektory,
/// sektory oczekujące, błędy CRC) dają `Warning`, bo pojedynczy incydent
/// zwykle nie oznacza jeszcze końca dysku.
pub fn ata_health(attributes: &[AtaAttribute]) -> Health {
    if attributes.iter().any(AtaAttribute::threshold_exceeded) {
        return Health::Bad;
    }

    let suspicious = attributes
        .iter()
        .any(|attribute| attribute.raw > 0 && SUSPICIOUS_ATTRIBUTE_IDS.contains(&attribute.id));

    if suspicious {
        Health::Warning
    } else {
        Health::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buduje stronę logu zdrowia NVMe z podanymi wartościami.
    fn nvme_page(
        critical_warning: u8,
        temperature_kelvin: u16,
        used_percent: u8,
        written_units: u128,
    ) -> Vec<u8> {
        let mut page = vec![0u8; SECTOR];
        page[0] = critical_warning;
        page[1..3].copy_from_slice(&temperature_kelvin.to_le_bytes());
        page[3] = 100; // available spare
        page[4] = 10; // spare threshold
        page[5] = used_percent;
        page[48..64].copy_from_slice(&written_units.to_le_bytes());
        page[128..144].copy_from_slice(&1_234u128.to_le_bytes());
        page[160..176].copy_from_slice(&0u128.to_le_bytes());
        page
    }

    #[test]
    fn parses_nvme_health_page() {
        let mut page = nvme_page(0, 273 + 41, 3, 2_000_000);
        page[112..128].copy_from_slice(&321u128.to_le_bytes());
        page[144..160].copy_from_slice(&7u128.to_le_bytes());
        page[200..202].copy_from_slice(&(273u16 + 38).to_le_bytes());
        page[202..204].copy_from_slice(&0u16.to_le_bytes());

        let health = parse_nvme_health(&page).expect("poprawna strona logu");
        assert_eq!(health.temperature_c, Some(41));
        assert_eq!(health.available_spare, 100);
        assert_eq!(health.spare_threshold, 10);
        assert_eq!(health.percentage_used, 3);
        assert_eq!(health.power_on_hours, 1_234);
        assert_eq!(health.power_cycles, 321);
        assert_eq!(health.unsafe_shutdowns, 7);
        // 2 000 000 jednostek × 1000 × 512 B = ~1,02 TB
        assert_eq!(health.data_written_bytes, 2_000_000u128 * 1000 * 512);
        // Zero w czujniku to brak pomiaru, nie 0 °C — numeracja zostaje.
        assert_eq!(
            health.sensors_c,
            vec![Some(38), None, None, None, None, None, None, None]
        );
        assert_eq!(nvme_health(&health), Health::Good);
    }

    #[test]
    fn nvme_temperature_zero_means_no_reading() {
        let page = nvme_page(0, 0, 0, 0);
        let health = parse_nvme_health(&page).expect("strona logu");
        assert_eq!(health.temperature_c, None);
    }

    #[test]
    fn rejects_short_nvme_page() {
        let error = parse_nvme_health(&[0u8; 64]).expect_err("za krótki bufor");
        assert_eq!(
            error,
            SmartParseError::TooShort {
                expected: SECTOR,
                actual: 64
            }
        );
    }

    #[test]
    fn nvme_health_flags_reliability_degradation() {
        let page = nvme_page(0b0000_0100, 300, 2, 0);
        let health = parse_nvme_health(&page).expect("strona logu");
        assert_eq!(nvme_health(&health), Health::Bad);
        assert_eq!(health.warnings(), vec![Warning::Reliability]);
    }

    #[test]
    fn nvme_health_flags_read_only_and_warnings() {
        let read_only = parse_nvme_health(&nvme_page(0b0000_1000, 300, 2, 0)).expect("strona");
        assert_eq!(nvme_health(&read_only), Health::Bad);

        let temperature = parse_nvme_health(&nvme_page(0b0000_0010, 300, 2, 0)).expect("strona");
        assert_eq!(nvme_health(&temperature), Health::Warning);
        assert_eq!(temperature.warnings(), vec![Warning::Temperature]);
        assert_eq!(
            Warning::Temperature.key(),
            "module.storage_smart.warn.temperature"
        );
    }

    #[test]
    fn nvme_health_warns_on_media_errors_and_wear() {
        let mut with_errors = nvme_page(0, 300, 2, 0);
        with_errors[160..176].copy_from_slice(&4u128.to_le_bytes());
        assert_eq!(
            nvme_health(&parse_nvme_health(&with_errors).expect("strona")),
            Health::Warning
        );

        for used in [79u8, 80, 99, 100, 254, 255] {
            let expected = if used >= 80 {
                Health::Warning
            } else {
                Health::Good
            };
            assert_eq!(
                nvme_health(&parse_nvme_health(&nvme_page(0, 300, used, 0)).expect("strona")),
                expected,
                "zużycie {used} %"
            );
        }
    }

    #[test]
    fn nvme_health_warns_when_spare_reaches_threshold() {
        let mut page = nvme_page(0, 300, 2, 0);
        page[3] = 10; // available spare == threshold
        assert_eq!(
            nvme_health(&parse_nvme_health(&page).expect("strona")),
            Health::Warning
        );
    }

    /// Buduje sektor atrybutów ATA z podanych `(id, value, worst, raw)`.
    fn ata_sector(attributes: &[(u8, u8, u8, u64)]) -> Vec<u8> {
        let mut sector = vec![0u8; SECTOR];
        sector[0] = 0x10; // rewizja ATA8
        for (slot, (id, value, worst, raw)) in attributes.iter().enumerate() {
            let entry = 2 + slot * 12;
            sector[entry] = *id;
            sector[entry + 1] = 0x03;
            sector[entry + 2] = 0x00;
            sector[entry + 3] = *value;
            sector[entry + 4] = *worst;
            sector[entry + 5..entry + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
        }
        sector
    }

    fn thresholds_sector(thresholds: &[(u8, u8)]) -> Vec<u8> {
        let mut sector = vec![0u8; SECTOR];
        sector[0] = 0x10;
        for (slot, (id, threshold)) in thresholds.iter().enumerate() {
            let entry = 2 + slot * 12;
            sector[entry] = *id;
            sector[entry + 1] = *threshold;
        }
        sector
    }

    #[test]
    fn parses_ata_attributes_with_thresholds() {
        let attributes = ata_sector(&[(5, 100, 100, 0), (9, 99, 99, 12_345), (194, 70, 50, 38)]);
        let thresholds = thresholds_sector(&[(5, 10), (194, 0)]);
        let parsed = parse_ata_attributes(&attributes, &thresholds).expect("poprawne atrybuty");

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].id, 9);
        assert_eq!(parsed[1].raw, 12_345);
        assert_eq!(parsed[1].threshold, 0, "atrybut bez progu");
        assert_eq!(parsed[0].threshold, 10);
        assert_eq!(parsed[2].value, 70);
        assert_eq!(parsed[2].worst, 50);
        assert_eq!(parsed[2].raw, 38);
        assert_eq!(ata_health(&parsed), Health::Good);
    }

    #[test]
    fn ata_health_is_bad_when_threshold_is_hit() {
        let attributes = ata_sector(&[(5, 10, 20, 8)]);
        let parsed =
            parse_ata_attributes(&attributes, &thresholds_sector(&[(5, 10)])).expect("atrybuty");
        assert!(parsed[0].threshold_exceeded());
        assert_eq!(ata_health(&parsed), Health::Bad);
    }

    #[test]
    fn ata_health_warns_on_reallocated_and_pending_sectors() {
        for id in SUSPICIOUS_ATTRIBUTE_IDS {
            let attributes = ata_sector(&[(*id, 100, 100, 2)]);
            let parsed = parse_ata_attributes(&attributes, &thresholds_sector(&[(*id, 1)]))
                .expect("atrybuty");
            assert_eq!(ata_health(&parsed), Health::Warning, "atrybut {id}");
        }
    }

    #[test]
    fn ata_health_ignores_high_raw_values_of_benign_attributes() {
        // 1 (raw read error rate) i 7 (seek error) mają surowe liczniki, które
        // na sprawnym dysku też bywają niezerowe — nie są sygnałem awarii.
        let attributes = ata_sector(&[(1, 100, 100, 12345), (7, 100, 100, 999), (9, 99, 99, 500)]);
        let parsed = parse_ata_attributes(&attributes, &thresholds_sector(&[])).expect("atrybuty");
        assert_eq!(ata_health(&parsed), Health::Good);
    }

    #[test]
    fn rejects_empty_attribute_sector() {
        let error = parse_ata_attributes(&vec![0u8; SECTOR], &vec![0u8; SECTOR])
            .expect_err("brak atrybutów");
        assert_eq!(error, SmartParseError::NoAttributes);
    }

    #[test]
    fn every_known_attribute_id_is_unique() {
        let mut ids = KNOWN_ATTRIBUTE_IDS.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN_ATTRIBUTE_IDS.len());
    }

    #[test]
    fn suspicious_attributes_are_all_known_by_name() {
        for id in SUSPICIOUS_ATTRIBUTE_IDS {
            assert!(
                KNOWN_ATTRIBUTE_IDS.contains(id),
                "atrybut {id} ma regułę, ale nie ma nazwy"
            );
        }
    }
}
