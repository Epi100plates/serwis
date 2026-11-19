//! `storage.sectors` — skan nieczytelnych sektorów (Error Scan jak w HD Tune).
//!
//! Skanu nie wykonuje `collect()`: pełny odczyt dysku trwa godziny, więc
//! uruchamia go wątek aplikacji, a moduł tylko czyta ostatni stan przez
//! [`SectorSource`] — ten sam wzorzec co `storage.bench`. Dzięki temu widok
//! jest natychmiastowy i testowalny bez dysku i bez administratora.
//!
//! Moduł pokazuje trzy rzeczy: listę dysków w bezczynności, mapę powierzchni
//! (zielone i czerwone komórki) oraz listę nieczytelnych zakresów po skanie.
//! Sam przebieg — plan, postęp i mapa — liczy `sk_core::sector`; tutaj jest
//! wyłącznie formatowanie wyniku.

use serde::Deserialize;
use sk_core::prelude::*;
use sk_core::sector::SECTOR_UNIT_BYTES;

use crate::bench;
use crate::util::{decode_all, format_bytes, format_size, number, text, trimmed};

const DISK_QUERY: &str = queries::WIN32_DISK_DRIVE;

/// Ile wierszy nieczytelnych zakresów pokazujemy w tabeli. Pełna lista jest
/// w danych JSON (ograniczona do `MAX_SECTOR_ERRORS` przez warstwę Windows),
/// a położenie wszystkich błędów i tak widać na mapie powierzchni.
const MAX_ERROR_ROWS: usize = 20;

#[derive(Debug, Default, Deserialize)]
struct DiskDrive {
    #[serde(rename = "Index", default)]
    index: Option<u32>,
    #[serde(rename = "Model", default)]
    model: Option<String>,
    #[serde(rename = "InterfaceType", default)]
    interface_type: Option<String>,
    #[serde(rename = "Size", default)]
    size_bytes: Option<u64>,
}

#[derive(Debug, Default)]
pub struct StorageSectors;

impl Module for StorageSectors {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "storage.sectors",
            name_key: "module.storage_sectors.name",
            desc_key: "module.storage_sectors.desc",
            group: Group::Storage,
            capability: Capability::Elevated,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let (drives, wmi_error) = load_drives(ctx);
        let state = ctx.sectors.state();
        let elevated = ctx.host.is_elevated();
        let map = state.map();

        let (blocks, data) = match &state {
            SectorState::Idle => idle_view(&drives, elevated),
            SectorState::Running {
                params,
                steps_done,
                steps_total,
                bytes_read,
                elapsed_ms,
                errors,
                ..
            } => running_view(
                &drives,
                params,
                Progress {
                    steps_done: *steps_done,
                    steps_total: *steps_total,
                    bytes_read: *bytes_read,
                    elapsed_ms: *elapsed_ms,
                    errors,
                },
                map,
            ),
            SectorState::Done(scan) => done_view(&drives, scan, map),
            SectorState::Failed { params, message } => {
                failed_view(&drives, params.as_ref(), message, elevated)
            }
        };

        let mut blocks = blocks;
        if wmi_error.is_some() {
            // Lista dysków to dana dodatkowa: brak WMI nie może ukryć wyniku
            // skanu, ale technik ma wiedzieć, dlaczego model jest nieznany.
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.storage_sectors.note.wmi",
            ));
        }

        let view = View::new(vec![
            Section::new("module.storage_sectors.section").with_blocks(blocks),
        ]);

        let data = with_common_data(data, &state, elevated, &drives);
        Ok(ModuleOutput { data, view })
    }
}

