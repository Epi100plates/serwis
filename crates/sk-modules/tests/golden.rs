//! Testy golden: pilnują stabilnego **kształtu** widoków, czyli kontraktu raportów.
//!
//! Jeśli zmiana w module jest zamierzona, odśwież wzorce:
//!
//! ```powershell
//! $env:UPDATE_GOLDEN = "1"; cargo test -p sk-modules --test golden; Remove-Item Env:\UPDATE_GOLDEN
//! ```
//!
//! Pliki leżą w `crates/sk-modules/tests/golden/` i są czytelne w diffie.
//!
//! `allow` na dole jest wyłącznie dla **pomocników tego pliku testowego**:
//! clippy nie traktuje funkcji pomocniczych w `tests/` jako kodu testowego,
//! a reguła „zero `unwrap`/`panic` poza testami” nadal obowiązuje w produkcie.
#![allow(clippy::expect_used, clippy::panic)]

use sk_core::providers::{BusKind, DriveKind, PhysicalDrive};
use sk_core::surface::{SurfaceAccess, SurfaceParams, SurfaceSample, SurfaceScan, SurfaceState};
use sk_core::testing::{Harness, MockStorage, MockSurface, MockWmi};
use sk_core::view::View;
use sk_modules::registry;

fn fixture(name: &str) -> Vec<serde_json::Value> {
    let text = match name {
        "win32_processor.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_processor.json"
        )),
        "wmi_processorperformance.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/wmi_processorperformance.json"
        )),
        "win32_physicalmemory.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_physicalmemory.json"
        )),
        "win32_physicalmemoryarray.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_physicalmemoryarray.json"
        )),
        "win32_diskdrive.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_diskdrive.json"
        )),
        other => panic!("brak fixture {other}"),
    };
    serde_json::from_str(text).expect("fixture jest poprawnym JSON-em")
}

fn harness(rows: &[(&str, &str)]) -> Harness {
    let mut wmi = MockWmi::new();
    for (fragment, file) in rows {
        wmi = wmi.with_rows(*fragment, fixture(file));
    }
    Harness::new().with_wmi(wmi)
}

fn check(name: &str, view: &View) {
    let actual = serde_json::to_string_pretty(view).expect("serializacja widoku");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.json"));

    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("katalog golden");
        }
        std::fs::write(&path, format!("{actual}\n")).expect("zapis golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "brak wzorca {}: {err}. Uruchom testy z UPDATE_GOLDEN=1, żeby go utworzyć.",
            path.display()
        )
    });
    assert_eq!(
        format!("{actual}\n"),
        expected,
        "widok {name} zmienił się — jeśli to zamierzone, odśwież wzorzec (UPDATE_GOLDEN=1)"
    );
}

#[test]
fn hw_cpu_view_is_stable() {
    let registry = registry();
    let module = registry.find("hw.cpu").expect("moduł hw.cpu");
    let harness = harness(&[
        ("Win32_Processor", "win32_processor.json"),
        ("ProcessorPerformance", "wmi_processorperformance.json"),
    ]);
    let output = module.collect(&harness.ctx()).expect("zbieranie danych");
    check("hw_cpu_view", &output.view);
}

#[test]
fn hw_memory_view_is_stable() {
    let registry = registry();
    let module = registry.find("hw.memory").expect("moduł hw.memory");
    let harness = harness(&[
        (
            "Win32_PhysicalMemoryArray",
            "win32_physicalmemoryarray.json",
        ),
        ("Win32_PhysicalMemory", "win32_physicalmemory.json"),
    ]);
    let output = module.collect(&harness.ctx()).expect("zbieranie danych");
    check("hw_memory_view", &output.view);
}

