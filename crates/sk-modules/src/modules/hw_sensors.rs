//! `hw.sensors` — czujniki jak w HWiNFO: temperatury, wentylatory, napięcia,
//! moc, taktowanie i obciążenie.
//!
//! Moduł jest bezstanowy: odczyty GPU/ACPI/baterii dostaje z [`SensorsSource`],
//! historię do minimum/maksimum/średniej również. Dyski czyta przez
//! `StorageProbe` (SMART), bo tylko tam jest temperatura nośnika — WMI jej nie
//! podaje. Brak sterownika albo uprawnień to notka, nie błąd całego modułu.
//!
//! Czego nie ma i dlaczego: temperatura rdzeni CPU wymaga odczytu MSR/SMN
//! w ring 0, więc dojdzie dopiero z helperem LHM + PawnIO (Faza 5).

use std::collections::BTreeMap;

use serde::Deserialize;
use sk_core::prelude::*;

use crate::smart::{self, ata_temperature};
use crate::util::{decode_all, trimmed};

/// Ten sam zestaw pól co `hw.disk_inventory` i `storage.smart` — cache WMI
/// obsługuje wszystkie trzy moduły jednym zapytaniem.
const DISK_QUERY: &str = queries::WIN32_DISK_DRIVE;

/// Ile próbek pokazujemy na wykresie (przy 1 s = 5 minut).
const MAX_CHART_SAMPLES: usize = 300;
/// Więcej serii na jednym wykresie tylko pogarsza czytelność.
const MAX_CHART_SERIES: usize = 4;

#[derive(Debug, Default, Deserialize)]
struct DiskDrive {
    #[serde(rename = "Index", default)]
    index: Option<u32>,
    #[serde(rename = "Model", default)]
    model: Option<String>,
}

/// Co udało się odczytać z dysków — razem z powodem braków.
#[derive(Debug, Default)]
struct DriveScan {
    readings: Vec<SensorReading>,
    reports: Vec<serde_json::Value>,
    denied: bool,
    unavailable: bool,
}

#[derive(Debug, Default)]
struct ReadingStats {
    min: f64,
    max: f64,
    sum: f64,
    count: u32,
}

impl ReadingStats {
    fn add(&mut self, value: f64) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
        self.sum += value;
        self.count += 1;
    }

    fn average(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / f64::from(self.count))
    }
}

#[derive(Debug, Default)]
pub struct Sensors;

impl Module for Sensors {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.sensors",
            name_key: "module.hw_sensors.name",
            desc_key: "module.hw_sensors.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let mut readings = ctx.sensors.readings();
        let drives = scan_drives(ctx);
        readings.extend(drives.readings.clone());
        deduplicate(&mut readings);
        readings.sort_by(reading_order);

        let history = ctx.sensors.history();
        let stats = collect_stats(&history);
        let samples = history.len();

        let mut blocks = Vec::new();
        if readings.is_empty() {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_sensors.note.no_sources",
            ));
        } else {
            blocks.push(Block::Table {
                table: readings_table(&readings, &stats),
            });
        }

        if let Some(chart) = temperature_chart(&history) {
            blocks.push(Block::chart(chart));
        }

        if samples == 1 {
            blocks.push(Block::note(
                NoteLevel::Info,
                "module.hw_sensors.note.history",
            ));
        }
        if !has_kind(&readings, DeviceKind::Cpu, SensorKind::Temperature) {
            blocks.push(Block::note(
                NoteLevel::Info,
                "module.hw_sensors.note.cpu_temperature",
            ));
        }
        if drives.denied {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_sensors.note.disk_access",
            ));
        }
        if drives.unavailable {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_sensors.note.disk_unavailable",
            ));
        }
        // Notka o GPU tylko wtedy, gdy inne źródła działają — inaczej zdanie
        // „brak czujników” już to mówi.
        if !readings.is_empty() && !has_kind(&readings, DeviceKind::Gpu, SensorKind::Temperature) {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_sensors.note.gpu_unavailable",
            ));
        }

        let view = View::new(vec![
            Section::new("module.hw_sensors.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "sample_count": samples,
            "sensor_count": readings.len(),
            "readings": readings.iter().map(reading_json).collect::<Vec<_>>(),
            "drives": drives.reports,
            "stats": stats_json(&readings, &stats),
        });

        Ok(ModuleOutput { data, view })
    }
}