/// Lista dysków z WMI. Błąd nie przerywa modułu — wracamy z pustą listą
/// i notką, bo wynik skanu jest ważniejszy niż nazwa modelu.
fn load_drives(ctx: &CollectCtx<'_>) -> (Vec<DiskDrive>, Option<String>) {
    match ctx.wmi.query(NS_CIMV2, DISK_QUERY) {
        Ok(rows) => match decode_all::<DiskDrive>(&rows, "Win32_DiskDrive") {
            Ok(drives) => (drives, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        },
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}

fn drive_for(drives: &[DiskDrive], index: u32) -> Option<&DiskDrive> {
    drives
        .iter()
        .find(|drive| drive.index == Some(index))
        .or_else(|| drives.first())
}

fn drive_label(drives: &[DiskDrive], index: u32) -> String {
    drive_for(drives, index)
        .and_then(|drive| drive.model.as_deref())
        .map_or_else(
            || format!("PhysicalDrive{index}"),
            |model| model.trim().to_string(),
        )
}

fn drive_rows(drives: &[DiskDrive]) -> Vec<Vec<String>> {
    drives
        .iter()
        .map(|drive| {
            vec![
                number(drive.index),
                text(drive.model.as_deref()),
                drive
                    .size_bytes
                    .map_or_else(|| "-".to_string(), format_bytes),
                text(drive.interface_type.as_deref()),
            ]
        })
        .collect()
}

/// Stan bezczynny: lista dysków do wyboru i ograniczenia. Instrukcji „co
/// uruchomić” nie ma — mówi ją karta skanu w panelu GUI.
fn idle_view(drives: &[DiskDrive], elevated: bool) -> (Vec<Block>, serde_json::Value) {
    let mut blocks = Vec::new();

    if drives.is_empty() {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_sectors.note.no_drives",
        ));
    } else {
        blocks.push(Block::Table {
            table: Table::new(
                vec![
                    "module.storage_sectors.col.index".to_string(),
                    "module.storage_sectors.col.model".to_string(),
                    "module.storage_sectors.col.size".to_string(),
                    "module.storage_sectors.col.interface".to_string(),
                ],
                drive_rows(drives),
            ),
        });
    }

    if !elevated {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_sectors.note.elevation",
        ));
    }

    (blocks, serde_json::json!({ "state": "idle" }))
}

/// Migawka stanu `Running` potrzebna widokowi — jeden argument zamiast sześciu.
struct Progress<'a> {
    steps_done: u64,
    steps_total: u64,
    bytes_read: u64,
    elapsed_ms: u64,
    errors: &'a [SectorError],
}

/// Skan w toku: postęp, mapa rosnąca od pierwszej komórki i szacowany koniec.
///
/// Skan nie ma limitu czasu, więc postęp liczą odczytane jednostki, a nie
/// zegar — pasek dojdzie do 100 % dopiero z końcem zakresu.
fn running_view(
    drives: &[DiskDrive],
    params: &SectorParams,
    progress: Progress<'_>,
    map: Option<SurfaceMap>,
) -> (Vec<Block>, serde_json::Value) {
    let Progress {
        steps_done,
        steps_total,
        bytes_read,
        elapsed_ms,
        errors,
    } = progress;

    let mut blocks = vec![Block::key_values(vec![
        KeyValue::new(
            "module.storage_sectors.row.drive",
            drive_label(drives, params.drive),
        ),
        KeyValue::state("module.storage_sectors.row.mode", mode_key(params.mode)),
        KeyValue::new(
            "module.storage_sectors.row.unit",
            format_size(SECTOR_UNIT_BYTES),
        ),
        KeyValue::new(
            "module.storage_sectors.row.read",
            format!(
                "{} / {}",
                format_size(bytes_read),
                format_size(params.span_bytes)
            ),
        ),
        KeyValue::new(
            "module.storage_sectors.row.duration",
            bench::format_duration(elapsed_ms),
        ),
        KeyValue::new(
            "module.storage_sectors.row.speed",
            format_speed(read_speed_mbps(bytes_read, elapsed_ms)),
        ),
        KeyValue::new(
            "module.storage_sectors.row.remaining",
            eta_ms(steps_done, steps_total, elapsed_ms)
                .map_or_else(|| "-".to_string(), bench::format_duration),
        ),
        KeyValue::new(
            "module.storage_sectors.row.errors",
            errors.len().to_string(),
        ),
    ])];

    if let Some(map) = map {
        blocks.push(Block::map(map));
    }

    let data = serde_json::json!({
        "state": "running",
        "steps_done": steps_done,
        "steps_total": steps_total,
        "bytes_read": bytes_read,
        "elapsed_ms": elapsed_ms,
        "errors": errors,
    });
    (blocks, data)
}

