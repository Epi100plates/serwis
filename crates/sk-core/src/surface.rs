//! Pomiar prędkości powierzchni dysku (benchmark odczytu).
//!
//! Ten moduł zawiera wyłącznie typy i magazyn wyniku. Sam odczyt urządzenia
//! żyje w `sk-win` (jedyne miejsce z `unsafe`), a analiza i widok — w module
//! `storage.bench`. Dzięki temu moduł testuje się na przygotowanym wyniku,
//! bez dysku i bez uprawnień administratora.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Najmniejszy blok odczytu. Mniejszy nie ma sensu: narzut wywołań zjada
/// różnicę, a dysk i tak czyta całymi stronami.
pub const MIN_BLOCK_BYTES: u64 = 1 << 20;
/// Górny limit bloku — przy ogromnych dyskach ogranicza pamięć i czas reakcji.
pub const MAX_BLOCK_BYTES: u64 = 256 << 20;
/// Górny limit próbek trzymanych w pamięci (16k × 16 B ≈ 256 KiB).
pub const MAX_SAMPLES: usize = 16_384;
/// Domyślna liczba losowych odczytów 4K w fazie czasu dostępu.
pub const DEFAULT_RANDOM_READS: u32 = 200;
/// Rozmiar bloku w fazie losowej (4K, wyrównany do sektora).
pub const RANDOM_BLOCK_BYTES: u32 = 4096;
/// Domyślny limit czasu przebiegu (1 minuta). To on sprawia, że test na dysku
/// 2 TB kończy się w rozsądnym czasie, a nie po godzinach: po jego upływie faza
/// sekwencyjna zatrzymuje się z tym, co już zmierzone.
pub const DEFAULT_TIME_LIMIT_MS: u64 = 60_000;
/// Ile czasu z limitu rezerwujemy na fazę losową (czas dostępu 4K). Bez tej
/// rezerwy szybki dysk zjadłby cały budżet na mapę i pomiar IOPS nie odbyłby
/// się wcale.
pub const RANDOM_PHASE_RESERVE_MS: u64 = 3_000;

/// Parametry jednego przebiegu. `span_bytes` jest zawsze dodatnie i wyrównane
/// do `block_bytes`; urządzenie może dodatkowo ograniczyć zakres (patrz
/// [`SurfaceParams::clamped_to`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceParams {
    pub drive: u32,
    pub span_bytes: u64,
    pub block_bytes: u64,
    pub random_reads: u32,
    /// Limit czasu przebiegu w ms; 0 = bez limitu. Po jego upływie faza
    /// sekwencyjna kończy się wynikiem częściowym (`time_limit_reached`).
    #[serde(default)]
    pub time_limit_ms: u64,
}

impl SurfaceParams {
    /// Parametry dla zakresu `span_bytes` z blokiem dobranym do liczby próbek.
    pub fn new(drive: u32, span_bytes: u64) -> Self {
        Self {
            drive,
            span_bytes,
            block_bytes: block_for_span(span_bytes),
            random_reads: DEFAULT_RANDOM_READS,
            time_limit_ms: DEFAULT_TIME_LIMIT_MS,
        }
    }

    /// Ustawia limit czasu przebiegu; 0 wyłącza limit (test trwa tyle, ile
    /// zajmie cały zakres).
    pub fn with_time_limit_ms(mut self, time_limit_ms: u64) -> Self {
        self.time_limit_ms = time_limit_ms;
        self
    }

    /// Wyłącza fazę losową (np. gdy urządzenie jej nie obsługuje).
    pub fn without_random_reads(mut self) -> Self {
        self.random_reads = 0;
        self
    }

    /// Dopasowuje zakres i blok do rzeczywistego rozmiaru dysku.
    ///
    /// Zakres jest zaokrąglany w dół do całych bloków — odczyt poza końcem
    /// urządzenia kończy się błędem, a nie skróceniem danych.
    pub fn clamped_to(&self, disk_bytes: u64) -> Self {
        let mut params = *self;
        let block = if self.block_bytes == 0 {
            block_for_span(self.span_bytes)
        } else {
            self.block_bytes.clamp(MIN_BLOCK_BYTES, MAX_BLOCK_BYTES)
        };
        params.block_bytes = block;
        let span = self.span_bytes.min(disk_bytes);
        params.span_bytes = span / block * block;
        params
    }
}