/// Syntetyczna strona logu zdrowia NVMe — stałe wartości, żeby wzorzec był
/// deterministyczny. Prawdziwe dane SMART wymagają administratora, więc nie
/// mogą być fixtures w CI.
fn nvme_page() -> Vec<u8> {
    let mut page = vec![0u8; 512];
    page[1..3].copy_from_slice(&314u16.to_le_bytes()); // 41 °C
    page[3] = 100;
    page[4] = 10;
    page[5] = 3;
    page[48..64].copy_from_slice(&2_000_000u128.to_le_bytes());
    page[128..144].copy_from_slice(&1_234u128.to_le_bytes());
    page
}

/// Syntetyczne atrybuty ATA: 5, 9, 194 i 241 (bez przekroczonych progów).
fn ata_sectors() -> (Vec<u8>, Vec<u8>) {
    let mut attributes = vec![0u8; 512];
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
    let mut thresholds = vec![0u8; 512];
    thresholds[0] = 0x10;
    thresholds[2] = 5;
    thresholds[3] = 50;
    (attributes, thresholds)
}

fn storage_harness() -> Harness {
    let wmi = MockWmi::new().with_rows("Win32_DiskDrive", fixture("win32_diskdrive.json"));
    let (attributes, thresholds) = ata_sectors();
    let storage = MockStorage::new()
        .with_nvme(
            PhysicalDrive::new(4, BusKind::Nvme, DriveKind::Ssd),
            nvme_page(),
        )
        .with_ata(
            PhysicalDrive::new(0, BusKind::Sata, DriveKind::Ssd),
            attributes,
            thresholds,
        );
    Harness::new().with_wmi(wmi).with_storage(storage)
}

#[test]
fn storage_smart_view_is_stable() {
    let registry = registry();
    let module = registry.find("storage.smart").expect("moduł storage.smart");
    let output = module
        .collect(&storage_harness().ctx())
        .expect("zbieranie danych");
    check("storage_smart_view", &output.view);
}

/// Syntetyczna mapa prędkości: wartości stałe, żeby wzorzec był deterministyczny.
/// Prawdziwy pomiar wymaga administratora i dysku, więc nie może być fixture
/// w CI — tak samo jak bufory SMART.
///
/// Przebieg jest rozrzucony po całej powierzchni (0 / 256 GiB / koniec dysku),
/// bo tak działa próbkowanie z limitem czasu — i to ono ma być w wzorcu.
fn bench_harness() -> Harness {
    const DISK_BYTES: u64 = 512_105_932_800;
    const BLOCK_BYTES: u64 = 64 << 20;
    let wmi = MockWmi::new().with_rows("Win32_DiskDrive", fixture("win32_diskdrive.json"));
    let scan = SurfaceScan {
        params: SurfaceParams {
            drive: 0,
            span_bytes: DISK_BYTES,
            block_bytes: BLOCK_BYTES,
            random_reads: 200,
            time_limit_ms: 60_000,
        },
        disk_bytes: DISK_BYTES,
        bytes_read: 3 * BLOCK_BYTES,
        duration_ms: 58_400,
        samples: vec![
            SurfaceSample {
                offset_bytes: 0,
                speed_mbps: 520.0,
            },
            SurfaceSample {
                offset_bytes: 256 << 30,
                speed_mbps: 510.0,
            },
            SurfaceSample {
                offset_bytes: DISK_BYTES - BLOCK_BYTES,
                speed_mbps: 500.0,
            },
        ],
        errors: Vec::new(),
        access: Some(SurfaceAccess {
            reads: 200,
            avg_ms: 0.08,
            min_ms: 0.05,
            max_ms: 0.2,
            iops: 12_500.0,
        }),
        cancelled: false,
        time_limit_reached: false,
    };
    Harness::new()
        .with_wmi(wmi)
        .with_surface(MockSurface::with_state(SurfaceState::Done(scan)))
}

#[test]
fn storage_bench_view_is_stable() {
    let registry = registry();
    let module = registry.find("storage.bench").expect("moduł storage.bench");
    let output = module
        .collect(&bench_harness().ctx())
        .expect("zbieranie danych");
    check("storage_bench_view", &output.view);
}
