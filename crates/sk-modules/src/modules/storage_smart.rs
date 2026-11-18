//! `storage.smart` — stan dysków: strona logu zdrowia NVMe i atrybuty ATA/SATA.
//!
//! Moduł czyta tożsamość dysków z WMI (bez uprawnień), a surowe dane SMART
//! z `StorageProbe` (wymaga administratora). Liczy się to, czy dysk ma zapas
//! bloków, ile przepracował i czy firmware zgłasza stan przedawaryjny.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::smart::{self, AtaAttribute, Health, KNOWN_ATTRIBUTE_IDS, NvmeHealth};
use crate::util::{boolean_key, decode_all, format_bytes, number, text, trimmed};

/// Wszystkie napędy z WMI — ten sam zestaw pól czyta `hw.disk_inventory`,
/// więc cache WMI obsługuje oba moduły jednym zapytaniem.
const DISK_QUERY: &str = queries::WIN32_DISK_DRIVE;

#[derive(Debug, Default, Deserialize)]
struct DiskDrive {
    #[serde(rename = "Index", default)]
    index: Option<u32>,
    #[serde(rename = "Model", default)]
    model: Option<String>,
    #[serde(rename = "SerialNumber", default)]
    serial: Option<String>,
    #[serde(rename = "FirmwareRevision", default)]
    firmware: Option<String>,
    #[serde(rename = "Size", default)]
    size_bytes: Option<u64>,
    #[serde(rename = "InterfaceType", default)]
    interface_type: Option<String>,
}

/// Co udało się ustalić o jednym dysku — razem z powodem braku danych.
struct DriveReport {
    index: u32,
    model: String,
    serial: String,
    firmware: String,
    size_bytes: Option<u64>,
    interface_type: String,
    bus: BusKind,
    kind: DriveKind,
    removable: bool,
    /// Napęd bez nośnika (czytnik kart, pusta kieszeń) — nie ma czego odpytywać.
    no_medium: bool,
    protocol: Option<&'static str>,
    health: Option<Health>,
    nvme: Option<NvmeHealth>,
    ata: Option<Vec<AtaAttribute>>,
    error: Option<ModuleError>,
}

impl DriveReport {
    fn from_disk(disk: &DiskDrive) -> Self {
        Self {
            index: disk.index.unwrap_or(0),
            model: text(disk.model.as_deref()),
            serial: text(disk.serial.as_deref()),
            firmware: text(disk.firmware.as_deref()),
            size_bytes: disk.size_bytes,
            interface_type: text(disk.interface_type.as_deref()),
            bus: BusKind::Unknown,
            kind: DriveKind::Unknown,
            removable: false,
            no_medium: false,
            protocol: None,
            health: None,
            nvme: None,
            ata: None,
            error: None,
        }
    }

    fn has_smart(&self) -> bool {
        self.nvme.is_some() || self.ata.is_some()
    }

    fn temperature_c(&self) -> Option<i16> {
        self.nvme
            .as_ref()
            .and_then(|health| health.temperature_c)
            .or_else(|| self.ata.as_deref().and_then(ata_temperature))
    }

    fn power_on_hours(&self) -> Option<u64> {
        self.nvme
            .as_ref()
            .map(|health| health.power_on_hours)
            .or_else(|| {
                self.ata
                    .as_deref()
                    .and_then(|attributes| ata_raw(attributes, 9))
                    .map(|raw| raw & 0xFFFF_FFFF)
            })
    }

    fn written_bytes(&self) -> Option<u64> {
        let nvme = self.nvme.as_ref()?;
        // Zero w liczniku NVMe znaczy „nie raportowano”, a nie zero bajtów.
        (nvme.data_written_bytes > 0)
            .then(|| nvme.data_written_bytes.min(u128::from(u64::MAX)) as u64)
    }
}

#[derive(Debug, Default)]
pub struct StorageSmart;

impl Module for StorageSmart {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "storage.smart",
            name_key: "module.storage_smart.name",
            desc_key: "module.storage_smart.desc",
            group: Group::Storage,
            // Bez administratora `\\.\PhysicalDriveN` zwraca odmowę dostępu.
            capability: Capability::Elevated,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let disks: Vec<DiskDrive> =
            decode_all(&ctx.wmi.query(NS_CIMV2, DISK_QUERY)?, "Win32_DiskDrive")?;

        if disks.is_empty() {
            return Ok(ModuleOutput {
                data: serde_json::json!({
                    "schema_version": 1,
                    "elevated": ctx.host.is_elevated(),
                    "drives": [],
                    "drives_with_smart": 0,
                }),
                view: View::new(vec![
                    Section::new("module.storage_smart.section").with_blocks(vec![Block::note(
                        NoteLevel::Warn,
                        "module.storage_smart.note.none",
                    )]),
                ]),
            });
        }

