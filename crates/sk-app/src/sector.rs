//! Skan nieczytelnych sektorów w wątku tła.
//!
//! Skan czyta całą powierzchnię (tryb dokładny) albo próbki rozrzucone po
//! nośniku (tryb szybki), więc nigdy nie biegnie na wątku UI. Postęp trafia do
//! [`SectorStore`], skąd czytają go panel i moduł `storage.sectors` — ten sam
//! wzorzec co przy teście prędkości (`bench.rs`).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use sk_core::sector::{SectorError, SectorParams, SectorProgress, SectorState, SectorStore, plan};
use sk_win::scan_sectors;

/// Jak często aktualizujemy częściowy wynik (mapa i lista błędów).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Uruchamia skan w osobnym wątku. Błąd uruchomienia wątku trafia do magazynu
/// jako `Failed`, żeby UI pokazało przyczynę zamiast ciszy.
pub fn spawn(store: Arc<SectorStore>, params: SectorParams, cancel: Arc<AtomicBool>) {
    let worker = {
        let store = Arc::clone(&store);
        thread::Builder::new()
            .name("serwiskit-sector".to_string())
            .spawn(move || run(store, params, cancel))
    };
    if let Err(err) = worker {
        store.set(SectorState::Failed {
            params: Some(params),
            message: format!("nie udało się uruchomić wątku skanu: {err}"),
        });
    }
}

fn run(store: Arc<SectorStore>, params: SectorParams, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    let steps_total = plan(&params).steps();
    store.set(SectorState::Running {
        params,
        disk_bytes: params.span_bytes,
        steps_done: 0,
        steps_total,
        bytes_read: 0,
        elapsed_ms: 0,
        errors: Vec::new(),
    });

    let mut last_update = Instant::now();
    let mut errors: Vec<SectorError> = Vec::new();
    let result = scan_sectors(&params, &cancel, &mut |progress: SectorProgress<'_>| {
        // Migawka błędów rośnie razem z mapą — moduł czerwieni komórki
        // w trakcie skanu, a nie dopiero po jego zakończeniu.
        errors = progress.errors.to_vec();
        if last_update.elapsed() >= PROGRESS_INTERVAL {
            store.set(SectorState::Running {
                params,
                disk_bytes: params.span_bytes,
                steps_done: progress.steps_done,
                steps_total: progress.steps_total,
                bytes_read: progress.bytes_read,
                elapsed_ms: elapsed_ms(started),
                errors: errors.clone(),
            });
            last_update = Instant::now();
        }
    });

    match result {
        Ok(scan) => store.set(SectorState::Done(scan)),
        Err(error) => store.set(SectorState::Failed {
            params: Some(params),
            message: error.to_string(),
        }),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
