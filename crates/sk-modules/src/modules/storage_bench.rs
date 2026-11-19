//! `storage.bench` — mapa prędkości powierzchni dysku (odczyt sekwencyjny).
//!
//! Moduł nie wykonuje pomiaru w `collect()`: test trwa sekundy lub godziny,
//! więc uruchamia go wątek benchmarku aplikacji, a moduł tylko czyta gotowy
//! lub częściowy wynik przez [`SurfaceSource`]. Ten sam wzorzec co `system.live`
//! (`LiveSource`) sprawia, że `collect()` jest natychmiastowe i testowalne.
//!
//! Zakres: odczyt sekwencyjny z mapą prędkości (jak Benchmark w HD Tune),
//! opcjonalny test losowy 4K (czas dostępu, IOPS) i raport nieczytelnych
//! bloków. Zero zapisu — moduł nie zmienia danych na dysku.

use serde::Deserialize;
use sk_core::prelude::*;
use sk_core::surface::{SurfaceScan, SurfaceState, covered_bytes};

use crate::bench::{self, BenchOutcome};
use crate::util::{decode_all, format_bytes, number, text};

const DISK_QUERY: &str = queries::WIN32_DISK_DRIVE;
/// Ile wierszy nieczytelnych bloków pokazujemy w tabeli; pełna lista jest
/// w danych JSON (ograniczonej do 128 wpisów przez warstwę Windows).
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
pub struct StorageBench;

impl Module for StorageBench {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "storage.bench",
            name_key: "module.storage_bench.name",
            desc_key: "module.storage_bench.desc",
            group: Group::Storage,
            capability: Capability::Elevated,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let (drives, wmi_error) = load_drives(ctx);
        let state = ctx.surface.state();
        let elevated = ctx.host.is_elevated();

        let (blocks, data) = match &state {
            SurfaceState::Idle => idle_view(&drives, elevated),
            SurfaceState::Running {
                params,
                disk_bytes,
                bytes_read,
                elapsed_ms,
                samples,
            } => running_view(
                &drives,
                params,
                *disk_bytes,
                *bytes_read,
                *elapsed_ms,
                samples,
                state.progress_percent().unwrap_or(0.0),
            ),
            SurfaceState::Done(scan) => done_view(ctx, &drives, scan),
            SurfaceState::Failed { params, message } => {
                failed_view(&drives, params.as_ref(), message, elevated)
            }
        };

        let mut blocks = blocks;
        if wmi_error.is_some() {
            // Lista dysków to dana dodatkowa: brak WMI nie może ukryć wyniku
            // testu, ale technik ma wiedzieć, dlaczego model jest nieznany.
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.storage_bench.note.wmi",
            ));
        }
        blocks.push(Block::note(
            NoteLevel::Info,
            "module.storage_bench.note.readonly",
        ));

        let view = View::new(vec![
            Section::new("module.storage_bench.section").with_blocks(blocks),
        ]);

        let data = with_common_data(data, &state, elevated, &drives);
        Ok(ModuleOutput { data, view })
    }
}

/// Lista dysków z WMI. Błąd nie przerywa modułu — wracamy z pustą listą
/// i notką, bo wynik testu jest ważniejszy niż nazwa modelu.
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

/// Stan bezczynny: objaśnienie, lista dysków do wyboru i ograniczenia.
fn idle_view(drives: &[DiskDrive], elevated: bool) -> (Vec<Block>, serde_json::Value) {
    let mut blocks = vec![Block::note(
        NoteLevel::Info,
        "module.storage_bench.note.idle",
    )];

    if drives.is_empty() {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_bench.note.no_drives",
        ));
    } else {
        blocks.push(Block::Table {
            table: Table::new(
                vec![
                    "module.storage_bench.col.index".to_string(),
                    "module.storage_bench.col.model".to_string(),
                    "module.storage_bench.col.size".to_string(),
                    "module.storage_bench.col.interface".to_string(),
                ],
                drive_rows(drives),
            ),
        });
    }

    if !elevated {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_bench.note.elevation",
        ));
    }
    blocks.push(Block::note(
        NoteLevel::Info,
        "module.storage_bench.note.whole_disk",
    ));

    (blocks, serde_json::json!({ "state": "idle" }))
}