        let mut reports: Vec<DriveReport> = Vec::new();
        for disk in &disks {
            let mut report = DriveReport::from_disk(disk);

            // Napęd bez nośnika nie ma czego raportować — nawet deskryptora nie
            // ma sensu pytać, a IOCTL zwróciłby „urządzenie niegotowe”, co nie
            // jest błędem SMART.
            if report.size_bytes.is_none() {
                report.no_medium = true;
                report.error = Some(ModuleError::unavailable("brak nośnika"));
                reports.push(report);
                continue;
            }

            // Deskryptor daje magistralę i rodzaj nośnika; brak dostępu nie
            // przerywa modułu — pokazujemy wtedy mniej, ale nie zgadujemy.
            if let Ok(drive) = ctx.storage.describe(report.index) {
                report.bus = drive.bus;
                report.kind = drive.kind;
                report.removable = drive.removable;
            }

            match ctx.storage.smart(report.index) {
                Ok(SmartData::Nvme { log_page }) => match smart::parse_nvme_health(&log_page) {
                    Ok(health) => {
                        report.health = Some(smart::nvme_health(&health));
                        report.protocol = Some("nvme");
                        report.nvme = Some(health);
                    }
                    Err(err) => {
                        report.error = Some(ModuleError::provider("SMART NVMe", err));
                    }
                },
                Ok(SmartData::Ata {
                    attributes,
                    thresholds,
                }) => match smart::parse_ata_attributes(&attributes, &thresholds) {
                    Ok(attributes) => {
                        report.health = Some(smart::ata_health(&attributes));
                        report.protocol = Some("ata");
                        report.ata = Some(attributes);
                    }
                    Err(err) => {
                        report.error = Some(ModuleError::provider("SMART ATA", err));
                    }
                },
                Err(err) => report.error = Some(err),
            }

            reports.push(report);
        }

        Ok(self.render(ctx, &reports))
    }
}

impl StorageSmart {
    fn render(&self, ctx: &CollectCtx<'_>, reports: &[DriveReport]) -> ModuleOutput {
        let elevated = ctx.host.is_elevated();
        let mut overview = Table::new(
            vec![
                "module.storage_smart.col.drive".to_string(),
                "module.storage_smart.col.bus".to_string(),
                "module.storage_smart.col.kind".to_string(),
                "module.storage_smart.col.health".to_string(),
                "module.storage_smart.col.temperature".to_string(),
                "module.storage_smart.col.hours".to_string(),
                "module.storage_smart.col.written".to_string(),
            ],
            reports
                .iter()
                .map(|report| {
                    vec![
                        report.model.clone(),
                        bus_label(report.bus).to_string(),
                        String::new(),
                        String::new(),
                        report
                            .temperature_c()
                            .map_or_else(|| "-".to_string(), |value| format!("{value} °C")),
                        report
                            .power_on_hours()
                            .map_or_else(|| "-".to_string(), |hours| format!("{hours} h")),
                        report
                            .written_bytes()
                            .map_or_else(|| "-".to_string(), format_bytes),
                    ]
                })
                .collect(),
        );
        for (row, report) in reports.iter().enumerate() {
            overview = overview.with_value_key(row, 2, kind_key(report.kind));
            overview = overview.with_value_key(row, 3, health_key(report.health));
            if report.bus == BusKind::Unknown {
                overview = overview.with_value_key(row, 1, "common.value.unknown");
            }
        }

        let mut overview_blocks = vec![Block::Table { table: overview }];
        if !elevated {
            overview_blocks.push(Block::note(
                NoteLevel::Warn,
                "module.storage_smart.note.elevation",
            ));
        }
        if reports.iter().any(|report| {
            matches!(&report.error, Some(ModuleError::Unavailable { .. }))
                && matches!(report.bus, BusKind::Usb | BusKind::Raid)
        }) {
            overview_blocks.push(Block::note(
                NoteLevel::Info,
                "module.storage_smart.note.bridge",
            ));
        }

        let mut sections =
            vec![Section::new("module.storage_smart.section").with_blocks(overview_blocks)];

        for report in reports {
            let mut section = Section::new("module.storage_smart.section.drive");
            section.subtitle = Some(format!(
                "{} · \\\\.\\PhysicalDrive{}",
                report.model, report.index
            ));
            let mut blocks = vec![Block::key_values(drive_rows(report))];

            if report.no_medium {
                blocks.push(Block::note(
                    NoteLevel::Info,
                    "module.storage_smart.note.no_media",
                ));
            } else {
                match &report.error {
                    Some(ModuleError::AccessDenied { .. }) => blocks.push(Block::note(
                        NoteLevel::Error,
                        "module.storage_smart.note.denied",
                    )),
                    Some(ModuleError::Unavailable { .. }) => blocks.push(Block::note(
                        NoteLevel::Info,
                        "module.storage_smart.note.unavailable",
                    )),
                    Some(_) => blocks.push(Block::note(
                        NoteLevel::Error,
                        "module.storage_smart.note.failed",
                    )),
                    None => {}
                }
            }

            if let Some(health) = &report.nvme {
                for warning in health.warnings() {
                    blocks.push(Block::note(NoteLevel::Warn, warning.key()));
                }
                blocks.push(Block::key_values(nvme_rows(health)));
            }

            if let Some(attributes) = &report.ata {
                match report.health {
                    // Przekroczony próg producenta to stan przedawaryjny.
                    Some(Health::Bad)
                        if attributes.iter().any(AtaAttribute::threshold_exceeded) =>
                    {
                        blocks.push(Block::note(
                            NoteLevel::Error,
                            "module.storage_smart.note.threshold_exceeded",
                        ));
                    }
                    // Ostrzeżenie przy ATA zawsze pochodzi z niezerowych liczników.
                    Some(Health::Warning) => blocks.push(Block::note(
                        NoteLevel::Warn,
                        "module.storage_smart.note.counters",
                    )),
                    _ => {}
                }
                blocks.push(Block::Table {
                    table: attribute_table(attributes),
                });
            }

            section.blocks = blocks;
            sections.push(section);
        }

        let data = serde_json::json!({
            "schema_version": 1,
            "elevated": elevated,
            "drives": reports.iter().map(drive_json).collect::<Vec<_>>(),
            "drives_with_smart": reports.iter().filter(|report| report.has_smart()).count(),
        });

        ModuleOutput {
            data,
            view: View::new(sections),
        }
    }
}