/// Zakończony (albo przerwany) skan: ocena, statystyki, mapa i nieczytelne
/// zakresy. Baner i tabela mówią wszystko — notki są tylko o przerwaniu
/// i o uciętej liście.
fn done_view(
    drives: &[DiskDrive],
    scan: &SectorScan,
    map: Option<SurfaceMap>,
) -> (Vec<Block>, serde_json::Value) {
    let (status_key, level) = if scan.errors.is_empty() {
        ("module.storage_sectors.status.clean", StatusLevel::Ok)
    } else {
        ("module.storage_sectors.status.damaged", StatusLevel::Bad)
    };

    let mut blocks = vec![Block::status(
        "module.storage_sectors.status.label",
        status_key,
        level,
    )];

    blocks.push(Block::key_values(vec![
        KeyValue::new(
            "module.storage_sectors.row.drive",
            drive_label(drives, scan.params.drive),
        ),
        KeyValue::state(
            "module.storage_sectors.row.mode",
            mode_key(scan.params.mode),
        ),
        KeyValue::new(
            "module.storage_sectors.row.unit",
            format_size(SECTOR_UNIT_BYTES),
        ),
        KeyValue::new(
            "module.storage_sectors.row.read",
            format!(
                "{} / {}",
                format_size(scan.bytes_read),
                format_size(scan.params.span_bytes)
            ),
        ),
        KeyValue::new(
            "module.storage_sectors.row.duration",
            bench::format_duration(scan.duration_ms),
        ),
        KeyValue::new(
            "module.storage_sectors.row.speed",
            format_speed(Some(scan.average_mbps())),
        ),
        KeyValue::new(
            "module.storage_sectors.row.errors",
            scan.errors.len().to_string(),
        ),
        KeyValue::new(
            "module.storage_sectors.row.damaged",
            format_size(scan.damaged_bytes()),
        ),
    ]));

    if scan.cancelled {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_sectors.note.cancelled",
        ));
    }
    if scan.errors_truncated {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_sectors.note.truncated",
        ));
    }

    if let Some(map) = map {
        blocks.push(Block::map(map));
    }

    if !scan.errors.is_empty() {
        let rows: Vec<Vec<String>> = scan
            .errors
            .iter()
            .take(MAX_ERROR_ROWS)
            .map(|error| {
                vec![
                    format_position_mb(error.offset_bytes),
                    format_size(error.length_bytes),
                    error.detail.clone(),
                ]
            })
            .collect();
        blocks.push(Block::Table {
            table: Table::new(
                vec![
                    "module.storage_sectors.col.position".to_string(),
                    "module.storage_sectors.col.length".to_string(),
                    "module.storage_sectors.col.error".to_string(),
                ],
                rows,
            ),
        });
    }

    let data = serde_json::json!({
        "state": "done",
        "cancelled": scan.cancelled,
        "steps_done": scan.steps_done,
        "steps_total": scan.steps_total,
        "bytes_read": scan.bytes_read,
        "duration_ms": scan.duration_ms,
        "errors": scan.errors,
        "damaged_bytes": scan.damaged_bytes(),
        "errors_truncated": scan.errors_truncated,
    });
    (blocks, data)
}

/// Skan, który się nie zaczął (brak uprawnień, brak urządzenia). To nie błąd
/// modułu: technik dostaje komunikat urządzenia i notkę o uprawnieniach.
fn failed_view(
    drives: &[DiskDrive],
    params: Option<&SectorParams>,
    message: &str,
    elevated: bool,
) -> (Vec<Block>, serde_json::Value) {
    let mut rows = Vec::new();
    if let Some(params) = params {
        rows.push(KeyValue::new(
            "module.storage_sectors.row.drive",
            drive_label(drives, params.drive),
        ));
    }
    rows.push(KeyValue::new("module.storage_sectors.row.error", message));

    let mut blocks = vec![
        Block::note(NoteLevel::Error, "module.storage_sectors.note.failed"),
        Block::key_values(rows),
    ];
    if !elevated {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_sectors.note.elevation",
        ));
    }

    let data = serde_json::json!({
        "state": "failed",
        "error": message,
    });
    (blocks, data)
}