/// Test w toku: postęp, bieżąca prędkość i rosnący wykres.
///
/// Postęp przy ustawionym limicie czasu liczy się z zegara (patrz
/// [`SurfaceState::progress_percent`]), dlatego odczytany zakres pokazujemy
/// osobnym wierszem — inaczej nie byłoby widać, jak duży fragment objęła mapa.
fn running_view(
    drives: &[DiskDrive],
    params: &SurfaceParams,
    disk_bytes: u64,
    bytes_read: u64,
    elapsed_ms: u64,
    samples: &[SurfaceSample],
    progress: f64,
) -> (Vec<Block>, serde_json::Value) {
    let last_speed = samples.last().map(|sample| sample.speed_mbps);
    let model = drive_label(drives, params.drive);
    // Pozycja ostatniej próbki = jak daleko doszło próbkowanie powierzchni.
    let covered = covered_bytes(samples, params.block_bytes);

    let mut blocks = vec![Block::key_values(vec![
        KeyValue::new("module.storage_bench.row.drive", &model),
        KeyValue::new(
            "module.storage_bench.row.drive_index",
            params.drive.to_string(),
        ),
        KeyValue::new(
            "module.storage_bench.row.disk_size",
            format_bytes(disk_bytes),
        ),
        KeyValue::new(
            "module.storage_bench.row.progress",
            format!("{progress:.0} %"),
        ),
        KeyValue::new(
            "module.storage_bench.row.read",
            format!(
                "{} / {}",
                format_size(bytes_read),
                format_size(params.span_bytes)
            ),
        ),
        KeyValue::new(
            "module.storage_bench.row.covered",
            format!(
                "{} / {}",
                format_size(covered),
                format_size(params.span_bytes)
            ),
        ),
        time_limit_row(params),
        KeyValue::new(
            "module.storage_bench.row.current_speed",
            last_speed.map_or_else(|| "-".to_string(), format_speed),
        ),
        KeyValue::new(
            "module.storage_bench.row.samples",
            samples.len().to_string(),
        ),
    ])];

    blocks.push(Block::note(
        NoteLevel::Info,
        "module.storage_bench.note.running",
    ));

    // Wykres jest od pierwszej chwili: oś X to cały dysk w GB, a punkty
    // dochodzą na swoje miejsce w trakcie próbkowania.
    blocks.push(chart_block(
        bench::chart_points(samples),
        disk_bytes.max(params.span_bytes),
    ));

    let data = serde_json::json!({
        "state": "running",
        "params": params,
        "disk_bytes": disk_bytes,
        "bytes_read": bytes_read,
        "covered_bytes": covered,
        "elapsed_ms": elapsed_ms,
        "progress_percent": progress,
        "samples": samples,
    });
    (blocks, data)
}