/// Czujniki dysków: NVMe z logu zdrowia, ATA z atrybutu 194/190.
fn scan_drives(ctx: &CollectCtx<'_>) -> DriveScan {
    let mut scan = DriveScan::default();
    let disks: Vec<DiskDrive> = match ctx.wmi.query(NS_CIMV2, DISK_QUERY) {
        Ok(rows) => decode_all(&rows, "Win32_DiskDrive").unwrap_or_default(),
        // Bez listy dysków nie ma czego odpytać; reszta czujników działa dalej.
        Err(_) => return scan,
    };

    for disk in disks {
        let index = disk.index.unwrap_or(0);
        let model = trimmed(disk.model.clone()).unwrap_or_else(|| format!("PhysicalDrive{index}"));
        let mut report = serde_json::json!({
            "index": index,
            "model": model,
            "temperature_c": serde_json::Value::Null,
            "error": serde_json::Value::Null,
        });

        match ctx.storage.smart(index) {
            Ok(SmartData::Nvme { log_page }) => match smart::parse_nvme_health(&log_page) {
                Ok(health) => {
                    if let Some(temperature) = health.temperature_c {
                        report["temperature_c"] = serde_json::json!(temperature);
                        scan.readings.push(SensorReading::new(
                            model.clone(),
                            DeviceKind::Drive,
                            SensorKind::Temperature,
                            f64::from(temperature),
                        ));
                    }
                    // Dodatkowe czujniki NVMe (1–8) pokazujemy tylko wtedy, gdy
                    // różnią się od temperatury złożonej — inaczej to duplikat.
                    for (channel, value) in health.sensors_c.iter().enumerate() {
                        let Some(value) = value else { continue };
                        if *value <= 0 || Some(*value) == health.temperature_c {
                            continue;
                        }
                        let Ok(channel) = u8::try_from(channel + 1) else {
                            continue;
                        };
                        scan.readings.push(
                            SensorReading::new(
                                model.clone(),
                                DeviceKind::Drive,
                                SensorKind::Temperature,
                                f64::from(*value),
                            )
                            .with_channel(channel),
                        );
                    }
                }
                Err(error) => {
                    report["error"] = serde_json::json!(error.to_string());
                }
            },
            Ok(SmartData::Ata {
                attributes,
                thresholds,
            }) => match smart::parse_ata_attributes(&attributes, &thresholds) {
                Ok(attributes) => {
                    if let Some(temperature) = ata_temperature(&attributes) {
                        report["temperature_c"] = serde_json::json!(temperature);
                        scan.readings.push(SensorReading::new(
                            model.clone(),
                            DeviceKind::Drive,
                            SensorKind::Temperature,
                            f64::from(temperature),
                        ));
                    }
                }
                Err(error) => {
                    report["error"] = serde_json::json!(error.to_string());
                }
            },
            Err(ModuleError::AccessDenied { .. }) => {
                scan.denied = true;
                report["error"] = serde_json::json!("access_denied");
            }
            Err(error) => {
                scan.unavailable = true;
                report["error"] = serde_json::json!(error.to_string());
            }
        }
        scan.reports.push(report);
    }
    scan
}

fn reading_order(first: &SensorReading, second: &SensorReading) -> std::cmp::Ordering {
    first
        .device_kind
        .rank()
        .cmp(&second.device_kind.rank())
        .then_with(|| {
            first
                .device
                .to_lowercase()
                .cmp(&second.device.to_lowercase())
        })
        .then_with(|| first.kind.cmp(&second.kind))
        .then_with(|| first.channel.cmp(&second.channel))
}

/// ADL i NVML mogą opisać tę samą kartę (np. po zmianie sterownika w trakcie
/// działania aplikacji); zostawiamy pierwszy odczyt danego czujnika.
fn deduplicate(readings: &mut Vec<SensorReading>) {
    let mut seen = std::collections::BTreeSet::new();
    readings.retain(|reading| seen.insert(sensor_key(reading)));
}

fn sensor_key(reading: &SensorReading) -> String {
    format!(
        "{}|{}|{:?}|{:?}",
        reading.device_kind.rank(),
        reading.device,
        reading.kind,
        reading.channel
    )
}

fn collect_stats(history: &[SensorSample]) -> BTreeMap<String, ReadingStats> {
    let mut stats: BTreeMap<String, ReadingStats> = BTreeMap::new();
    for sample in history {
        for reading in &sample.readings {
            if reading.value.is_finite() {
                stats
                    .entry(sensor_key(reading))
                    .or_default()
                    .add(reading.value);
            }
        }
    }
    stats
}

