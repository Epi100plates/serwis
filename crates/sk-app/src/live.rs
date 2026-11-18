//! Próbkowanie telemetrii dla modułu `system.live`.
//!
//! Próbka jest zbierana w wątku roboczym i dopisywana do [`LiveHistory`].
//! Pojedynczy błąd licznika nie przerywa próbkowania — w próbce zostaje `None`,
//! a szczegół trafia do logu diagnostycznego.

use serde::Deserialize;
use sk_core::live::{LiveHistory, LiveSample, percent_of};
use sk_core::providers::{NS_CIMV2, NS_WMI, WmiProvider};

const PROCESSOR_QUERY: &str =
    "SELECT Name, PercentProcessorTime FROM Win32_PerfFormattedData_PerfOS_Processor";
const PERFORMANCE_QUERY: &str = "SELECT Active, frequency FROM ProcessorPerformance";
const MEMORY_QUERY: &str = "SELECT FreePhysicalMemory, TotalVisibleMemorySize \
     FROM Win32_OperatingSystem";

#[derive(Debug, Deserialize)]
struct ProcessorCounter {
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "PercentProcessorTime", default)]
    percent: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Performance {
    #[serde(rename = "Active", default)]
    active: Option<bool>,
    #[serde(rename = "frequency", default)]
    frequency_mhz: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Memory {
    #[serde(rename = "FreePhysicalMemory", default)]
    free_kib: Option<u64>,
    #[serde(rename = "TotalVisibleMemorySize", default)]
    total_kib: Option<u64>,
}

/// Zbiera jedną próbkę, dopisuje ją do historii i zwraca.
pub fn sample(wmi: &dyn WmiProvider, history: &mut LiveHistory, elapsed_secs: f64) -> LiveSample {
    let mut sample = LiveSample::new(elapsed_secs);

    match rows::<ProcessorCounter>(
        wmi,
        NS_CIMV2,
        PROCESSOR_QUERY,
        "Win32_PerfFormattedData_PerfOS_Processor",
    ) {
        Ok(counters) => {
            sample.cpu_load_percent = counters
                .iter()
                .find(|counter| counter.name.as_deref() == Some("_Total"))
                .and_then(|counter| counter.percent)
                .map(|percent| percent.min(100) as f64);
        }
        Err(message) => tracing::debug!(message, "brak licznika obciążenia CPU"),
    }

    match rows::<Performance>(wmi, NS_WMI, PERFORMANCE_QUERY, "ProcessorPerformance") {
        Ok(entries) => {
            sample.cpu_frequency_mhz = entries
                .iter()
                .filter(|entry| entry.active.unwrap_or(true))
                .filter_map(|entry| entry.frequency_mhz)
                .max()
                .map(f64::from);
        }
        Err(message) => tracing::debug!(message, "brak licznika taktowania CPU"),
    }

    match rows::<Memory>(wmi, NS_CIMV2, MEMORY_QUERY, "Win32_OperatingSystem") {
        Ok(entries) => {
            if let Some(memory) = entries.first()
                && let (Some(free), Some(total)) = (memory.free_kib, memory.total_kib)
            {
                sample.ram_used_percent =
                    percent_of((total - free.min(total)) as f64, total as f64);
            }
        }
        Err(message) => tracing::debug!(message, "brak danych o pamięci"),
    }

    history.push(sample);
    sample
}

fn rows<T: serde::de::DeserializeOwned>(
    wmi: &dyn WmiProvider,
    namespace: &str,
    query: &str,
    class: &str,
) -> Result<Vec<T>, String> {
    let rows = wmi
        .query(namespace, query)
        .map_err(|err| format!("{class}: {err}"))?;
    serde_json::from_value(serde_json::Value::Array(rows))
        .map_err(|err| format!("{class}: nieoczekiwany format odpowiedzi: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockWmi};

    fn wmi() -> MockWmi {
        MockWmi::new()
            .with_rows(
                "Win32_PerfFormattedData_PerfOS_Processor",
                crate::tests_support::fixture_rows("win32_perfformatteddata_perfos_processor.json"),
            )
            .with_rows(
                "ProcessorPerformance",
                crate::tests_support::fixture_rows("wmi_processorperformance.json"),
            )
            .with_rows(
                "Win32_OperatingSystem",
                crate::tests_support::fixture_rows("win32_operatingsystem.json"),
            )
    }

    #[test]
    fn builds_sample_from_real_counters() {
        let harness = Harness::new().with_wmi(wmi());
        let mut history = LiveHistory::new(10);
        let sample = sample(&harness.wmi, &mut history, 1.0);

        assert_eq!(sample.elapsed_secs, 1.0);

        // Wartości oczekiwane liczymy z tych samych fixtures, więc test nie
        // zależy od chwilowego obciążenia maszyny w momencie ich zbierania.
        let counters =
            crate::tests_support::fixture_rows("win32_perfformatteddata_perfos_processor.json");
        let total = counters
            .iter()
            .find(|row| row["Name"] == "_Total")
            .and_then(|row| row["PercentProcessorTime"].as_u64())
            .expect("wiersz _Total w fixture");
        assert_eq!(sample.cpu_load_percent, Some(total as f64));

        let performance = crate::tests_support::fixture_rows("wmi_processorperformance.json");
        let expected_mhz = performance
            .iter()
            .filter_map(|row| row["frequency"].as_u64())
            .max()
            .expect("taktowanie w fixture");
        assert_eq!(sample.cpu_frequency_mhz, Some(expected_mhz as f64));

        let memory = crate::tests_support::fixture_rows("win32_operatingsystem.json");
        let free = memory[0]["FreePhysicalMemory"]
            .as_f64()
            .expect("wolna pamięć");
        let total_kib = memory[0]["TotalVisibleMemorySize"]
            .as_f64()
            .expect("pamięć całkowita");
        let expected_ram = (total_kib - free) / total_kib * 100.0;
        let ram = sample.ram_used_percent.expect("zużycie RAM");
        assert!((ram - expected_ram).abs() < 0.01, "zużycie RAM {ram}");

        assert_eq!(history.len(), 1);
    }

    #[test]
    fn missing_counters_produce_empty_sample_without_panicking() {
        let harness = Harness::new();
        let mut history = LiveHistory::new(4);
        let sample = sample(&harness.wmi, &mut history, 0.0);
        assert_eq!(sample.cpu_load_percent, None);
        assert_eq!(sample.cpu_frequency_mhz, None);
        assert_eq!(sample.ram_used_percent, None);
        assert_eq!(history.len(), 1);
    }
}