/// Zakończony przebieg: ocena, statystyki, mapa prędkości, nieczytelne bloki.
fn done_view(
    ctx: &CollectCtx<'_>,
    drives: &[DiskDrive],
    scan: &SurfaceScan,
) -> (Vec<Block>, serde_json::Value) {
    let outcome = bench::outcome(scan);
    let (status_key, level) = match outcome {
        BenchOutcome::Complete => ("module.storage_bench.status.complete", StatusLevel::Ok),
        BenchOutcome::Cancelled => ("module.storage_bench.status.cancelled", StatusLevel::Warn),
        BenchOutcome::ReadErrors => ("module.storage_bench.status.read_errors", StatusLevel::Bad),
        BenchOutcome::Incomplete => ("module.storage_bench.status.incomplete", StatusLevel::Warn),
        // Limit czasu to normalne zakończenie skonfigurowanego testu:
        // dysk nie jest gorszy od tego, ile zdążyliśmy zmierzyć.
        BenchOutcome::TimeLimit => ("module.storage_bench.status.time_limit", StatusLevel::Ok),
    };

    let mut blocks = vec![Block::status(
        "module.storage_bench.status.label",
        status_key,
        level,
    )];

    let model = drive_label(drives, scan.params.drive);
    let describe = ctx.storage.describe(scan.params.drive).ok();
    // Ile powierzchni naprawdę objęła mapa: przy próbkowaniu rozrzuconym po
    // całym dysku to koniec ostatniej próbki, a nie liczba odczytanych bajtów.
    let covered = scan.covered_bytes();
    let mut rows = vec![
        KeyValue::new("module.storage_bench.row.drive", &model),
        KeyValue::new(
            "module.storage_bench.row.drive_index",
            scan.params.drive.to_string(),
        ),
        KeyValue::new(
            "module.storage_bench.row.disk_size",
            format_size(scan.disk_bytes),
        ),
        KeyValue::new(
            "module.storage_bench.row.block",
            format_size(scan.params.block_bytes),
        ),
        KeyValue::new(
            "module.storage_bench.row.read",
            format_size(scan.bytes_read),
        ),
        KeyValue::new(
            "module.storage_bench.row.covered",
            format!(
                "{} / {}",
                format_size(covered),
                format_size(scan.params.span_bytes)
            ),
        ),
        KeyValue::new(
            "module.storage_bench.row.duration",
            format_duration(scan.duration_ms),
        ),
        time_limit_row(&scan.params),
        KeyValue::new(
            "module.storage_bench.row.samples",
            scan.samples.len().to_string(),
        ),
    ];
    if let Some(drive) = describe {
        rows.push(KeyValue::state(
            "module.storage_bench.row.bus",
            bus_key(drive.bus),
        ));
        rows.push(KeyValue::state(
            "module.storage_bench.row.kind",
            kind_key(drive.kind),
        ));
    }

    if let Some(stats) = bench::stats(scan) {
        rows.extend([
            KeyValue::new(
                "module.storage_bench.row.speed_min",
                format_speed(stats.min_mbps),
            ),
            KeyValue::new(
                "module.storage_bench.row.speed_avg",
                format_speed(stats.avg_mbps),
            ),
            KeyValue::new(
                "module.storage_bench.row.speed_max",
                format_speed(stats.max_mbps),
            ),
        ]);
    }
    if let Some(access) = scan.access {
        rows.extend([
            KeyValue::new(
                "module.storage_bench.row.access_avg",
                format!("{:.2} ms", access.avg_ms),
            ),
            KeyValue::new(
                "module.storage_bench.row.access_min",
                format!("{:.2} ms", access.min_ms),
            ),
            KeyValue::new(
                "module.storage_bench.row.access_max",
                format!("{:.2} ms", access.max_ms),
            ),
            KeyValue::new(
                "module.storage_bench.row.iops",
                format!("{:.0} IOPS", access.iops),
            ),
            KeyValue::new(
                "module.storage_bench.row.random_reads",
                access.reads.to_string(),
            ),
        ]);
    }
    if !scan.errors.is_empty() {
        rows.push(KeyValue::new(
            "module.storage_bench.row.unreadable",
            scan.errors.len().to_string(),
        ));
    }
    blocks.push(Block::key_values(rows));

    match outcome {
        BenchOutcome::Cancelled => blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_bench.note.cancelled",
        )),
        BenchOutcome::ReadErrors => blocks.push(Block::note(
            NoteLevel::Error,
            "module.storage_bench.note.read_errors",
        )),
        BenchOutcome::Incomplete => blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_bench.note.incomplete",
        )),
        BenchOutcome::TimeLimit => blocks.push(Block::note(
            NoteLevel::Info,
            "module.storage_bench.note.time_limit",
        )),
        BenchOutcome::Complete => {}
    }

    let points = bench::chart_points(&scan.samples);
    let point_count = points.len();
    blocks.push(chart_block(
        points,
        scan.disk_bytes.max(scan.params.span_bytes),
    ));

    if !scan.errors.is_empty() {
        let rows: Vec<Vec<String>> = scan
            .errors
            .iter()
            .take(MAX_ERROR_ROWS)
            .map(|error| vec![format_size(error.offset_bytes), error.detail.clone()])
            .collect();
        blocks.push(Block::Table {
            table: Table::new(
                vec![
                    "module.storage_bench.col.position".to_string(),
                    "module.storage_bench.col.error".to_string(),
                ],
                rows,
            ),
        });
    }

    let stats = bench::stats(scan).map(|stats| {
        serde_json::json!({
            "min_mbps": stats.min_mbps,
            "avg_mbps": stats.avg_mbps,
            "max_mbps": stats.max_mbps,
            "bytes_read": stats.bytes_read,
            "duration_ms": stats.duration_ms,
            "sample_count": stats.sample_count,
        })
    });
    let data = serde_json::json!({
        "state": "done",
        "outcome": outcome_name(outcome),
        "scan": scan,
        "stats": stats,
        "chart_points": point_count,
    });
    (blocks, data)
}