fn has_kind(readings: &[SensorReading], device: DeviceKind, kind: SensorKind) -> bool {
    readings
        .iter()
        .any(|reading| reading.device_kind == device && reading.kind == kind)
}

fn sensor_label(kind: SensorKind) -> &'static str {
    match kind {
        SensorKind::Temperature => "module.hw_sensors.sensor.temperature",
        SensorKind::Hotspot => "module.hw_sensors.sensor.hotspot",
        SensorKind::MemoryTemp => "module.hw_sensors.sensor.memory_temp",
        SensorKind::FanRpm => "module.hw_sensors.sensor.fan_rpm",
        SensorKind::FanPercent => "module.hw_sensors.sensor.fan_percent",
        SensorKind::Voltage => "module.hw_sensors.sensor.voltage",
        SensorKind::Power => "module.hw_sensors.sensor.power",
        SensorKind::Load => "module.hw_sensors.sensor.load",
        SensorKind::CoreClock => "module.hw_sensors.sensor.core_clock",
        SensorKind::MemoryClock => "module.hw_sensors.sensor.memory_clock",
    }
}

fn device_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Cpu => "module.hw_sensors.device.cpu",
        DeviceKind::Gpu => "module.hw_sensors.device.gpu",
        DeviceKind::Board => "module.hw_sensors.device.board",
        DeviceKind::Drive => "module.hw_sensors.device.drive",
        DeviceKind::Battery => "module.hw_sensors.device.battery",
    }
}

/// Nazwa urządzenia w tabeli. Kanał dopisujemy jako daną (`#3`), bo to numer
/// czujnika, a nie tekst do tłumaczenia.
fn device_cell(reading: &SensorReading) -> String {
    match reading.channel {
        Some(channel) if !reading.device.is_empty() => {
            format!("{} #{channel}", reading.device)
        }
        _ => reading.device.clone(),
    }
}

fn readings_table(readings: &[SensorReading], stats: &BTreeMap<String, ReadingStats>) -> Table {
    let headers = [
        "module.hw_sensors.col.device",
        "module.hw_sensors.col.sensor",
        "module.hw_sensors.col.value",
        "module.hw_sensors.col.min",
        "module.hw_sensors.col.max",
        "module.hw_sensors.col.avg",
    ]
    .map(str::to_string)
    .to_vec();

    let mut rows = Vec::with_capacity(readings.len());
    let mut value_keys = Vec::with_capacity(readings.len());
    for reading in readings {
        let unit = reading.unit();
        let value = format_reading(reading.value, unit);
        let reading_stats = stats.get(&sensor_key(reading));
        let bound = |value: Option<f64>| {
            value.map_or_else(|| "-".to_string(), |value| format_reading(value, unit))
        };
        rows.push(vec![
            device_cell(reading),
            String::new(),
            value,
            bound(reading_stats.map(|stats| stats.min)),
            bound(reading_stats.map(|stats| stats.max)),
            bound(reading_stats.and_then(ReadingStats::average)),
        ]);

        let mut keys = vec![
            None,
            Some(sensor_label(reading.kind).to_string()),
            None,
            None,
            None,
            None,
        ];
        if reading.device.is_empty() {
            keys[0] = Some(device_label(reading.device_kind).to_string());
        }
        value_keys.push(keys);
    }

    let mut table = Table::new(headers, rows);
    for (row, keys) in value_keys.into_iter().enumerate() {
        for (column, key) in keys.into_iter().enumerate() {
            if let Some(key) = key {
                table = table.with_value_key(row, column, key);
            }
        }
    }
    table
}