/// Wspólne pola danych: metadane raportu, lista dysków i parametry przebiegu.
///
/// `progress_percent` wchodzi tylko wtedy, gdy jest co liczyć — stan bezczynny
/// ma w danych wyłącznie cztery klucze.
fn with_common_data(
    mut data: serde_json::Value,
    state: &SectorState,
    elevated: bool,
    drives: &[DiskDrive],
) -> serde_json::Value {
    let object = match data.as_object_mut() {
        Some(object) => object,
        None => return data,
    };
    object.insert("schema_version".to_string(), serde_json::json!(1));
    object.insert("elevated".to_string(), serde_json::json!(elevated));
    object.insert(
        "drives".to_string(),
        serde_json::json!(
            drives
                .iter()
                .map(|drive| serde_json::json!({
                    "index": drive.index,
                    "model": trimmed(drive.model.clone()),
                    "interface_type": trimmed(drive.interface_type.clone()),
                    "size_bytes": drive.size_bytes,
                }))
                .collect::<Vec<_>>()
        ),
    );
    if let Some(params) = state.params() {
        object.insert(
            "params".to_string(),
            serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
        );
    }
    if let Some(progress) = state.progress_percent() {
        object.insert(
            "progress_percent".to_string(),
            serde_json::to_value(progress).unwrap_or(serde_json::Value::Null),
        );
    }
    data
}

fn mode_key(mode: SectorMode) -> &'static str {
    match mode {
        SectorMode::Quick => "module.storage_sectors.value.mode.quick",
        SectorMode::Full => "module.storage_sectors.value.mode.full",
    }
}

/// Pozostały czas z tempa dotychczasowego przebiegu; `None`, gdy nic jeszcze
/// nie odczytano, więc nie ma z czego liczyć.
fn eta_ms(steps_done: u64, steps_total: u64, elapsed_ms: u64) -> Option<u64> {
    if steps_done == 0 {
        return None;
    }
    if steps_total <= steps_done {
        return Some(0);
    }
    let remaining =
        u128::from(elapsed_ms) * u128::from(steps_total - steps_done) / u128::from(steps_done);
    u64::try_from(remaining).ok()
}

/// Prędkość średnia z bieżącego przebiegu w MB/s (1 MB = 1 000 000 B).
fn read_speed_mbps(bytes_read: u64, elapsed_ms: u64) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    Some(bytes_read as f64 / (elapsed_ms as f64 / 1000.0) / 1_000_000.0)
}

fn format_speed(mbps: Option<f64>) -> String {
    match mbps {
        Some(value) if value.is_finite() => format!("{value:.1} MB/s"),
        _ => "-".to_string(),
    }
}

