//! Telemetria w czasie: pojedyncza próbka i bufor pierścieniowy.
//!
//! Moduł `system.live` nie zbiera historii sam — dostaje ją przez
//! [`crate::providers::LiveSource`]. Historię zapełnia wątek roboczy aplikacji,
//! dzięki czemu moduł pozostaje bezstanowy i testowalny.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Jedna próbka telemetrii. Wszystkie pola poza czasem są opcjonalne, bo część
/// danych bywa niedostępna (np. brak licznika wydajności na maszynie wirtualnej).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LiveSample {
    /// Sekundy od uruchomienia zbierania (0 = pierwsza próbka).
    pub elapsed_secs: f64,
    /// Obciążenie CPU w procentach (0–100).
    pub cpu_load_percent: Option<f64>,
    /// Bieżące taktowanie CPU w MHz.
    pub cpu_frequency_mhz: Option<f64>,
    /// Zużycie pamięci RAM w procentach (0–100).
    pub ram_used_percent: Option<f64>,
}

impl LiveSample {
    pub fn new(elapsed_secs: f64) -> Self {
        Self {
            elapsed_secs,
            cpu_load_percent: None,
            cpu_frequency_mhz: None,
            ram_used_percent: None,
        }
    }

    pub fn with_cpu_load(mut self, percent: f64) -> Self {
        self.cpu_load_percent = Some(percent);
        self
    }

    pub fn with_cpu_frequency(mut self, mhz: f64) -> Self {
        self.cpu_frequency_mhz = Some(mhz);
        self
    }

    pub fn with_ram_used(mut self, percent: f64) -> Self {
        self.ram_used_percent = Some(percent);
        self
    }
}

/// Bufor pierścieniowy próbek. Nowa próbka wypycha najstarszą.
#[derive(Debug, Clone)]
pub struct LiveHistory {
    capacity: usize,
    samples: VecDeque<LiveSample>,
}

impl LiveHistory {
    /// `capacity` mniejsze od 1 jest traktowane jak 1 — bufor nigdy nie jest pusty z definicji.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::new(),
        }
    }

    pub fn push(&mut self, sample: LiveSample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub fn samples(&self) -> &VecDeque<LiveSample> {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

impl crate::providers::LiveSource for LiveHistory {
    fn samples(&self) -> Vec<LiveSample> {
        self.samples.iter().copied().collect()
    }
}

/// Procent `part` z `total`, odporny na dzielenie przez zero i wartości spoza zakresu.
pub fn percent_of(part: f64, total: f64) -> Option<f64> {
    if !(total.is_finite() && total > 0.0) || !part.is_finite() {
        return None;
    }
    Some((part / total * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_evicts_oldest_sample() {
        let mut history = LiveHistory::new(3);
        for index in 0..5 {
            history.push(LiveSample::new(index as f64));
        }
        assert_eq!(history.len(), 3);
        let elapsed: Vec<f64> = history.samples().iter().map(|s| s.elapsed_secs).collect();
        assert_eq!(elapsed, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn history_never_has_zero_capacity() {
        let mut history = LiveHistory::new(0);
        history.push(LiveSample::new(0.0));
        history.push(LiveSample::new(1.0));
        assert_eq!(history.capacity(), 1);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn live_source_returns_samples_in_order() {
        let mut history = LiveHistory::new(4);
        history.push(LiveSample::new(0.0).with_cpu_load(10.0));
        history.push(LiveSample::new(1.0).with_cpu_load(20.0));
        let samples = history.samples();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1].cpu_load_percent, Some(20.0));
    }

    #[test]
    fn percent_of_guards_division_by_zero() {
        assert_eq!(percent_of(50.0, 100.0), Some(50.0));
        assert_eq!(percent_of(0.0, 0.0), None);
        assert_eq!(percent_of(1.0, f64::NAN), None);
        assert_eq!(percent_of(-5.0, 100.0), Some(0.0));
        assert_eq!(percent_of(200.0, 100.0), Some(100.0));
    }

    #[test]
    fn clearing_history_empties_it() {
        let mut history = LiveHistory::new(2);
        history.push(LiveSample::new(0.0));
        history.clear();
        assert!(history.is_empty());
    }
}