/// Wiersze tożsamości dysku — wspólne dla NVMe i ATA.
fn drive_rows(report: &DriveReport) -> Vec<KeyValue> {
    vec![
        KeyValue::new("module.storage_smart.drive.model", report.model.clone()),
        KeyValue::new("module.storage_smart.drive.serial", report.serial.clone()),
        KeyValue::new(
            "module.storage_smart.drive.firmware",
            report.firmware.clone(),
        ),
        KeyValue::new(
            "module.storage_smart.drive.size",
            report
                .size_bytes
                .map_or_else(|| "-".to_string(), format_bytes),
        ),
        KeyValue::new(
            "module.storage_smart.drive.interface",
            report.interface_type.clone(),
        ),
        bus_row(report.bus),
        KeyValue::state("module.storage_smart.drive.kind", kind_key(report.kind)),
        KeyValue::state(
            "module.storage_smart.drive.health",
            health_key(report.health),
        ),
        KeyValue::state(
            "module.storage_smart.drive.removable",
            boolean_key(report.removable),
        ),
    ]
}

/// Szczegóły NVMe ze strony logu zdrowia.
fn nvme_rows(health: &NvmeHealth) -> Vec<KeyValue> {
    let mut rows = vec![
        KeyValue::new(
            "module.storage_smart.nvme.critical_warning",
            format!("0x{:02x}", health.critical_warning),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.temperature",
            health
                .temperature_c
                .map_or_else(|| "-".to_string(), |value| format!("{value} °C")),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.sensors",
            numbered_sensors(&health.sensors_c),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.available_spare",
            format!("{} %", health.available_spare),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.spare_threshold",
            format!("{} %", health.spare_threshold),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.percentage_used",
            format!("{} %", health.percentage_used),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.data_read",
            reported_bytes(health.data_read_bytes),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.data_written",
            reported_bytes(health.data_written_bytes),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.power_on_hours",
            format!("{} h", health.power_on_hours),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.power_cycles",
            number(Some(health.power_cycles)),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.busy_minutes",
            format!("{} min", health.controller_busy_minutes),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.unsafe_shutdowns",
            number(Some(health.unsafe_shutdowns)),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.media_errors",
            number(Some(health.media_errors)),
        ),
        KeyValue::new(
            "module.storage_smart.nvme.error_log",
            number(Some(health.error_log_entries)),
        ),
    ];
    if health.warning_temp_minutes > 0 || health.critical_temp_minutes > 0 {
        rows.push(KeyValue::new(
            "module.storage_smart.nvme.temperature_time",
            format!(
                "{} / {} min",
                health.warning_temp_minutes, health.critical_temp_minutes
            ),
        ));
    }
    rows
}