/// Blok odczytu tak dobrany, żeby liczba próbek nie przekroczyła [`MAX_SAMPLES`].
///
/// Dla 1 GiB daje 1 MiB (1024 próbki), dla 512 GB — 32 MiB, a dla 2 TB — 128 MiB.
/// Kolejne przebiegi po całej powierzchni mają więc podobną rozdzielczość mapy
/// przy ograniczonej pamięci.
pub fn block_for_span(span_bytes: u64) -> u64 {
    let per_sample = span_bytes / MAX_SAMPLES as u64;
    let block = per_sample.max(MIN_BLOCK_BYTES).div_ceil(MIN_BLOCK_BYTES) * MIN_BLOCK_BYTES;
    block.clamp(MIN_BLOCK_BYTES, MAX_BLOCK_BYTES)
}

/// Jedna próbka mapy prędkości: pozycja na dysku i zmierzona przepustowość.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceSample {
    /// Offset początku bloku od początku dysku, w bajtach.
    pub offset_bytes: u64,
    /// Przepustowość bloku w MB/s (1 MB = 1 000 000 B, jak w HD Tune).
    pub speed_mbps: f64,
}

/// Blok, którego nie udało się odczytać — odpowiednik „czerwonego” sektora
/// w teście powierzchni. `detail` to komunikat systemu (dana techniczna).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceReadError {
    pub offset_bytes: u64,
    pub detail: String,
}

/// Wynik fazy losowej: czas dostępu i IOPS dla odczytu 4K, kolejka QD1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceAccess {
    pub reads: u32,
    pub avg_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub iops: f64,
}

/// Zakończony (lub przerwany) przebieg.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceScan {
    pub params: SurfaceParams,
    /// Rozmiar urządzenia odczytany z uchwytu w chwili startu.
    pub disk_bytes: u64,
    pub bytes_read: u64,
    pub duration_ms: u64,
    pub samples: Vec<SurfaceSample>,
    pub errors: Vec<SurfaceReadError>,
    pub access: Option<SurfaceAccess>,
    /// Przerwane przez użytkownika — dane częściowe, nie wynik końcowy.
    pub cancelled: bool,
    /// Fazę sekwencyjną zatrzymał limit czasu: mapa obejmuje tylko odczytany
    /// początek zakresu. To nie błąd — tak kończy się test z limitem.
    #[serde(default)]
    pub time_limit_reached: bool,
}

impl SurfaceScan {
    /// Koniec zmierzonego fragmentu powierzchni w bajtach.
    ///
    /// Równy `params.span_bytes` dla pełnego przebiegu, a mniejszy po
    /// zatrzymaniu limitem czasu, anulowaniu albo błędach odczytu — wykres
    /// skaluje się wtedy do tego, co naprawdę zmierzono.
    pub fn covered_bytes(&self) -> u64 {
        covered_bytes(&self.samples, self.params.block_bytes)
    }

    /// Czy przebieg dotarł do końca zakresu: ostatnia próbka obejmuje ostatni
    /// blok powierzchni.
    ///
    /// Przy próbkowaniu rozrzuconym po całym dysku to jedyny sensowny test
    /// pokrycia — odczytanych bajtów jest mniej niż rozmiar dysku i tak ma być.
    pub fn reached_end(&self) -> bool {
        self.params.span_bytes == 0 || self.covered_bytes() >= self.params.span_bytes
    }
}

/// Koniec zmierzonego fragmentu z listy próbek: offset ostatniej próbki plus
/// cały blok. Wydzielone, bo podgląd na żywo (`SurfaceState::Running`) ma
/// próbki, ale nie ma jeszcze gotowego [`SurfaceScan`].
pub fn covered_bytes(samples: &[SurfaceSample], block_bytes: u64) -> u64 {
    samples
        .iter()
        .map(|sample| sample.offset_bytes.saturating_add(block_bytes))
        .max()
        .unwrap_or(0)
}