/// Wykres temperatur: jedna seria na urządzenie. Bierzemy tylko główną
/// temperaturę (`SensorKind::Temperature`), bo hotspot ma inną skalę i myliłby
/// odczyt „ile stopni ma karta”.
fn temperature_chart(history: &[SensorSample]) -> Option<Chart> {
    let window = &history[history.len().saturating_sub(MAX_CHART_SAMPLES)..];
    if window.len() < 2 {
        return None;
    }

    let mut series: BTreeMap<String, Vec<[f64; 2]>> = BTreeMap::new();
    for sample in window {
        for reading in &sample.readings {
            if reading.device_kind == DeviceKind::Drive
                || reading.kind != SensorKind::Temperature
                || reading.channel.is_some()
                || !reading.value.is_finite()
            {
                continue;
            }
            let label = if reading.device.is_empty() {
                continue;
            } else {
                reading.device.clone()
            };
            series
                .entry(label)
                .or_default()
                .push([sample.elapsed_secs, reading.value]);
        }
    }

    let series: Vec<ChartSeries> = series
        .into_iter()
        .filter(|(_, points)| points.len() >= 2)
        .take(MAX_CHART_SERIES)
        .map(|(label, points)| ChartSeries::new(label, points))
        .collect();
    if series.is_empty() {
        return None;
    }

    let chart = Chart::new("module.hw_sensors.chart.temperature", "s", "°C").with_series(series);
    // Bez `x_min`/`x_max`: `default_x_bounds` w egui_plot zatrzymuje automatyczną
    // skalę na pierwszej klatce, więc rosnąca historia ucinałaby wykres. Oś ma
    // podążać za próbkami, jak w `system.live`.
    Some(chart)
}

fn reading_json(reading: &SensorReading) -> serde_json::Value {
    serde_json::json!({
        "device": reading.device,
        "device_kind": reading.device_kind,
        "kind": reading.kind,
        "channel": reading.channel,
        "value": reading.value,
        "unit": reading.unit().symbol(),
    })
}