/// Tabela atrybutów ATA. Nazwy atrybutów są kluczami i18n w komórkach, więc
/// tłumaczą się razem z interfejsem, a nieprogowe atrybuty producenta dostają
/// wspólną etykietę „atrybut producenta”.
fn attribute_table(attributes: &[AtaAttribute]) -> Table {
    let mut table = Table::new(
        vec![
            "module.storage_smart.ata.col.id".to_string(),
            "module.storage_smart.ata.col.name".to_string(),
            "module.storage_smart.ata.col.value".to_string(),
            "module.storage_smart.ata.col.worst".to_string(),
            "module.storage_smart.ata.col.threshold".to_string(),
            "module.storage_smart.ata.col.raw".to_string(),
        ],
        Vec::new(),
    );

    table.rows = attributes
        .iter()
        .map(|attribute| {
            vec![
                attribute.id.to_string(),
                String::new(),
                attribute.value.to_string(),
                attribute.worst.to_string(),
                if attribute.threshold == 0 {
                    "-".to_string()
                } else {
                    attribute.threshold.to_string()
                },
                attribute.raw.to_string(),
            ]
        })
        .collect();

    for (row, attribute) in attributes.iter().enumerate() {
        table = table.with_value_key(row, 1, attribute_name_key(attribute.id));
    }
    table
}

/// Klucz nazwy atrybutu: przetłumaczony, gdy znamy atrybut, inaczej wspólna
/// etykieta. Klucz musi istnieć w obu językach — pilnuje tego test modułu.
fn attribute_name_key(id: u8) -> String {
    if KNOWN_ATTRIBUTE_IDS.contains(&id) {
        format!("module.storage_smart.attr.{id}")
    } else {
        "module.storage_smart.ata.unknown".to_string()
    }
}

fn bus_label(bus: BusKind) -> &'static str {
    match bus {
        BusKind::Nvme => "NVMe",
        BusKind::Sata => "SATA",
        BusKind::Ata => "ATA",
        BusKind::Scsi => "SCSI/SAS",
        BusKind::Usb => "USB",
        BusKind::Raid => "RAID",
        BusKind::Unknown | BusKind::Other => "-",
    }
}

/// Wiersz „Magistrala”: znana magistrala to dana, nieznana to stan (klucz i18n).
fn bus_row(bus: BusKind) -> KeyValue {
    if bus == BusKind::Unknown {
        KeyValue::state("module.storage_smart.drive.bus", "common.value.unknown")
    } else {
        KeyValue::new("module.storage_smart.drive.bus", bus_label(bus))
    }
}

/// Czujniki temperatury z numerami — zero w czujniku to brak pomiaru, ale
/// numeracja musi zostać, żeby „Czujnik 2” nie udawał „Czujnika 1”.
fn numbered_sensors(sensors: &[Option<i16>]) -> String {
    let parts: Vec<String> = sensors
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.map(|value| format!("{}: {value} °C", index + 1)))
        .collect();
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(", ")
    }
}

/// Liczniki NVMe równe zero oznaczają „nie raportowano”, a nie zero bajtów.
fn reported_bytes(value: u128) -> String {
    if value == 0 {
        "-".to_string()
    } else {
        format_bytes(value.min(u128::from(u64::MAX)) as u64)
    }
}

fn kind_key(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Ssd => "common.value.ssd",
        DriveKind::Hdd => "common.value.hdd",
        DriveKind::Unknown => "common.value.unknown",
    }
}

fn health_key(health: Option<Health>) -> &'static str {
    match health {
        Some(Health::Good) => "module.storage_smart.health.good",
        Some(Health::Warning) => "module.storage_smart.health.warning",
        Some(Health::Bad) => "module.storage_smart.health.bad",
        None => "common.value.unknown",
    }
}

fn health_name(health: Option<Health>) -> Option<&'static str> {
    match health {
        Some(Health::Good) => Some("good"),
        Some(Health::Warning) => Some("warning"),
        Some(Health::Bad) => Some("bad"),
        None => None,
    }
}

/// Temperatura z atrybutów ATA: 194 (temperatura dysku) albo 190 (przepływ
/// powietrza). Surowe bajty niskie to stopnie Celsjusza (ze znakiem); zero
/// oznacza brak pomiaru, nie 0 °C.
fn ata_temperature(attributes: &[AtaAttribute]) -> Option<i16> {
    [194u8, 190]
        .iter()
        .find_map(|id| ata_raw(attributes, *id))
        .map(|raw| i16::from(raw as u8 as i8))
        .filter(|temperature| *temperature != 0 && (-20..=120).contains(temperature))
}