/// Stan testu widziany przez moduł i UI.
///
/// `Running` niesie częściowe próbki, dzięki czemu wykres rośnie w trakcie
/// pomiaru. To ten sam wzorzec co `LiveSource`: moduł tylko czyta stan.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SurfaceState {
    #[default]
    Idle,
    Running {
        params: SurfaceParams,
        disk_bytes: u64,
        bytes_read: u64,
        /// Czas od startu przebiegu — postęp przy ustawionym limicie czasu
        /// liczy się z zegara, nie z odczytanych bajtów.
        elapsed_ms: u64,
        samples: Vec<SurfaceSample>,
    },
    Done(SurfaceScan),
    Failed {
        params: Option<SurfaceParams>,
        message: String,
    },
}

impl SurfaceState {
    /// Postęp testu w procentach (0–100) albo `None`, gdy test nie jest w toku.
    ///
    /// Przy ustawionym limicie czasu miernikiem jest zegar: to on wyznacza
    /// koniec testu, więc pasek dochodzi do 100 % także wtedy, gdy przebieg
    /// objął tylko początek dużej powierzchni. Bez limitu postęp to udział
    /// odczytanego zakresu.
    pub fn progress_percent(&self) -> Option<f64> {
        match self {
            Self::Running {
                params,
                bytes_read,
                elapsed_ms,
                ..
            } => {
                if params.time_limit_ms > 0 {
                    percent_of(*elapsed_ms as f64, params.time_limit_ms as f64)
                } else {
                    percent_of(*bytes_read as f64, params.span_bytes as f64)
                }
            }
            // Przerwany test nie może raportować 100 % — postęp liczymy
            // z faktycznie odczytanych bajtów, nie z faktu zakończenia.
            Self::Done(scan) if scan.cancelled => {
                percent_of(scan.bytes_read as f64, scan.params.span_bytes as f64)
            }
            // Zakończony test — także ten zatrzymany limitem czasu albo
            // skróconym zakresem — jest w całości wykonany.
            Self::Done(_) => Some(100.0),
            _ => None,
        }
    }

    pub fn params(&self) -> Option<&SurfaceParams> {
        match self {
            Self::Running { params, .. } => Some(params),
            Self::Done(scan) => Some(&scan.params),
            Self::Failed { params, .. } => params.as_ref(),
            Self::Idle => None,
        }
    }
}

/// Procent wartości względem całości; `None`, gdy całość jest nieznana.
fn percent_of(value: f64, total: f64) -> Option<f64> {
    if total <= 0.0 {
        return None;
    }
    Some((value / total * 100.0).clamp(0.0, 100.0))
}

/// Źródło ostatniego wyniku testu. Implementacja realna ([`SurfaceStore`])
/// jest zapisywana przez wątek benchmarku, a odczytywana przez moduł —
/// ten sam podział odpowiedzialności co przy [`crate::providers::LiveSource`].
pub trait SurfaceSource: Send + Sync {
    fn state(&self) -> SurfaceState;
}

/// Magazyn wyniku współdzielony między wątkiem testu a modułem.
#[derive(Debug, Default)]
pub struct SurfaceStore {
    state: Mutex<SurfaceState>,
}