fn stats_json(
    readings: &[SensorReading],
    stats: &BTreeMap<String, ReadingStats>,
) -> Vec<serde_json::Value> {
    readings
        .iter()
        .filter_map(|reading| {
            let stats = stats.get(&sensor_key(reading))?;
            Some(serde_json::json!({
                "device": reading.device,
                "kind": reading.kind,
                "channel": reading.channel,
                "min": stats.min,
                "max": stats.max,
                "average": stats.average(),
                "samples": stats.count,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockSensors, MockStorage, MockWmi};

    fn gpu(device: &str, kind: SensorKind, value: f64) -> SensorReading {
        SensorReading::new(device, DeviceKind::Gpu, kind, value)
    }

    fn samples() -> Vec<SensorSample> {
        vec![
            SensorSample::new(
                0.0,
                vec![
                    gpu("AMD Radeon RX 6700 XT", SensorKind::Temperature, 50.0),
                    gpu("AMD Radeon RX 6700 XT", SensorKind::FanRpm, 600.0),
                ],
            ),
            SensorSample::new(
                1.0,
                vec![
                    gpu("AMD Radeon RX 6700 XT", SensorKind::Temperature, 60.0),
                    gpu("AMD Radeon RX 6700 XT", SensorKind::FanRpm, 700.0),
                ],
            ),
            SensorSample::new(
                2.0,
                vec![
                    gpu("AMD Radeon RX 6700 XT", SensorKind::Temperature, 70.0),
                    gpu("AMD Radeon RX 6700 XT", SensorKind::FanRpm, 800.0),
                ],
            ),
        ]
    }

    /// Strona logu zdrowia NVMe: temperatura złożona w K (HWiNFO podaje °C).
    fn nvme_page(composite_kelvin: u16, sensors_kelvin: &[u16]) -> Vec<u8> {
        let mut page = vec![0u8; 512];
        page[1..3].copy_from_slice(&composite_kelvin.to_le_bytes());
        for (index, kelvin) in sensors_kelvin.iter().enumerate() {
            let offset = 200 + index * 2;
            page[offset..offset + 2].copy_from_slice(&kelvin.to_le_bytes());
        }
        page
    }

    /// Sektor atrybutów ATA z jednym wpisem: ID, wartość, raw (6 B).
    fn ata_sector(id: u8, value: u8, raw: [u8; 6]) -> Vec<u8> {
        let mut sector = vec![0u8; 512];
        let entry = 2;
        sector[entry] = id;
        sector[entry + 3] = value;
        sector[entry + 4] = value;
        sector[entry + 5..entry + 11].copy_from_slice(&raw);
        sector
    }

    fn disk_wmi(index: u32, model: &str) -> MockWmi {
        MockWmi::new().with_rows(
            "Win32_DiskDrive",
            vec![serde_json::json!({ "Index": index, "Model": model })],
        )
    }

    #[test]
    fn empty_sources_show_notes_instead_of_failing() {
        // WMI bez dysków, brak czujników GPU/ACPI — typowa maszyna bez admina.
        let harness = Harness::new();
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_sensors.note.no_sources"));
        assert!(
            output
                .view
                .uses_key("module.hw_sensors.note.cpu_temperature")
        );
        assert_eq!(output.data["sensor_count"], 0);
    }

    #[test]
    fn gpu_readings_get_min_max_average_from_history() {
        let harness = Harness::new().with_sensors(MockSensors::new().with_samples(samples()));
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");

        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela czujników");
        let temperature = table
            .rows
            .iter()
            .find(|row| row[0].contains("RX 6700"))
            .expect("wiersz GPU");
        assert_eq!(temperature[2], "70.0 °C");
        assert_eq!(temperature[3], "50.0 °C");
        assert_eq!(temperature[4], "70.0 °C");
        assert_eq!(temperature[5], "60.0 °C");
        assert_eq!(
            table.value_key(0, 1),
            Some("module.hw_sensors.sensor.temperature")
        );

        assert!(output.view.uses_key("module.hw_sensors.chart.temperature"));
        assert_eq!(output.data["sample_count"], 3);
    }

    #[test]
    fn single_sample_has_no_statistics_and_says_so() {
        let harness = Harness::new().with_sensors(MockSensors::new().with_readings(vec![gpu(
            "AMD Radeon",
            SensorKind::Temperature,
            57.0,
        )]));
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_sensors.note.history"));
        assert!(!output.view.uses_key("module.hw_sensors.chart.temperature"));
    }

    #[test]
    fn nvme_drive_temperature_comes_from_health_log() {
        let page = nvme_page(310, &[310, 320]);
        let storage = MockStorage::new()
            .with_nvme(PhysicalDrive::new(0, BusKind::Nvme, DriveKind::Ssd), page);
        let harness = Harness::new()
            .with_wmi(disk_wmi(0, "Samsung SSD 970"))
            .with_storage(storage);
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");

        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela czujników");
        assert!(
            table
                .rows
                .iter()
                .any(|row| row[0] == "Samsung SSD 970" && row[2] == "37.0 °C")
        );
        // Drugi czujnik NVMe różni się od złożonego — dostaje numer kanału.
        assert!(
            table
                .rows
                .iter()
                .any(|row| row[0] == "Samsung SSD 970 #2" && row[2] == "47.0 °C")
        );
        assert_eq!(output.data["drives"][0]["temperature_c"], 37);
        assert!(!output.view.uses_key("module.hw_sensors.note.disk_access"));
    }

    #[test]
    fn ata_drive_temperature_uses_attribute_194() {
        let attributes = ata_sector(194, 100, [41, 0, 0, 0, 0, 0]);
        let thresholds = vec![0u8; 512];
        let storage = MockStorage::new().with_ata(
            PhysicalDrive::new(1, BusKind::Sata, DriveKind::Ssd),
            attributes,
            thresholds,
        );
        let harness = Harness::new()
            .with_wmi(disk_wmi(1, "CT1000MX500SSD1"))
            .with_storage(storage);
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_sensors.sensor.temperature"));
        assert_eq!(output.data["drives"][0]["temperature_c"], 41);
    }

    #[test]
    fn access_denied_for_drives_is_a_note_not_an_error() {
        let storage = MockStorage::new().with_denied(PhysicalDrive::new(
            0,
            BusKind::Unknown,
            DriveKind::Unknown,
        ));
        let harness = Harness::new()
            .with_wmi(disk_wmi(0, "Dysk systemowy"))
            .with_storage(storage);
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_sensors.note.disk_access"));
        assert_eq!(output.data["drives"][0]["error"], "access_denied");
    }

    #[test]
    fn channel_is_shown_as_data() {
        let reading = gpu("AMD Radeon", SensorKind::Temperature, 55.0).with_channel(3);
        assert_eq!(device_cell(&reading), "AMD Radeon #3");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        let harness = Harness::new().with_sensors(MockSensors::new().with_samples(samples()));
        let output = Sensors.collect(&harness.ctx()).expect("zbieranie danych");
        for key in output.view.translation_keys() {
            for locale in Locale::ALL {
                assert!(
                    translator.has(locale, &key),
                    "brak klucza {key} w języku {}",
                    locale.code()
                );
            }
        }
        for meta_key in [
            "module.hw_sensors.name",
            "module.hw_sensors.desc",
            "module.hw_sensors.section",
        ] {
            for locale in Locale::ALL {
                assert!(translator.has(locale, meta_key), "brak {meta_key}");
            }
        }
    }
}