fn ata_raw(attributes: &[AtaAttribute], id: u8) -> Option<u64> {
    attributes
        .iter()
        .find(|attribute| attribute.id == id)
        .map(|attribute| attribute.raw)
}

fn drive_json(report: &DriveReport) -> serde_json::Value {
    let error = report.error.as_ref().map(|error| {
        serde_json::json!({
            "kind": if report.no_medium {
                "no_medium"
            } else {
                error_kind(error)
            },
            "message": error.to_string(),
        })
    });

    serde_json::json!({
        "index": report.index,
        "model": trimmed(Some(report.model.clone())),
        "serial": trimmed(Some(report.serial.clone())),
        "firmware": trimmed(Some(report.firmware.clone())),
        "size_bytes": report.size_bytes,
        "interface_type": trimmed(Some(report.interface_type.clone())),
        "bus": report.bus,
        "kind": report.kind,
        "removable": report.removable,
        "no_medium": report.no_medium,
        "protocol": report.protocol,
        "health": health_name(report.health),
        "temperature_c": report.temperature_c(),
        "power_on_hours": report.power_on_hours(),
        "nvme": report.nvme.as_ref().map(nvme_json),
        "ata": report.ata.as_ref().map(|attributes| {
            attributes.iter().map(attribute_json).collect::<Vec<_>>()
        }),
        "error": error,
    })
}

fn nvme_json(health: &NvmeHealth) -> serde_json::Value {
    serde_json::json!({
        "critical_warning": health.critical_warning,
        "warnings": health.warnings().iter().map(|warning| warning.key()).collect::<Vec<_>>(),
        "temperature_c": health.temperature_c,
        "sensors_c": health.sensors_c,
        "available_spare_percent": health.available_spare,
        "spare_threshold_percent": health.spare_threshold,
        "percentage_used": health.percentage_used,
        "data_read_bytes": health.data_read_bytes.min(u128::from(u64::MAX)) as u64,
        "data_written_bytes": health.data_written_bytes.min(u128::from(u64::MAX)) as u64,
        "power_on_hours": health.power_on_hours,
        "power_cycles": health.power_cycles,
        "controller_busy_minutes": health.controller_busy_minutes,
        "unsafe_shutdowns": health.unsafe_shutdowns,
        "media_errors": health.media_errors,
        "error_log_entries": health.error_log_entries,
        "warning_temp_minutes": health.warning_temp_minutes,
        "critical_temp_minutes": health.critical_temp_minutes,
    })
}

fn attribute_json(attribute: &AtaAttribute) -> serde_json::Value {
    serde_json::json!({
        "id": attribute.id,
        "flags": attribute.flags,
        "value": attribute.value,
        "worst": attribute.worst,
        "threshold": attribute.threshold,
        "raw": attribute.raw,
        "threshold_exceeded": attribute.threshold_exceeded(),
    })
}