fn failed_view(
    drives: &[DiskDrive],
    params: Option<&SurfaceParams>,
    message: &str,
    elevated: bool,
) -> (Vec<Block>, serde_json::Value) {
    let mut blocks = vec![Block::note(
        NoteLevel::Error,
        "module.storage_bench.note.failed",
    )];
    if let Some(params) = params {
        blocks.push(Block::key_values(vec![
            KeyValue::new(
                "module.storage_bench.row.drive",
                drive_label(drives, params.drive),
            ),
            KeyValue::new(
                "module.storage_bench.row.span",
                format_size(params.span_bytes),
            ),
        ]));
    }
    blocks.push(Block::key_values(vec![KeyValue::new(
        "module.storage_bench.row.error",
        message,
    )]));
    if !elevated {
        blocks.push(Block::note(
            NoteLevel::Warn,
            "module.storage_bench.note.elevation",
        ));
    }

    let data = serde_json::json!({
        "state": "failed",
        "params": params,
        "error": message,
    });
    (blocks, data)
}

/// Wykres mapy prędkości. Oś X jest rozpięta na cały dysk w GB i ustawiona
/// z góry, więc wykres rysuje się w gotowym układzie — punkty dochodzą na
/// swoje miejsce, zamiast rozciągać skalę przy każdej próbce.
fn chart_block(points: Vec<[f64; 2]>, disk_bytes: u64) -> Block {
    let mut chart = Chart::new("module.storage_bench.chart.speed", "GB", "MB/s")
        .with_zero_base()
        .with_series(vec![ChartSeries::new("READ", points)]);
    if disk_bytes > 0 {
        chart = chart.with_x_range(0.0, bench::gigabytes(disk_bytes));
    }
    Block::chart(chart)
}

fn with_common_data(
    mut data: serde_json::Value,
    state: &SurfaceState,
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
                    "model": crate::util::trimmed(drive.model.clone()),
                    "interface_type": crate::util::trimmed(drive.interface_type.clone()),
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
    object.insert(
        "progress_percent".to_string(),
        serde_json::to_value(state.progress_percent()).unwrap_or(serde_json::Value::Null),
    );
    data
}

fn outcome_name(outcome: BenchOutcome) -> &'static str {
    match outcome {
        BenchOutcome::Complete => "complete",
        BenchOutcome::Cancelled => "cancelled",
        BenchOutcome::ReadErrors => "read_errors",
        BenchOutcome::Incomplete => "incomplete",
        BenchOutcome::TimeLimit => "time_limit",
    }
}

/// Wiersz „Limit czasu testu”: liczba sekund albo stan „bez limitu”.
fn time_limit_row(params: &SurfaceParams) -> KeyValue {
    if params.time_limit_ms == 0 {
        KeyValue::state(
            "module.storage_bench.row.time_limit",
            "module.storage_bench.value.no_limit",
        )
    } else {
        KeyValue::new(
            "module.storage_bench.row.time_limit",
            format_duration(params.time_limit_ms),
        )
    }
}

fn format_speed(mbps: f64) -> String {
    if !mbps.is_finite() {
        return "-".to_string();
    }
    format!("{mbps:.1} MB/s")
}

/// Rozmiar z jednostką dobraną do wielkości: mapa prędkości operuje na MiB
/// (zakres, blok), a dyski na GB/TB. `format_bytes` zawsze pokazuje GB, więc
/// dla małych wartości dawałoby „0.0 GB”.
fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.2} TB", value / TIB)
    } else if value >= GIB {
        format!("{:.1} GB", value / GIB)
    } else if value >= MIB {
        format!("{:.0} MB", value / MIB)
    } else if value >= KIB {
        format!("{:.0} KB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds >= 60 {
        format!("{} min {:02} s", seconds / 60, seconds % 60)
    } else {
        format!("{:.1} s", millis as f64 / 1000.0)
    }
}

