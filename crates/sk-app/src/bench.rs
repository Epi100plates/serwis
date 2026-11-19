//! Test prędkości dysku w wątku tła.
//!
//! Benchmark trwa od sekund do godzin, więc nigdy nie biegnie na wątku UI ani
//! na wspólnym wątku zbierania modułów (blokowałby odświeżanie i telemetrię).
//! Postęp trafia do [`SurfaceStore`], skąd czytają go panel i moduł
//! `storage.bench`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use sk_core::surface::{SurfaceParams, SurfaceState, SurfaceStore};
use sk_win::scan_surface;

/// Jak często aktualizujemy częściowy wynik. Próbki zbieramy wszystkie,
/// ale do magazynu wysyłamy migawkę najwyżej co 200 ms — inaczej kopiowanie
/// rosnącej listy próbek zjadałoby procesor.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Uruchamia test w osobnym wątku. Błąd samego uruchomienia wątku trafia do
/// magazynu jako `Failed`, żeby UI pokazało przyczynę zamiast ciszy.
pub fn spawn(
    store: Arc<SurfaceStore>,
    params: SurfaceParams,
    disk_bytes: u64,
    cancel: Arc<AtomicBool>,
) {
    let worker = {
        let store = Arc::clone(&store);
        thread::Builder::new()
            .name("serwiskit-bench".to_string())
            .spawn(move || run(store, params, disk_bytes, cancel))
    };
    if let Err(err) = worker {
        store.set(SurfaceState::Failed {
            params: Some(params),
            message: format!("nie udało się uruchomić wątku testu: {err}"),
        });
    }
}

fn run(store: Arc<SurfaceStore>, params: SurfaceParams, disk_bytes: u64, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    store.set(SurfaceState::Running {
        params,
        disk_bytes,
        bytes_read: 0,
        elapsed_ms: 0,
        samples: Vec::new(),
    });

    let mut samples = Vec::new();
    let mut last_update = Instant::now();
    let result = scan_surface(&params, &cancel, &mut |sample, bytes_read| {
        samples.push(*sample);
        if last_update.elapsed() >= PROGRESS_INTERVAL {
            store.set(SurfaceState::Running {
                params,
                disk_bytes,
                bytes_read,
                // Zegar jest potrzebny, bo przy limicie czasu to on wyznacza
                // postęp (patrz `SurfaceState::progress_percent`).
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                samples: samples.clone(),
            });
            last_update = Instant::now();
        }
    });

    match result {
        Ok(scan) => store.set(SurfaceState::Done(scan)),
        Err(error) => store.set(SurfaceState::Failed {
            params: Some(params),
            message: error.to_string(),
        }),
    }
}
