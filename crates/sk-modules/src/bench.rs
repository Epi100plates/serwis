//! Analiza mapy prędkości powierzchni (`storage.bench`).
//!
//! Funkcje są czyste: dostają gotowy [`SurfaceScan`] i zwracają liczby oraz
//! punkty wykresu. Dzięki temu każdy przypadek (przerwany test, błędy odczytu,
//! nierówna prędkość na HDD) testuje się bez dysku i bez administratora.

use sk_core::surface::{SurfaceSample, SurfaceScan};

/// Ile punktów trafia do wykresu. Pomiary mogą mieć 16k próbek, a ekran i tak
/// nie pokaże więcej — średnia z grupy zachowuje kształt krzywej.
pub const MAX_CHART_POINTS: usize = 1_000;

/// Jednostka osi X wykresu: 1 GB = 1024³ B, tak samo jak etykiety rozmiarów
/// w widoku. Dzięki temu oś pokazuje dokładnie tyle, ile „Rozmiar dysku”.
pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Pozycja na dysku w GB (jednostka osi X).
pub fn gigabytes(bytes: u64) -> f64 {
    bytes as f64 / GIB
}

/// Podsumowanie liczbowe mapy prędkości.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BenchStats {
    pub bytes_read: u64,
    pub duration_ms: u64,
    pub min_mbps: f64,
    pub avg_mbps: f64,
    pub max_mbps: f64,
    pub sample_count: usize,
}

/// Statystyki z próbek. `None`, gdy nie ma ani jednej dodatniej, skończonej
/// prędkości — wtedy nie ma czego pokazywać i nie zgadujemy.
pub fn stats(scan: &SurfaceScan) -> Option<BenchStats> {
    let speeds = valid_speeds(&scan.samples);
    if speeds.is_empty() {
        return None;
    }
    let min_mbps = speeds.iter().copied().fold(f64::INFINITY, f64::min);
    let max_mbps = speeds.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let avg_mbps = speeds.iter().sum::<f64>() / speeds.len() as f64;
    Some(BenchStats {
        bytes_read: scan.bytes_read,
        duration_ms: scan.duration_ms,
        min_mbps,
        avg_mbps,
        max_mbps,
        sample_count: speeds.len(),
    })
}

fn valid_speeds(samples: &[SurfaceSample]) -> Vec<f64> {
    samples
        .iter()
        .map(|sample| sample.speed_mbps)
        .filter(|speed| speed.is_finite() && *speed > 0.0)
        .collect()
}

/// Punkty wykresu: oś X to pozycja na dysku w GB (0 = początek powierzchni),
/// oś Y to prędkość w MB/s. Próbki są uśredniane grupowo, żeby wykres miał
/// najwyżej [`MAX_CHART_POINTS`] punktów.
///
/// Pozycja jest bezwzględna, a nie procentowa: przebieg próbkuje cały dysk, a
/// zakres osi ustala moduł (`Chart::with_x_range`) z rozmiaru nośnika, więc
/// wykres od pierwszej próbki rysuje się w gotowym układzie.
pub fn chart_points(samples: &[SurfaceSample]) -> Vec<[f64; 2]> {
    let valid: Vec<SurfaceSample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.speed_mbps.is_finite() && sample.speed_mbps > 0.0)
        .collect();
    if valid.is_empty() {
        return Vec::new();
    }

    let group = valid.len().div_ceil(MAX_CHART_POINTS).max(1);
    valid
        .chunks(group)
        .map(|chunk| {
            let count = chunk.len() as f64;
            let x = chunk
                .iter()
                .map(|sample| gigabytes(sample.offset_bytes))
                .sum::<f64>()
                / count;
            let y = chunk.iter().map(|sample| sample.speed_mbps).sum::<f64>() / count;
            [x, y]
        })
        .collect()
}

/// Jak zakończył się przebieg — na tej podstawie moduł dobiera stan i notkę.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchOutcome {
    /// Odczytano cały zaplanowany zakres, bez błędów.
    Complete,
    /// Użytkownik przerwał test — dane są częściowe.
    Cancelled,
    /// Co najmniej jeden blok nie dał się odczytać.
    ReadErrors,
    /// Test zatrzymał się przed końcem bez błędów i bez anulowania (np. dysk
    /// zgłosił krótszy rozmiar niż zakładano).
    Incomplete,
    /// Przebieg zatrzymał się na limicie czasu — mapa obejmuje tylko początek
    /// zakresu, ale sam test wykonał się w całości.
    TimeLimit,
}