impl SurfaceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, state: SurfaceState) {
        match self.state.lock() {
            Ok(mut slot) => *slot = state,
            // Trucizna blokady nie może zgubić wyniku testu — przejmujemy dane.
            Err(poisoned) => *poisoned.into_inner() = state,
        }
    }

    pub fn get(&self) -> SurfaceState {
        match self.state.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl SurfaceSource for SurfaceStore {
    fn state(&self) -> SurfaceState {
        self.get()
    }
}

/// Źródło bez testu — dla trybu CLI i raportu, gdzie benchmark się nie uruchamia.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdleSurface;

impl SurfaceSource for IdleSurface {
    fn state(&self) -> SurfaceState {
        SurfaceState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_size_grows_with_span_and_stays_bounded() {
        assert_eq!(block_for_span(64 << 20), MIN_BLOCK_BYTES);
        assert_eq!(block_for_span(1 << 30), MIN_BLOCK_BYTES);
        assert_eq!(block_for_span(512 << 30), 32 << 20);
        assert_eq!(block_for_span(2 << 40), 128 << 20);
        assert_eq!(block_for_span(u64::MAX), MAX_BLOCK_BYTES);
    }

    #[test]
    fn clamped_params_cover_full_blocks_only() {
        let params = SurfaceParams::new(4, 1 << 30).clamped_to(300 << 20);
        assert_eq!(params.span_bytes, 300 << 20);
        assert_eq!(params.block_bytes, MIN_BLOCK_BYTES);

        // Zakres mniejszy niż blok nie daje ani jednej próbki — wywołujący
        // musi to zgłosić, a nie czytać „prawie cały” dysk.
        let tiny = SurfaceParams::new(0, 1 << 30).clamped_to(512);
        assert_eq!(tiny.span_bytes, 0);
    }

    #[test]
    fn clamped_params_reject_unreasonable_block_sizes() {
        let params = SurfaceParams {
            drive: 1,
            span_bytes: 1 << 30,
            block_bytes: 3,
            random_reads: 0,
            time_limit_ms: 0,
        }
        .clamped_to(1 << 40);
        assert_eq!(params.block_bytes, MIN_BLOCK_BYTES);

        let params = SurfaceParams {
            drive: 1,
            span_bytes: 8 << 40,
            block_bytes: u64::MAX,
            random_reads: 0,
            time_limit_ms: 0,
        }
        .clamped_to(8 << 40);
        assert_eq!(params.block_bytes, MAX_BLOCK_BYTES);
    }

    #[test]
    fn new_params_carry_the_default_time_limit_and_clamping_keeps_it() {
        let params = SurfaceParams::new(3, 4 << 30);
        assert_eq!(params.time_limit_ms, DEFAULT_TIME_LIMIT_MS);
        assert_eq!(
            params.clamped_to(1 << 30).time_limit_ms,
            DEFAULT_TIME_LIMIT_MS
        );

        let unlimited = SurfaceParams::new(3, 4 << 30).with_time_limit_ms(0);
        assert_eq!(unlimited.time_limit_ms, 0);
        assert_eq!(unlimited.clamped_to(2 << 30).time_limit_ms, 0);
    }

    #[test]
    fn progress_follows_the_clock_when_a_limit_is_set() {
        // Limit wyznacza koniec testu, więc 15 s z 60 s to 25 % — niezależnie
        // od tego, jak mały fragment powierzchni zdążył odczytać.
        let state = SurfaceState::Running {
            params: SurfaceParams::new(0, 1 << 30),
            disk_bytes: 1 << 30,
            bytes_read: 5 << 20,
            elapsed_ms: 15_000,
            samples: Vec::new(),
        };
        assert_eq!(state.progress_percent(), Some(25.0));
    }

    #[test]
    fn progress_without_a_limit_is_the_read_share_of_the_span() {
        let state = SurfaceState::Running {
            params: SurfaceParams::new(0, 1000).with_time_limit_ms(0),
            disk_bytes: 2000,
            bytes_read: 250,
            elapsed_ms: 9_999,
            samples: Vec::new(),
        };
        assert_eq!(state.progress_percent(), Some(25.0));

        // Anulowany przebieg raportuje pokrycie, a nie 100 %.
        let cancelled = SurfaceScan {
            params: SurfaceParams::new(0, 1000).with_time_limit_ms(0),
            disk_bytes: 2000,
            bytes_read: 300,
            duration_ms: 10,
            samples: Vec::new(),
            errors: Vec::new(),
            access: None,
            cancelled: true,
            time_limit_reached: false,
        };
        assert_eq!(SurfaceState::Done(cancelled).progress_percent(), Some(30.0));

        // Zakończony limitem czasu też jest przebiegiem skończonym.
        let limited = SurfaceScan {
            params: SurfaceParams::new(0, 1000),
            disk_bytes: 2000,
            bytes_read: 30,
            duration_ms: 60_000,
            samples: Vec::new(),
            errors: Vec::new(),
            access: None,
            cancelled: false,
            time_limit_reached: true,
        };
        assert_eq!(SurfaceState::Done(limited).progress_percent(), Some(100.0));
        assert_eq!(SurfaceState::Idle.progress_percent(), None);
    }

    #[test]
    fn covered_bytes_ends_at_the_last_sample_block() {
        let samples = vec![
            SurfaceSample {
                offset_bytes: 0,
                speed_mbps: 500.0,
            },
            SurfaceSample {
                offset_bytes: 2 << 20,
                speed_mbps: 400.0,
            },
        ];
        assert_eq!(covered_bytes(&samples, 1 << 20), 3 << 20);
        assert_eq!(covered_bytes(&[], 1 << 20), 0);
        // Ogromny offset nie może przepełnić licznika.
        let huge = vec![SurfaceSample {
            offset_bytes: u64::MAX,
            speed_mbps: 1.0,
        }];
        assert_eq!(covered_bytes(&huge, 1 << 20), u64::MAX);
    }

    #[test]
    fn store_roundtrips_state() {
        let store = SurfaceStore::new();
        assert_eq!(store.get(), SurfaceState::Idle);

        let params = SurfaceParams::new(2, 1 << 20);
        store.set(SurfaceState::Running {
            params,
            disk_bytes: 1 << 20,
            bytes_read: 0,
            elapsed_ms: 0,
            samples: vec![SurfaceSample {
                offset_bytes: 0,
                speed_mbps: 123.4,
            }],
        });
        match store.state() {
            SurfaceState::Running { samples, .. } => {
                assert_eq!(samples.len(), 1);
                assert_eq!(samples[0].speed_mbps, 123.4);
            }
            other => panic!("oczekiwano Running, jest {other:?}"),
        }

        store.set(SurfaceState::Failed {
            params: Some(params),
            message: "brak dostępu".to_string(),
        });
        assert!(matches!(store.get(), SurfaceState::Failed { .. }));
        assert_eq!(IdleSurface.state(), SurfaceState::Idle);
    }

    #[test]
    fn params_and_scan_roundtrip_through_json() {
        let scan = SurfaceScan {
            params: SurfaceParams::new(1, 2 << 20).with_time_limit_ms(30_000),
            disk_bytes: 2 << 20,
            bytes_read: 1 << 20,
            duration_ms: 42,
            samples: vec![SurfaceSample {
                offset_bytes: 0,
                speed_mbps: 500.5,
            }],
            errors: vec![SurfaceReadError {
                offset_bytes: 1 << 20,
                detail: "błąd we/wy".to_string(),
            }],
            access: Some(SurfaceAccess {
                reads: 10,
                avg_ms: 0.1,
                min_ms: 0.05,
                max_ms: 0.2,
                iops: 10_000.0,
            }),
            cancelled: false,
            time_limit_reached: true,
        };
        let json = serde_json::to_string(&scan).expect("serializacja");
        let back: SurfaceScan = serde_json::from_str(&json).expect("deserializacja");
        assert_eq!(scan, back);
    }

    #[test]
    fn json_without_the_time_limit_fields_still_parses() {
        // Raport zapisany przed dodaniem limitu czasu nie może przestać się
        // czytać; brak pól oznacza przebieg bez limitu.
        let params: SurfaceParams = serde_json::from_str(
            r#"{"drive":0,"span_bytes":1048576,"block_bytes":1048576,"random_reads":0}"#,
        )
        .expect("stary format parametrów");
        assert_eq!(params.time_limit_ms, 0);

        let scan: SurfaceScan = serde_json::from_str(
            r#"{"params":{"drive":0,"span_bytes":1048576,"block_bytes":1048576,"random_reads":0},
                "disk_bytes":1048576,"bytes_read":1048576,"duration_ms":5,
                "samples":[],"errors":[],"access":null,"cancelled":false}"#,
        )
        .expect("stary format przebiegu");
        assert!(!scan.time_limit_reached);
    }
}