/// Pozycja błędu w MB dziesiętnych — jak „Error at … MB” w HD Tune. Jednostka
/// skanu (4 MiB) daje rozdzielczość 4,2 MB, więc jedno miejsce po przecinku
/// jest potrzebne.
fn format_position_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::sector::MAX_SECTOR_ERRORS;
    use sk_core::testing::{Harness, MockHost, MockSectors, MockWmi};

    fn harness(state: SectorState) -> Harness {
        let wmi =
            MockWmi::new().with_rows("Win32_DiskDrive", fixture_rows!("win32_diskdrive.json"));
        Harness::new()
            .with_wmi(wmi)
            .with_host(MockHost::elevated())
            .with_sectors(MockSectors::with_state(state))
    }

    fn sector_error(offset: u64, length: u64, detail: &str) -> SectorError {
        SectorError {
            offset_bytes: offset,
            length_bytes: length,
            detail: detail.to_string(),
        }
    }

    fn scan(cancelled: bool, errors: Vec<SectorError>) -> SectorScan {
        SectorScan {
            params: SectorParams::new(0, 16 * SECTOR_UNIT_BYTES, SectorMode::Full),
            disk_bytes: 16 * SECTOR_UNIT_BYTES,
            steps_done: 16,
            steps_total: 16,
            bytes_read: 16 * SECTOR_UNIT_BYTES,
            duration_ms: 8_000,
            errors,
            cancelled,
            errors_truncated: false,
        }
    }

    /// Pierwszy blok `KeyValues` widoku — wiersze wyniku.
    fn key_values(output: &ModuleOutput) -> &Vec<KeyValue> {
        output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::KeyValues { rows } => Some(rows),
                _ => None,
            })
            .expect("wiersze wyniku")
    }

    fn row_by_key<'a>(rows: &'a [KeyValue], key: &str) -> &'a KeyValue {
        rows.iter()
            .find(|row| row.label_key == key)
            .unwrap_or_else(|| panic!("brak wiersza {key}"))
    }

    fn map_of(output: &ModuleOutput) -> &SurfaceMap {
        output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Map { map } => Some(map),
                _ => None,
            })
            .expect("mapa powierzchni")
    }

    fn status_level(output: &ModuleOutput) -> StatusLevel {
        output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Status { level, .. } => Some(*level),
                _ => None,
            })
            .expect("baner stanu skanu")
    }

    #[test]
    fn idle_lists_drives_without_notes() {
        let output = StorageSectors
            .collect(&harness(SectorState::Idle).ctx())
            .expect("stan bezczynny to nie błąd");

        assert_eq!(output.data["state"], "idle");
        assert_eq!(output.data["schema_version"], 1);
        assert_eq!(output.data["elevated"], true);
        // Bezczynny widok to wyłącznie cztery klucze — bez postępu i parametrów.
        assert_eq!(
            output.data.as_object().map(serde_json::Map::len),
            Some(4),
            "{:?}",
            output.data
        );
        assert_eq!(
            output.data["drives"].as_array().map(Vec::len),
            Some(4),
            "fixture ma cztery dyski"
        );

        // Z uprawnieniami i działającym WMI widok bezczynny to sama lista
        // dysków — instrukcję „co uruchomić” mówi karta skanu w panelu GUI.
        assert!(
            output.view.sections[0]
                .blocks
                .iter()
                .all(|block| !matches!(block, Block::Note { .. })),
            "bezczynny widok nie ma notek: {:?}",
            output.view.sections[0].blocks
        );
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela dysków");
        assert_eq!(table.rows.len(), 4);
        assert_eq!(table.rows[0][0], "4");
        assert_eq!(table.rows[0][1], "WDC PC SN530 SDBPNPZ-512G-1114");
        assert_eq!(table.rows[3][1], "CT1000MX500SSD1");
    }

    #[test]
    fn idle_without_elevation_warns_about_rights() {
        let output = StorageSectors
            .collect(&Harness::new().ctx())
            .expect("brak WMI i uprawnień to nie błąd");
        assert_eq!(output.data["elevated"], false);
        assert!(
            output
                .view
                .uses_key("module.storage_sectors.note.elevation")
        );
        // Brak listy dysków to degradacja, nie pusty ekran.
        assert!(
            output
                .view
                .uses_key("module.storage_sectors.note.no_drives")
        );
        assert!(output.view.uses_key("module.storage_sectors.note.wmi"));
    }

    #[test]
    fn running_full_draws_the_map_a_cell_at_a_time() {
        // Zakres 4 MiB to jedna jednostka skanu, więc mapa ma jedną komórkę.
        let state = SectorState::Running {
            params: SectorParams::new(0, SECTOR_UNIT_BYTES, SectorMode::Full),
            disk_bytes: SECTOR_UNIT_BYTES,
            steps_done: 0,
            steps_total: 1,
            bytes_read: 0,
            elapsed_ms: 500,
            errors: Vec::new(),
        };
        let output = StorageSectors
            .collect(&harness(state).ctx())
            .expect("stan w toku");

        assert_eq!(output.data["state"], "running");
        assert_eq!(output.data["steps_done"], 0);
        assert_eq!(output.data["steps_total"], 1);
        assert_eq!(output.data["progress_percent"], 0.0);

        let map = map_of(&output);
        assert_eq!(map.columns, 64);
        assert_eq!(map.cells, vec![MapCell::Pending]);

        let rows = key_values(&output);
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.drive").value,
            "CT1000MX500SSD1"
        );
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.mode")
                .value_key
                .as_deref(),
            Some("module.storage_sectors.value.mode.full")
        );
        // Bez odczytanej jednostki nie ma z czego policzyć końca.
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.remaining").value,
            "-"
        );
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.unit").value,
            "4 MB"
        );

        // Po odczytaniu jedynej jednostki mapa jest zielona, a postęp pełny.
        let read = SectorState::Running {
            params: SectorParams::new(0, SECTOR_UNIT_BYTES, SectorMode::Full),
            disk_bytes: SECTOR_UNIT_BYTES,
            steps_done: 1,
            steps_total: 1,
            bytes_read: SECTOR_UNIT_BYTES,
            elapsed_ms: 800,
            errors: Vec::new(),
        };
        let output = StorageSectors
            .collect(&harness(read).ctx())
            .expect("stan w toku");
        assert_eq!(output.data["progress_percent"], 100.0);
        assert_eq!(map_of(&output).cells, vec![MapCell::Ok]);
        assert_eq!(
            row_by_key(key_values(&output), "module.storage_sectors.row.remaining").value,
            "0.0 s"
        );
    }

    #[test]
    fn running_quick_has_one_cell_per_probe() {
        // 640 jednostek, krok 10: 64 próbki po 8 jednostek (512 kroków), czyli
        // 64 komórki w 8 kolumnach — jedna na próbkę, nie na jednostkę.
        let span = 640 * SECTOR_UNIT_BYTES;
        let probes = sk_core::sector::quick_probes(span);
        assert_eq!(probes.len(), 64);
        let steps: u64 = probes.iter().map(|(_, len)| len / SECTOR_UNIT_BYTES).sum();
        assert_eq!(steps, 512);

        let state = SectorState::Running {
            params: SectorParams::new(0, span, SectorMode::Quick),
            disk_bytes: span,
            steps_done: steps,
            steps_total: steps,
            bytes_read: steps * SECTOR_UNIT_BYTES,
            elapsed_ms: 12_000,
            errors: Vec::new(),
        };
        let output = StorageSectors
            .collect(&harness(state).ctx())
            .expect("stan w toku");

        let map = map_of(&output);
        assert_eq!(map.cells.len(), 64);
        assert_eq!(map.columns, 8);
        assert!(map.cells.iter().all(|cell| *cell == MapCell::Ok));
        assert_eq!(
            row_by_key(key_values(&output), "module.storage_sectors.row.mode")
                .value_key
                .as_deref(),
            Some("module.storage_sectors.value.mode.quick")
        );
    }

    #[test]
    fn finished_scan_without_errors_is_clean() {
        let output = StorageSectors
            .collect(&harness(SectorState::Done(scan(false, Vec::new()))).ctx())
            .expect("zakończony skan");

        assert_eq!(output.data["state"], "done");
        assert_eq!(output.data["damaged_bytes"], 0);
        assert!(output.view.uses_key("module.storage_sectors.status.clean"));
        assert_eq!(status_level(&output), StatusLevel::Ok);
        assert!(
            !output
                .view
                .uses_key("module.storage_sectors.status.damaged")
        );
        assert!(
            !output
                .view
                .uses_key("module.storage_sectors.note.cancelled")
        );
        assert!(
            !output
                .view
                .uses_key("module.storage_sectors.note.truncated")
        );
        // Bez błędów nie ma tabeli nieczytelnych zakresów.
        assert!(
            !output.view.sections[0]
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Table { .. })),
            "tabela błędów przy czystym skanie"
        );
        let map = map_of(&output);
        assert_eq!(map.cells.len(), 16);
        assert!(map.cells.iter().all(|cell| *cell == MapCell::Ok));
        // Prędkość: 64 MiB w 8 s to 8,4 MB/s.
        assert_eq!(
            row_by_key(key_values(&output), "module.storage_sectors.row.speed").value,
            "8.4 MB/s"
        );
    }

    #[test]
    fn finished_scan_with_errors_is_bad_and_lists_them() {
        let errors = vec![
            sector_error(SECTOR_UNIT_BYTES, SECTOR_UNIT_BYTES, "CRC"),
            sector_error(2 * SECTOR_UNIT_BYTES, SECTOR_UNIT_BYTES, "timeout"),
        ];
        let output = StorageSectors
            .collect(&harness(SectorState::Done(scan(false, errors))).ctx())
            .expect("błędy odczytu to dana, nie błąd modułu");

        assert!(
            output
                .view
                .uses_key("module.storage_sectors.status.damaged")
        );
        assert_eq!(status_level(&output), StatusLevel::Bad);
        assert_eq!(output.data["damaged_bytes"], 2 * SECTOR_UNIT_BYTES);
        assert_eq!(output.data["errors"].as_array().map(Vec::len), Some(2));

        let rows = key_values(&output);
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.errors").value,
            "2"
        );
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.damaged").value,
            "8 MB"
        );

        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela nieczytelnych zakresów");
        assert_eq!(table.rows.len(), 2);
        // Pozycja w MB dziesiętnych z jednym miejscem po przecinku.
        assert_eq!(table.rows[0][0], "4.2 MB");
        assert_eq!(table.rows[0][1], "4 MB");
        assert_eq!(table.rows[0][2], "CRC");
        assert_eq!(table.rows[1][2], "timeout");
    }

    #[test]
    fn truncated_error_list_is_warned_about_and_capped() {
        let units = MAX_SECTOR_ERRORS as u64;
        let span = units * SECTOR_UNIT_BYTES;
        let errors: Vec<SectorError> = (0..units)
            .map(|index| sector_error(index * SECTOR_UNIT_BYTES, SECTOR_UNIT_BYTES, "CRC"))
            .collect();
        let mut scan = scan(false, errors);
        scan.params = SectorParams::new(0, span, SectorMode::Full);
        scan.disk_bytes = span;
        scan.steps_done = units;
        scan.steps_total = units;
        scan.bytes_read = span;
        scan.errors_truncated = true;

        let output = StorageSectors
            .collect(&harness(SectorState::Done(scan)).ctx())
            .expect("skończony skan");
        assert_eq!(output.data["errors_truncated"], true);
        assert!(
            output
                .view
                .uses_key("module.storage_sectors.note.truncated")
        );

        // Pełna lista jest w danych, a tabela pokazuje tylko pierwsze 20 wpisów.
        assert_eq!(
            output.data["errors"].as_array().map(Vec::len),
            Some(MAX_SECTOR_ERRORS)
        );
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela nieczytelnych zakresów");
        assert_eq!(table.rows.len(), MAX_ERROR_ROWS);
    }

    #[test]
    fn cancelled_scan_keeps_partial_map_and_warns() {
        let mut cancelled = scan(true, Vec::new());
        cancelled.steps_done = 4;
        cancelled.bytes_read = 4 * SECTOR_UNIT_BYTES;
        cancelled.duration_ms = 2_000;

        let output = StorageSectors
            .collect(&harness(SectorState::Done(cancelled)).ctx())
            .expect("przerwany skan");
        assert_eq!(output.data["cancelled"], true);
        assert!(
            output
                .view
                .uses_key("module.storage_sectors.note.cancelled")
        );
        let progress = output.data["progress_percent"]
            .as_f64()
            .expect("postęp liczbowy");
        assert!(progress < 100.0, "przerwany skan to {progress} %");
        // Mapa zostaje, ale dalsze komórki czekają.
        let map = map_of(&output);
        assert_eq!(map.cells[0], MapCell::Ok);
        assert!(map.cells[4..].iter().all(|cell| *cell == MapCell::Pending));
    }

    #[test]
    fn failed_scan_shows_device_message_instead_of_failing_module() {
        let params = SectorParams::new(0, SECTOR_UNIT_BYTES, SectorMode::Full);
        let state = SectorState::Failed {
            params: Some(params),
            message: "brak uprawnień: \\\\.\\PhysicalDrive0".to_string(),
        };
        let output = StorageSectors
            .collect(&harness(state).ctx())
            .expect("nieudany skan to nie błąd modułu");

        assert_eq!(output.data["state"], "failed");
        assert_eq!(
            output.data["error"],
            "brak uprawnień: \\\\.\\PhysicalDrive0"
        );
        assert!(output.view.uses_key("module.storage_sectors.note.failed"));
        let rows = key_values(&output);
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.drive").value,
            "CT1000MX500SSD1"
        );
        assert_eq!(
            row_by_key(rows, "module.storage_sectors.row.error").value,
            "brak uprawnień: \\\\.\\PhysicalDrive0"
        );
        // Proces bez uprawnień dostaje też notkę o administratorze.
        let output = StorageSectors
            .collect(
                &Harness::new()
                    .with_sectors(MockSectors::with_state(SectorState::Failed {
                        params: None,
                        message: "brak urządzenia".to_string(),
                    }))
                    .ctx(),
            )
            .expect("nieudany skan bez parametrów");
        assert!(
            output
                .view
                .uses_key("module.storage_sectors.note.elevation")
        );
        assert!(
            !key_values(&output)
                .iter()
                .any(|row| row.label_key == "module.storage_sectors.row.drive"),
            "bez parametrów nie ma wiersza dysku"
        );
    }

    #[test]
    fn eta_and_speed_follow_elapsed_time() {
        assert_eq!(eta_ms(25, 100, 1_000), Some(3_000));
        assert_eq!(bench::format_duration(3_000), "3.0 s");
        assert_eq!(eta_ms(0, 100, 1_000), None);
        assert_eq!(eta_ms(100, 100, 1_000), Some(0));
        // 100 MB w 2 s to 50 MB/s; bez czasu nie zgadujemy prędkości.
        assert_eq!(
            format_speed(read_speed_mbps(100_000_000, 2_000)),
            "50.0 MB/s"
        );
        assert_eq!(format_speed(read_speed_mbps(0, 0)), "-");
        assert_eq!(format_speed(None), "-");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        let span = SECTOR_UNIT_BYTES;
        let mut truncated = scan(false, vec![sector_error(0, span, "CRC")]);
        truncated.errors_truncated = true;
        let mut cancelled = scan(true, Vec::new());
        cancelled.steps_done = 4;
        let states = [
            SectorState::Idle,
            SectorState::Running {
                params: SectorParams::new(0, span, SectorMode::Full),
                disk_bytes: span,
                steps_done: 0,
                steps_total: 1,
                bytes_read: 0,
                elapsed_ms: 0,
                errors: Vec::new(),
            },
            SectorState::Running {
                params: SectorParams::new(0, span, SectorMode::Quick),
                disk_bytes: span,
                steps_done: 0,
                steps_total: 1,
                bytes_read: 0,
                elapsed_ms: 0,
                errors: Vec::new(),
            },
            SectorState::Done(scan(false, Vec::new())),
            SectorState::Done(scan(false, vec![sector_error(0, span, "CRC")])),
            SectorState::Done(truncated),
            SectorState::Done(cancelled),
            SectorState::Failed {
                params: Some(SectorParams::new(0, span, SectorMode::Full)),
                message: "x".to_string(),
            },
            SectorState::Failed {
                params: None,
                message: "x".to_string(),
            },
        ];
        for state in states {
            // Jeden wariant bez WMI i bez uprawnień, żeby pokryć notki degradacji.
            for harness in [harness(state), Harness::new()] {
                let output = StorageSectors
                    .collect(&harness.ctx())
                    .expect("zbieranie danych");
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
    }
}