pub fn outcome(scan: &SurfaceScan) -> BenchOutcome {
    if scan.cancelled {
        BenchOutcome::Cancelled
    } else if !scan.errors.is_empty() {
        BenchOutcome::ReadErrors
    } else if scan.time_limit_reached && !scan.reached_end() {
        // Limit urwał koniec mapy. Jeśli plan zdążył dobić do końca powierzchni,
        // przebieg jest kompletny — po prostu rzadziej próbkowany.
        BenchOutcome::TimeLimit
    } else if !scan.reached_end() {
        BenchOutcome::Incomplete
    } else {
        BenchOutcome::Complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::surface::{SurfaceAccess, SurfaceParams, SurfaceReadError};

    fn scan(samples: Vec<SurfaceSample>, span_bytes: u64) -> SurfaceScan {
        let bytes_read = samples
            .iter()
            .map(|sample| sample.offset_bytes)
            .max()
            .map_or(0, |offset| offset + (1 << 20))
            .min(span_bytes);
        SurfaceScan {
            params: SurfaceParams {
                drive: 0,
                span_bytes,
                block_bytes: 1 << 20,
                random_reads: 0,
                time_limit_ms: 60_000,
            },
            disk_bytes: span_bytes,
            bytes_read,
            duration_ms: 1000,
            samples,
            errors: Vec::new(),
            access: None,
            cancelled: false,
            time_limit_reached: false,
        }
    }

    fn sample(offset_mib: u64, speed: f64) -> SurfaceSample {
        SurfaceSample {
            offset_bytes: offset_mib << 20,
            speed_mbps: speed,
        }
    }

    #[test]
    fn stats_skip_invalid_speeds_and_compute_extremes() {
        let scan = scan(
            vec![
                sample(0, 500.0),
                sample(1, f64::NAN),
                sample(2, 0.0),
                sample(3, 250.0),
            ],
            4 << 20,
        );
        let stats = stats(&scan).expect("statystyki");
        assert_eq!(stats.sample_count, 2);
        assert_eq!(stats.min_mbps, 250.0);
        assert_eq!(stats.max_mbps, 500.0);
        assert_eq!(stats.avg_mbps, 375.0);
    }

    #[test]
    fn empty_samples_have_no_stats() {
        assert!(stats(&scan(Vec::new(), 1 << 20)).is_none());
    }

    #[test]
    fn chart_x_is_the_position_in_gigabytes() {
        // 1 GiB = 1024 MiB pozycji → 1.0 GB na osi X.
        let scan = scan(vec![sample(0, 100.0), sample(1024, 200.0)], 4 << 30);
        let points = chart_points(&scan.samples);
        assert_eq!(points, vec![[0.0, 100.0], [1.0, 200.0]]);
    }

    #[test]
    fn chart_is_downsampled_without_losing_edges() {
        let samples: Vec<SurfaceSample> = (0..5000)
            .map(|index| sample(index, 100.0 + index as f64 * 0.1))
            .collect();
        let points = chart_points(&samples);
        assert!(
            points.len() <= MAX_CHART_POINTS,
            "punktów: {}",
            points.len()
        );
        assert!(points.len() > 900, "zbyt agresywne uśrednianie");
        // 5000 próbek po 1 MiB to ~4.88 GB — skrajne punkty muszą zostać.
        assert!(points.first().expect("pierwszy punkt")[0] < 0.05);
        assert!(points.last().expect("ostatni punkt")[0] > 4.8);
        // Średnia z grupy rośnie monotonicznie dla liniowo rosnącej prędkości.
        assert!(points[1][1] > points[0][1]);
    }

    #[test]
    fn empty_samples_give_no_chart() {
        assert!(chart_points(&[]).is_empty());
        // Próbka bez sensownej prędkości też nie rysuje punktu.
        assert!(chart_points(&[sample(0, 0.0)]).is_empty());
    }

    #[test]
    fn outcome_distinguishes_cancel_errors_and_completion() {
        let mut complete = scan(vec![sample(0, 100.0)], 1 << 20);
        assert_eq!(outcome(&complete), BenchOutcome::Complete);

        complete.cancelled = true;
        assert_eq!(outcome(&complete), BenchOutcome::Cancelled);

        let mut broken = scan(vec![sample(0, 100.0)], 2 << 20);
        broken.errors.push(SurfaceReadError {
            offset_bytes: 1 << 20,
            detail: "błąd we/wy".to_string(),
        });
        assert_eq!(outcome(&broken), BenchOutcome::ReadErrors);

        let partial = scan(vec![sample(0, 100.0)], 2 << 20);
        assert_eq!(outcome(&partial), BenchOutcome::Incomplete);
    }

    #[test]
    fn time_limit_is_its_own_outcome_and_beats_incomplete() {
        let mut limited = scan(vec![sample(0, 100.0)], 8 << 20);
        limited.time_limit_reached = true;
        assert_eq!(outcome(&limited), BenchOutcome::TimeLimit);

        // Błędy odczytu są ważniejsze niż skrócony zakres — to sygnał zdrowia.
        limited.errors.push(SurfaceReadError {
            offset_bytes: 0,
            detail: "błąd we/wy".to_string(),
        });
        assert_eq!(outcome(&limited), BenchOutcome::ReadErrors);

        // Anulowanie przez użytkownika też ma pierwszeństwo.
        limited.cancelled = true;
        assert_eq!(outcome(&limited), BenchOutcome::Cancelled);
    }

    #[test]
    fn compressed_plan_that_still_reaches_the_end_is_complete() {
        // Dysk zwolnił, plan skrócił się do skoku na koniec — mapa obejmuje
        // całą powierzchnię, więc to kompletny przebieg, a nie urwany.
        let mut scan = scan(vec![sample(0, 500.0), sample(7, 480.0)], 8 << 20);
        scan.time_limit_reached = true;
        assert_eq!(outcome(&scan), BenchOutcome::Complete);
    }

    #[test]
    fn access_time_is_kept_in_scan() {
        let mut scan = scan(vec![sample(0, 100.0)], 1 << 20);
        scan.access = Some(SurfaceAccess {
            reads: 20,
            avg_ms: 0.08,
            min_ms: 0.05,
            max_ms: 0.2,
            iops: 12_500.0,
        });
        let access = scan.access.expect("czas dostępu");
        assert!(access.iops > 0.0);
    }
}