fn bus_key(bus: BusKind) -> &'static str {
    match bus {
        BusKind::Nvme => "module.storage_bench.bus.nvme",
        BusKind::Sata => "module.storage_bench.bus.sata",
        BusKind::Ata => "module.storage_bench.bus.ata",
        BusKind::Scsi => "module.storage_bench.bus.scsi",
        BusKind::Usb => "module.storage_bench.bus.usb",
        BusKind::Raid => "module.storage_bench.bus.raid",
        BusKind::Unknown | BusKind::Other => "common.value.unknown",
    }
}

fn kind_key(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Hdd => "common.value.hdd",
        DriveKind::Ssd => "common.value.ssd",
        DriveKind::Unknown => "common.value.unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::MAX_CHART_POINTS;
    use sk_core::surface::{SurfaceAccess, SurfaceReadError};
    use sk_core::testing::{Harness, MockSurface, MockWmi};

    fn harness(state: SurfaceState) -> Harness {
        let wmi =
            MockWmi::new().with_rows("Win32_DiskDrive", fixture_rows!("win32_diskdrive.json"));
        Harness::new()
            .with_wmi(wmi)
            .with_host(sk_core::testing::MockHost::elevated())
            .with_surface(MockSurface::with_state(state))
    }

    fn sample(offset_mib: u64, speed: f64) -> SurfaceSample {
        SurfaceSample {
            offset_bytes: offset_mib << 20,
            speed_mbps: speed,
        }
    }

    fn done_scan(cancelled: bool, errors: Vec<SurfaceReadError>) -> SurfaceScan {
        SurfaceScan {
            params: SurfaceParams {
                drive: 0,
                span_bytes: 4 << 20,
                block_bytes: 1 << 20,
                random_reads: 200,
                time_limit_ms: 60_000,
            },
            disk_bytes: 4 << 20,
            bytes_read: 2 << 20,
            duration_ms: 2500,
            // Początek i koniec zakresu — tak wygląda mapa próbkowana po całej
            // powierzchni, a nie tylko jej pierwsze bloki.
            samples: vec![sample(0, 500.0), sample(3, 480.0)],
            errors,
            access: Some(SurfaceAccess {
                reads: 200,
                avg_ms: 0.08,
                min_ms: 0.05,
                max_ms: 0.2,
                iops: 12_500.0,
            }),
            cancelled,
            time_limit_reached: false,
        }
    }

    #[test]
    fn idle_explains_how_to_start_and_lists_drives() {
        let output = StorageBench
            .collect(&harness(SurfaceState::Idle).ctx())
            .expect("stan bezczynny to nie błąd");
        assert_eq!(output.data["state"], "idle");
        assert!(output.view.uses_key("module.storage_bench.note.idle"));
        assert!(output.view.uses_key("module.storage_bench.note.readonly"));
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela dysków");
        assert_eq!(table.rows.len(), 4);
        assert_eq!(table.rows[0][1], "WDC PC SN530 SDBPNPZ-512G-1114");
    }

    #[test]
    fn idle_without_elevation_warns_about_rights() {
        let output = StorageBench
            .collect(
                &Harness::new()
                    .with_surface(MockSurface::with_state(SurfaceState::Idle))
                    .ctx(),
            )
            .expect("brak WMI i uprawnień to nie błąd");
        assert!(output.view.uses_key("module.storage_bench.note.elevation"));
        assert_eq!(output.data["elevated"], false);
    }

    #[test]
    fn running_shows_progress_and_growing_chart() {
        // Bez limitu czasu postęp to udział odczytanego zakresu.
        let state = SurfaceState::Running {
            params: SurfaceParams {
                drive: 0,
                span_bytes: 4 << 20,
                block_bytes: 1 << 20,
                random_reads: 0,
                time_limit_ms: 0,
            },
            disk_bytes: 4 << 20,
            bytes_read: 2 << 20,
            elapsed_ms: 1000,
            samples: vec![sample(0, 500.0), sample(1, 480.0)],
        };
        let output = StorageBench
            .collect(&harness(state).ctx())
            .expect("stan w toku");
        assert_eq!(output.data["state"], "running");
        assert_eq!(output.data["progress_percent"], 50.0);
        assert!(output.view.uses_key("module.storage_bench.note.running"));
        assert!(output.view.uses_key("module.storage_bench.chart.speed"));
    }

    #[test]
    fn running_with_a_time_limit_reports_clock_progress() {
        // 30 s z 60 s limitu to 50 % — mimo że odczytano dopiero 2 MB z 4 MB
        // i tak samo wyglądałby przebieg na dysku 2 TB.
        let state = SurfaceState::Running {
            params: SurfaceParams::new(0, 4 << 20),
            disk_bytes: 4 << 20,
            bytes_read: 1 << 20,
            elapsed_ms: 30_000,
            samples: vec![sample(0, 500.0)],
        };
        let output = StorageBench
            .collect(&harness(state).ctx())
            .expect("stan w toku");
        assert_eq!(output.data["progress_percent"], 50.0);
        assert_eq!(output.data["elapsed_ms"], 30_000);
        assert!(output.view.uses_key("module.storage_bench.row.time_limit"));
        assert!(output.view.uses_key("module.storage_bench.row.read"));
    }

    #[test]
    fn time_limited_scan_reports_coverage_and_stays_ok() {
        let mut scan = done_scan(false, Vec::new());
        scan.params.span_bytes = 8 << 20;
        scan.bytes_read = 3 << 20;
        scan.duration_ms = 60_000;
        scan.samples = vec![sample(0, 500.0), sample(2, 490.0)];
        scan.time_limit_reached = true;

        let output = StorageBench
            .collect(&harness(SurfaceState::Done(scan)).ctx())
            .expect("test z limitem czasu");
        assert_eq!(output.data["outcome"], "time_limit");
        assert!(
            output
                .view
                .uses_key("module.storage_bench.status.time_limit")
        );
        assert!(output.view.uses_key("module.storage_bench.note.time_limit"));
        // Pokrycie zakresu jest osobnym wierszem, bo sam status mówi tylko,
        // że test wykonał się w całości.
        let rows = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::KeyValues { rows } => Some(rows),
                _ => None,
            })
            .expect("wiersze wyniku");
        assert!(
            rows.iter()
                .any(|row| row.label_key == "module.storage_bench.row.covered"),
            "brak wiersza pokrycia"
        );
        assert!(
            rows.iter()
                .any(|row| row.label_key == "module.storage_bench.row.time_limit"),
            "brak wiersza limitu czasu"
        );
        assert!(output.view.uses_key("module.storage_bench.chart.speed"));
    }

    #[test]
    fn finished_scan_reports_stats_and_chart() {
        let output = StorageBench
            .collect(&harness(SurfaceState::Done(done_scan(false, Vec::new()))).ctx())
            .expect("zakończony test");
        assert_eq!(output.data["state"], "done");
        assert_eq!(output.data["outcome"], "complete");
        assert_eq!(output.data["stats"]["min_mbps"], 480.0);
        assert_eq!(output.data["stats"]["avg_mbps"], 490.0);
        assert_eq!(output.data["stats"]["max_mbps"], 500.0);
        assert!(output.view.uses_key("module.storage_bench.status.complete"));
        assert!(output.view.uses_key("module.storage_bench.row.speed_avg"));
        assert!(output.view.uses_key("module.storage_bench.row.iops"));
        assert!(
            !output
                .view
                .uses_key("module.storage_bench.note.read_errors")
        );
    }

    #[test]
    fn read_errors_are_reported_as_bad_state_with_table() {
        let errors = vec![SurfaceReadError {
            offset_bytes: 1 << 20,
            detail: "błąd we/wy".to_string(),
        }];
        let output = StorageBench
            .collect(&harness(SurfaceState::Done(done_scan(false, errors))).ctx())
            .expect("błędy odczytu to dana, nie błąd modułu");
        assert_eq!(output.data["outcome"], "read_errors");
        assert!(
            output
                .view
                .uses_key("module.storage_bench.status.read_errors")
        );
        assert!(
            output
                .view
                .uses_key("module.storage_bench.note.read_errors")
        );
        let table = output.view.sections[0]
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .find(|table| {
                table.headers.first().map(String::as_str)
                    == Some("module.storage_bench.col.position")
            })
            .expect("tabela nieczytelnych bloków");
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][1], "błąd we/wy");
    }

    #[test]
    fn cancelled_scan_is_warned_but_keeps_partial_data() {
        let mut scan = done_scan(true, Vec::new());
        scan.bytes_read = 2 << 20;
        scan.samples = vec![sample(0, 500.0)];
        let output = StorageBench
            .collect(&harness(SurfaceState::Done(scan)).ctx())
            .expect("przerwany test");
        assert_eq!(output.data["outcome"], "cancelled");
        assert!(
            output
                .view
                .uses_key("module.storage_bench.status.cancelled")
        );
        assert!(output.view.uses_key("module.storage_bench.note.cancelled"));
        // Wykres zostaje nawet przy jednej próbce — ma stały układ całego dysku.
        assert!(output.view.uses_key("module.storage_bench.chart.speed"));
    }

    #[test]
    fn chart_axis_covers_the_whole_disk_from_the_first_frame() {
        let output = StorageBench
            .collect(&harness(SurfaceState::Done(done_scan(false, Vec::new()))).ctx())
            .expect("zakończony test");
        let chart = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Chart { chart } => Some(chart),
                _ => None,
            })
            .expect("wykres prędkości");
        assert_eq!(chart.x_label, "GB");
        assert_eq!(chart.x_min, Some(0.0));
        // 4 MiB dysku = 0.00390625 GB; oś musi sięgać końca nośnika.
        let max = chart.x_max.expect("stały koniec osi");
        assert!((max - 4.0 / 1024.0).abs() < 1e-9, "x_max = {max}");
        assert!(chart.from_zero);

        // Punkty leżą na absolutnych pozycjach: 0 GB i 3 MiB = 0.0029296875 GB.
        let points = &chart.series[0].points;
        assert_eq!(points.len(), 2);
        assert_eq!(points[0][0], 0.0);
        assert!((points[1][0] - 3.0 / 1024.0).abs() < 1e-9, "{points:?}");

        // Podgląd na żywo rysuje ten sam układ, zanim przyjdzie pierwsza próbka.
        let running = SurfaceState::Running {
            params: SurfaceParams::new(0, 4 << 20),
            disk_bytes: 4 << 20,
            bytes_read: 0,
            elapsed_ms: 0,
            samples: Vec::new(),
        };
        let live = StorageBench
            .collect(&harness(running).ctx())
            .expect("stan w toku");
        let live_chart = live.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Chart { chart } => Some(chart),
                _ => None,
            })
            .expect("wykres w trakcie testu");
        assert_eq!(live_chart.x_max, chart.x_max);
        assert!(live_chart.series[0].points.is_empty());
    }

    #[test]
    fn failed_state_shows_message_instead_of_failing_module() {
        let params = SurfaceParams::new(0, 1 << 20);
        let state = SurfaceState::Failed {
            params: Some(params),
            message: "brak uprawnień: \\\\.\\PhysicalDrive0".to_string(),
        };
        let output = StorageBench
            .collect(&harness(state).ctx())
            .expect("nieudany test to nie błąd modułu");
        assert_eq!(output.data["state"], "failed");
        assert!(output.view.uses_key("module.storage_bench.note.failed"));
        assert!(output.view.uses_key("module.storage_bench.row.error"));
    }

    #[test]
    fn chart_never_exceeds_point_limit() {
        let samples: Vec<SurfaceSample> = (0..MAX_CHART_POINTS * 2)
            .map(|index| sample(index as u64, 100.0))
            .collect();
        let points = bench::chart_points(&samples);
        assert_eq!(points.len(), MAX_CHART_POINTS);
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        let mut time_limited = done_scan(false, Vec::new());
        time_limited.time_limit_reached = true;
        time_limited.bytes_read = 1 << 20;
        time_limited.params.span_bytes = 8 << 20;
        let states = [
            SurfaceState::Idle,
            SurfaceState::Running {
                params: SurfaceParams::new(0, 1 << 20),
                disk_bytes: 1 << 20,
                bytes_read: 0,
                elapsed_ms: 0,
                samples: Vec::new(),
            },
            SurfaceState::Done(done_scan(false, Vec::new())),
            SurfaceState::Done(done_scan(
                false,
                vec![SurfaceReadError {
                    offset_bytes: 0,
                    detail: "x".to_string(),
                }],
            )),
            SurfaceState::Done(time_limited),
            SurfaceState::Failed {
                params: None,
                message: "x".to_string(),
            },
        ];
        for state in states {
            let output = StorageBench
                .collect(&harness(state).ctx())
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
