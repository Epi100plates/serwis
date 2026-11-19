//! Testy sprzętowe: uruchamiane wyłącznie ręcznie na maszynie serwisowej.
//!
//! ```powershell
//! cargo test -p sk-modules --test hardware -- --ignored
//! ```
//!
//! Sprawdzają, że moduły zbierają **realne** dane z WMI tej maszyny (a nie tylko
//! z fixtures) i że brak sprzętu (np. baterii na desktopie) daje degradację,
//! a nie błąd.

use sk_core::i18n::Locale;
use sk_core::module::CollectCtx;
use sk_core::sensors::SensorStore;
use sk_core::surface::IdleSurface;
use sk_win::{SystemClock, WinHostEnv, WinMachine, WinSensors, WinStorage, WinWmi};

/// Benchmark nie jest uruchamiany w testach sprzętowych — moduł ma pokazać
/// poprawny stan bezczynny.
const IDLE: IdleSurface = IdleSurface;
/// Skan sektorów ma własny wątek w GUI; tutaj widok dostaje stan bezczynny.
const IDLE_SECTORS: sk_core::sector::IdleSectors = sk_core::sector::IdleSectors;

fn ctx_parts() -> (
    WinWmi,
    SystemClock,
    WinHostEnv,
    WinMachine,
    WinStorage,
    sk_core::live::LiveHistory,
    SensorStore,
) {
    // Jedna próbka z prawdziwych czujników: testy sprzętowe mają widzieć
    // temperatury GPU, a nie pustą tabelę.
    let mut sensors = SensorStore::new(4);
    sensors.push(0.0, WinSensors::new().readings());
    (
        WinWmi::new(),
        SystemClock::new(),
        WinHostEnv::new(),
        WinMachine::new(),
        WinStorage::new(),
        sk_core::live::LiveHistory::new(4),
        sensors,
    )
}

#[test]
#[ignore = "wymaga prawdziwego WMI: cargo test -p sk-modules --test hardware -- --ignored"]
fn every_hardware_module_reads_real_data() {
    let registry = sk_modules::registry();
    let (wmi, clock, host, machine, storage, live, sensors) = ctx_parts();
    wmi.begin_round();
    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        machine: &machine,
        storage: &storage,
        live: &live,
        sensors: &sensors,
        sectors: &IDLE_SECTORS,
        surface: &IDLE,
        locale: Locale::Pl,
    };

    for id in [
        "hw.cpu",
        "hw.memory",
        "hw.board",
        "hw.bios",
        "hw.gpu",
        "hw.monitor",
        "hw.disk_inventory",
        "hw.sensors",
        "storage.bench",
        "system.info",
    ] {
        let module = registry
            .find(id)
            .unwrap_or_else(|| panic!("brak modułu {id}"));
        let output = module
            .collect(&ctx)
            .unwrap_or_else(|err| panic!("moduł {id} nie zebrał danych: {err}"));
        assert!(
            !output.view.sections.is_empty(),
            "moduł {id} zwrócił pusty widok"
        );
        assert!(
            output.data.is_object(),
            "moduł {id} zwrócił dane inne niż obiekt JSON"
        );
    }
}

#[test]
#[ignore = "wymaga prawdziwego WMI: cargo test -p sk-modules --test hardware -- --ignored"]
fn cpu_memory_and_disks_report_positive_numbers() {
    let registry = sk_modules::registry();
    let (wmi, clock, host, machine, storage, live, sensors) = ctx_parts();
    wmi.begin_round();
    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        machine: &machine,
        storage: &storage,
        live: &live,
        sensors: &sensors,
        sectors: &IDLE_SECTORS,
        surface: &IDLE,
        locale: Locale::Pl,
    };

    let cpu = registry
        .find("hw.cpu")
        .expect("moduł hw.cpu")
        .collect(&ctx)
        .expect("dane CPU");
    assert!(
        cpu.data["cores"].as_u64().unwrap_or(0) > 0,
        "brak liczby rdzeni: {}",
        cpu.data
    );

    let memory = registry
        .find("hw.memory")
        .expect("moduł hw.memory")
        .collect(&ctx)
        .expect("dane pamięci");
    assert!(
        memory.data["total_bytes"].as_u64().unwrap_or(0) > 0,
        "brak sumy pamięci: {}",
        memory.data
    );

    let disks = registry
        .find("hw.disk_inventory")
        .expect("moduł hw.disk_inventory")
        .collect(&ctx)
        .expect("dane dysków");
    assert!(
        disks.data["disks_total"].as_u64().unwrap_or(0) > 0,
        "brak dysków: {}",
        disks.data
    );
}

#[test]
#[ignore = "wymaga prawdziwego WMI: cargo test -p sk-modules --test hardware -- --ignored"]
fn missing_battery_degrades_instead_of_failing() {
    let registry = sk_modules::registry();
    let (wmi, clock, host, machine, storage, live, sensors) = ctx_parts();
    wmi.begin_round();
    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        machine: &machine,
        storage: &storage,
        live: &live,
        sensors: &sensors,
        sectors: &IDLE_SECTORS,
        surface: &IDLE,
        locale: Locale::Pl,
    };

    let battery = registry
        .find("hw.battery")
        .expect("moduł hw.battery")
        .collect(&ctx)
        .expect("brak baterii nie może być błędem modułu");
    // Na desktopie `present` = false, na laptopie dane; oba warianty są poprawne.
    assert!(battery.data["present"].is_boolean(), "{}", battery.data);
}

#[test]
#[ignore = "wymaga prawdziwych dysków i administratora: cargo test -p sk-modules --test hardware -- --ignored"]
fn smart_reports_drives_and_degrades_without_elevation() {
    let registry = sk_modules::registry();
    let (wmi, clock, host, machine, storage, live, sensors) = ctx_parts();
    wmi.begin_round();
    let ctx = CollectCtx {
        wmi: &wmi,
        clock: &clock,
        host: &host,
        machine: &machine,
        storage: &storage,
        live: &live,
        sensors: &sensors,
        sectors: &IDLE_SECTORS,
        surface: &IDLE,
        locale: Locale::Pl,
    };

    let output = registry
        .find("storage.smart")
        .expect("moduł storage.smart")
        .collect(&ctx)
        .expect("SMART nie może wywalić całego raportu");

    let drives = output.data["drives"].as_array().expect("lista dysków");
    assert!(!drives.is_empty(), "brak dysków w systemie");

    if output.data["elevated"].as_bool().unwrap_or(false) {
        assert!(
            output.data["drives_with_smart"].as_u64().unwrap_or(0) > 0,
            "z administratorem żaden dysk nie oddał SMART: {}",
            output.data
        );
    } else {
        // Bez uprawnień moduł nadal ma pokazać dyski i wyjaśnić, czego brakuje.
        assert!(output.view.uses_key("module.storage_smart.note.elevation"));
    }
}