fn error_kind(error: &ModuleError) -> &'static str {
    match error {
        ModuleError::Unavailable { .. } => "unavailable",
        ModuleError::Provider { .. } => "provider",
        ModuleError::AccessDenied { .. } => "access_denied",
        ModuleError::Internal(_) => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::providers::PhysicalDrive;
    use sk_core::testing::{Harness, MockStorage, MockWmi};

    fn disk_row(index: u32, model: &str) -> serde_json::Value {
        serde_json::json!({
            "Index": index,
            "Model": model,
            "SerialNumber": "SERIAL",
            "FirmwareRevision": "1.0",
            "Size": 512_105_932_800u64,
            "InterfaceType": "SCSI",
        })
    }

    /// Strona logu NVMe: 41 °C, 3 % zużycia, 2 000 000 jednostek zapisu.
    fn nvme_page() -> Vec<u8> {
        let mut page = vec![0u8; smart::SECTOR];
        page[1..3].copy_from_slice(&(273u16 + 41).to_le_bytes());
        page[3] = 100;
        page[4] = 10;
        page[5] = 3;
        page[48..64].copy_from_slice(&2_000_000u128.to_le_bytes());
        page[128..144].copy_from_slice(&1_234u128.to_le_bytes());
        page
    }

    /// Atrybuty ATA: 5 (przealokowane, 0), 9 (12 345 h), 194 (38 °C), 241.
    fn ata_sectors() -> (Vec<u8>, Vec<u8>) {
        let mut attributes = vec![0u8; smart::SECTOR];
        attributes[0] = 0x10;
        for (slot, (id, value, raw)) in [
            (5u8, 100u8, 0u64),
            (9, 99, 12_345),
            (194, 70, 38),
            (241, 99, 1_000_000),
        ]
        .iter()
        .enumerate()
        {
            let entry = 2 + slot * 12;
            attributes[entry] = *id;
            attributes[entry + 3] = *value;
            attributes[entry + 4] = *value;
            attributes[entry + 5..entry + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
        }
        let mut thresholds = vec![0u8; smart::SECTOR];
        thresholds[0] = 0x10;
        thresholds[2] = 5;
        thresholds[3] = 50;
        (attributes, thresholds)
    }

    fn harness() -> Harness {
        let wmi = MockWmi::new().with_rows(
            "Win32_DiskDrive",
            vec![
                disk_row(0, "WDC PC SN530 SDBPNPZ-512G"),
                disk_row(1, "CT1000MX500SSD1"),
            ],
        );
        let (attributes, thresholds) = ata_sectors();
        let storage = MockStorage::new()
            .with_nvme(
                PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd),
                nvme_page(),
            )
            .with_ata(
                PhysicalDrive::new(1, BusKind::Sata, DriveKind::Ssd),
                attributes,
                thresholds,
            );
        Harness::new()
            .with_wmi(wmi)
            .with_storage(storage)
            .with_host(sk_core::testing::MockHost::elevated())
    }

    fn table_at(view: &View, block_index: usize) -> &Table {
        view.sections[0]
            .blocks
            .get(block_index)
            .and_then(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela w widoku")
    }

    #[test]
    fn reads_nvme_health_and_ata_attributes() {
        let output = StorageSmart.collect(&harness().ctx()).expect("zbieranie");
        assert_eq!(output.data["drives_with_smart"], 2);

        let nvme = &output.data["drives"][0];
        assert_eq!(nvme["protocol"], "nvme");
        assert_eq!(nvme["health"], "good");
        assert_eq!(nvme["temperature_c"], 41);
        assert_eq!(nvme["power_on_hours"], 1_234);
        assert_eq!(nvme["nvme"]["percentage_used"], 3);
        assert_eq!(
            nvme["nvme"]["data_written_bytes"],
            2_000_000u64 * 1000 * 512
        );

        let ata = &output.data["drives"][1];
        assert_eq!(ata["protocol"], "ata");
        assert_eq!(ata["health"], "good");
        assert_eq!(ata["temperature_c"], 38);
        assert_eq!(ata["power_on_hours"], 12_345);
        assert_eq!(ata["ata"][1]["id"], 9);
        assert_eq!(ata["ata"][1]["raw"], 12_345);
    }

    #[test]
    fn overview_table_shows_state_as_translation_keys() {
        let output = StorageSmart.collect(&harness().ctx()).expect("zbieranie");
        let table = table_at(&output.view, 0);
        assert_eq!(table.rows[0][0], "WDC PC SN530 SDBPNPZ-512G");
        assert_eq!(table.rows[0][1], "NVMe");
        assert_eq!(table.rows[0][4], "41 °C");
        assert_eq!(table.value_key(0, 2), Some("common.value.ssd"));
        assert_eq!(
            table.value_key(0, 3),
            Some("module.storage_smart.health.good")
        );
        assert_eq!(
            table.value_key(1, 3),
            Some("module.storage_smart.health.good")
        );
        // Wartości w komórkach nie mogą być prozą.
        for row in &table.rows {
            assert_ne!(row[2], "SSD");
        }
    }

    #[test]
    fn per_drive_sections_carry_attribute_table() {
        let output = StorageSmart.collect(&harness().ctx()).expect("zbieranie");
        assert_eq!(output.view.sections.len(), 3, "przegląd + dwa dyski");

        let ata_section = &output.view.sections[2];
        assert_eq!(
            ata_section.subtitle.as_deref(),
            Some("CT1000MX500SSD1 · \\\\.\\PhysicalDrive1")
        );
        let table = ata_section
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela atrybutów");
        assert_eq!(table.rows.len(), 4);
        assert_eq!(table.rows[0][0], "5");
        assert_eq!(table.rows[0][2], "100");
        assert_eq!(table.rows[0][4], "50");
        assert_eq!(table.value_key(0, 1), Some("module.storage_smart.attr.5"));
        assert_eq!(table.value_key(2, 1), Some("module.storage_smart.attr.194"));
    }

    #[test]
    fn unknown_attribute_gets_shared_label() {
        // 236 nie ma nazwy w tablicy tłumaczeń — taki atrybut producenta
        // dostaje wspólną etykietę zamiast surowego klucza.
        let table = attribute_table(&[AtaAttribute {
            id: 236,
            flags: 0,
            value: 99,
            worst: 99,
            threshold: 0,
            raw: 0,
        }]);
        assert_eq!(
            table.value_key(0, 1),
            Some("module.storage_smart.ata.unknown")
        );
    }

    #[test]
    fn denied_drive_is_reported_as_missing_permission() {
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(0, "Dysk")]);
        let storage = MockStorage::new().with_denied(PhysicalDrive::new(
            0,
            BusKind::Unknown,
            DriveKind::Unknown,
        ));
        let harness = Harness::new()
            .with_wmi(wmi)
            .with_storage(storage)
            .with_host(sk_core::testing::MockHost::userland());

        let output = StorageSmart.collect(&harness.ctx()).expect("zbieranie");
        assert_eq!(output.data["elevated"], false);
        assert_eq!(output.data["drives"][0]["error"]["kind"], "access_denied");
        assert!(output.view.uses_key("module.storage_smart.note.elevation"));
        assert!(output.view.uses_key("module.storage_smart.note.denied"));
        assert_eq!(output.data["drives"][0]["health"], serde_json::Value::Null);
    }

    #[test]
    fn bridge_without_smart_is_an_info_not_an_error() {
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(2, "Czytnik USB")]);
        let storage = MockStorage::new().with_unavailable(
            PhysicalDrive::new(2, BusKind::Usb, DriveKind::Unknown).with_removable(true),
            "mostek USB nie przekazuje SMART",
        );
        let harness = Harness::new()
            .with_wmi(wmi)
            .with_storage(storage)
            .with_host(sk_core::testing::MockHost::elevated());

        let output = StorageSmart.collect(&harness.ctx()).expect("zbieranie");
        assert!(output.view.uses_key("module.storage_smart.note.bridge"));
        assert!(
            output
                .view
                .uses_key("module.storage_smart.note.unavailable")
        );
        assert_eq!(output.data["drives"][0]["error"]["kind"], "unavailable");
    }

    #[test]
    fn read_only_nvme_is_bad_and_shows_warning() {
        let mut page = nvme_page();
        page[0] = 0b0000_1000; // read only
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(0, "NVMe")]);
        let storage = MockStorage::new()
            .with_nvme(PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd), page);
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");
        assert_eq!(output.data["drives"][0]["health"], "bad");
        assert!(output.view.uses_key("module.storage_smart.warn.read_only"));
    }

    #[test]
    fn ata_threshold_hit_marks_drive_bad() {
        let (mut attributes, thresholds) = ata_sectors();
        // Atrybut 5: wartość 40 przy progu 50 → przekroczony.
        attributes[2 + 3] = 40;
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(1, "SSD")]);
        let storage = MockStorage::new().with_ata(
            PhysicalDrive::new(1, BusKind::Sata, DriveKind::Ssd),
            attributes,
            thresholds,
        );
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");

        assert_eq!(output.data["drives"][0]["health"], "bad");
        assert_eq!(
            output.data["drives"][0]["ata"][0]["threshold_exceeded"],
            true
        );
        assert!(
            output
                .view
                .uses_key("module.storage_smart.note.threshold_exceeded")
        );
    }

    #[test]
    fn drive_without_medium_is_skipped_without_probing() {
        let mut row = disk_row(3, "Czytnik kart");
        row["Size"] = serde_json::Value::Null;
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![row]);
        // Makieta zna ten dysk, więc gdyby moduł pytał o deskryptor, magistrala
        // byłaby znana (USB). Napęd bez nośnika nie może być odpytywany.
        let storage = MockStorage::new().with_unavailable(
            PhysicalDrive::new(3, BusKind::Usb, DriveKind::Unknown),
            "brak nośnika",
        );
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");

        assert_eq!(output.data["drives"][0]["error"]["kind"], "no_medium");
        assert_eq!(output.data["drives"][0]["no_medium"], true);
        assert_eq!(
            output.data["drives"][0]["bus"], "unknown",
            "deskryptor nie powinien być pytany dla pustego napędu"
        );
        assert!(output.view.uses_key("module.storage_smart.note.no_media"));
        assert!(
            !output
                .view
                .uses_key("module.storage_smart.note.unavailable")
        );
    }

    #[test]
    fn broken_nvme_page_is_reported_as_failure() {
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(0, "NVMe")]);
        let storage = MockStorage::new().with_nvme(
            PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd),
            vec![0u8; 64],
        );
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("uszkodzona strona logu nie może wywalić raportu");

        assert_eq!(output.data["drives"][0]["error"]["kind"], "provider");
        assert!(output.view.uses_key("module.storage_smart.note.failed"));
        assert_eq!(output.data["drives"][0]["health"], serde_json::Value::Null);
    }

    #[test]
    fn nvme_warning_bits_get_translated_notes() {
        let mut page = nvme_page();
        page[0] = 0b0011_0001; // zapas bloków, backup pamięci ulotnej, pamięć trwała
        page[192..196].copy_from_slice(&5u32.to_le_bytes());
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(0, "NVMe")]);
        let storage = MockStorage::new()
            .with_nvme(PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd), page);
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");

        assert_eq!(output.data["drives"][0]["health"], "warning");
        for key in [
            "module.storage_smart.health.warning",
            "module.storage_smart.warn.spare",
            "module.storage_smart.warn.volatile_backup",
            "module.storage_smart.warn.persistent_read_only",
            "module.storage_smart.nvme.temperature_time",
        ] {
            assert!(output.view.uses_key(key), "brak notki {key}");
        }
    }

    #[test]
    fn ata_counter_warning_explains_itself() {
        let (mut attributes, mut thresholds) = ata_sectors();
        // Atrybut 197 (sektory oczekujące) z surową wartością 3, próg nieprzekroczony.
        let entry = 2 + 4 * 12;
        attributes[entry] = 197;
        attributes[entry + 3] = 100;
        attributes[entry + 4] = 100;
        attributes[entry + 5..entry + 11].copy_from_slice(&3u64.to_le_bytes()[..6]);
        thresholds[entry] = 197;
        thresholds[entry + 1] = 1;

        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(1, "SSD")]);
        let storage = MockStorage::new().with_ata(
            PhysicalDrive::new(1, BusKind::Sata, DriveKind::Ssd),
            attributes,
            thresholds,
        );
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");

        assert_eq!(output.data["drives"][0]["health"], "warning");
        assert!(output.view.uses_key("module.storage_smart.note.counters"));
        assert!(output.view.uses_key("module.storage_smart.health.warning"));
    }

    #[test]
    fn nvme_zero_counters_show_as_missing() {
        let mut page = vec![0u8; smart::SECTOR];
        page[1..3].copy_from_slice(&314u16.to_le_bytes());
        page[3] = 100;
        page[4] = 10;
        page[128..144].copy_from_slice(&10u128.to_le_bytes());

        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![disk_row(0, "NVMe")]);
        let storage = MockStorage::new()
            .with_nvme(PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd), page);
        let output = StorageSmart
            .collect(
                &Harness::new()
                    .with_wmi(wmi)
                    .with_storage(storage)
                    .with_host(sk_core::testing::MockHost::elevated())
                    .ctx(),
            )
            .expect("zbieranie");

        let table = table_at(&output.view, 0);
        assert_eq!(
            table.rows[0][6], "-",
            "zero w liczniku NVMe to brak pomiaru, nie 0,0 GB"
        );
        let written = output
            .view
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .find_map(|block| match block {
                Block::KeyValues { rows } => rows
                    .iter()
                    .find(|row| row.label_key == "module.storage_smart.nvme.data_written"),
                _ => None,
            })
            .expect("wiersz zapisanych danych");
        assert_eq!(written.value, "-");
    }

    #[test]
    fn ata_temperature_accepts_negative_values() {
        let attributes = [AtaAttribute {
            id: 194,
            flags: 0,
            value: 70,
            worst: 70,
            threshold: 0,
            raw: 251, // 0xFB = -5 °C
        }];
        assert_eq!(ata_temperature(&attributes), Some(-5));

        let missing = [AtaAttribute {
            id: 194,
            flags: 0,
            value: 70,
            worst: 70,
            threshold: 0,
            raw: 0,
        }];
        assert_eq!(ata_temperature(&missing), None, "zero to brak pomiaru");
    }

    #[test]
    fn empty_disk_list_shows_note() {
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![]);
        let output = StorageSmart
            .collect(&Harness::new().with_wmi(wmi).ctx())
            .expect("pusta lista to nie błąd");
        assert!(output.view.uses_key("module.storage_smart.note.none"));
        // Kształt danych musi być ten sam co przy niepustej liście.
        assert_eq!(output.data["drives_with_smart"], 0);
        assert_eq!(output.data["elevated"], false);
        assert!(output.data["drives"].as_array().is_some_and(Vec::is_empty));
    }

    #[test]
    fn every_attribute_name_key_exists_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        for id in KNOWN_ATTRIBUTE_IDS {
            let key = attribute_name_key(*id);
            for locale in Locale::ALL {
                assert!(
                    translator.has(locale, &key),
                    "brak klucza {key} w języku {}",
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = StorageSmart.collect(&harness().ctx()).expect("zbieranie");
        let translator = Translator::load().expect("tłumaczenia");
        for key in output.view.translation_keys() {
            for locale in Locale::ALL {
                assert!(
                    translator.has(locale, &key),
                    "brak klucza {key} w języku {}",
                    locale.code()
                );
            }
        }
    }
}
