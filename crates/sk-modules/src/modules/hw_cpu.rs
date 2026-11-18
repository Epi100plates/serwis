//! `hw.cpu` — inwentarz procesora i bieżące taktowanie.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{bool_row, decode_first, format_kib, number, text};

const PROCESSOR_QUERY: &str = "SELECT Caption, Name, Manufacturer, SocketDesignation, \
     NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed, CurrentClockSpeed, ExtClock, \
     L2CacheSize, L3CacheSize, AddressWidth, DataWidth, Architecture, ProcessorId, \
     VirtualizationFirmwareEnabled, VMMonitorModeExtensions, \
     SecondLevelAddressTranslationExtensions, LoadPercentage FROM Win32_Processor";
const PERFORMANCE_QUERY: &str = queries::PROCESSOR_PERFORMANCE;

#[derive(Debug, Default, Deserialize)]
struct Processor {
    #[serde(rename = "Caption", default)]
    caption: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "Manufacturer", default)]
    manufacturer: Option<String>,
    #[serde(rename = "SocketDesignation", default)]
    socket: Option<String>,
    #[serde(rename = "NumberOfCores", default)]
    cores: Option<u32>,
    #[serde(rename = "NumberOfLogicalProcessors", default)]
    threads: Option<u32>,
    #[serde(rename = "MaxClockSpeed", default)]
    max_clock_mhz: Option<u32>,
    #[serde(rename = "CurrentClockSpeed", default)]
    current_clock_mhz: Option<u32>,
    #[serde(rename = "ExtClock", default)]
    bus_clock_mhz: Option<u32>,
    #[serde(rename = "L2CacheSize", default)]
    l2_cache_kib: Option<u32>,
    #[serde(rename = "L3CacheSize", default)]
    l3_cache_kib: Option<u32>,
    #[serde(rename = "AddressWidth", default)]
    address_width: Option<u16>,
    #[serde(rename = "DataWidth", default)]
    data_width: Option<u16>,
    #[serde(rename = "Architecture", default)]
    architecture: Option<u16>,
    #[serde(rename = "ProcessorId", default)]
    processor_id: Option<String>,
    #[serde(rename = "VirtualizationFirmwareEnabled", default)]
    virtualization_firmware: Option<bool>,
    #[serde(rename = "VMMonitorModeExtensions", default)]
    vm_monitor_extensions: Option<bool>,
    #[serde(rename = "SecondLevelAddressTranslationExtensions", default)]
    slat: Option<bool>,
    #[serde(rename = "LoadPercentage", default)]
    load_percent: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct Performance {
    #[serde(rename = "Active", default)]
    active: Option<bool>,
    #[serde(rename = "frequency", default)]
    frequency_mhz: Option<u32>,
    #[serde(rename = "percentage", default)]
    percent_frequency: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Cpu;

impl Module for Cpu {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.cpu",
            name_key: "module.hw_cpu.name",
            desc_key: "module.hw_cpu.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let cpu: Processor = decode_first(
            &ctx.wmi.query(NS_CIMV2, PROCESSOR_QUERY)?,
            "Win32_Processor",
        )?;

        // Licznik wydajności bywa niedostępny (np. w maszynie wirtualnej) —
        // wtedy pokazujemy taktowanie nominalne z WMI i mówimy o tym wprost,
        // zamiast po cichu pokazywać nieaktualną wartość.
        let performance_result = ctx
            .wmi
            .query(NS_WMI, PERFORMANCE_QUERY)
            .and_then(|rows| crate::util::decode_all::<Performance>(&rows, "ProcessorPerformance"));
        let live_available = performance_result.is_ok();
        let performance = performance_result.unwrap_or_default();

        let live_frequency = performance
            .iter()
            .filter(|row| row.active.unwrap_or(true))
            .filter_map(|row| row.frequency_mhz)
            .max();
        let live_percent = performance
            .iter()
            .filter(|row| row.active.unwrap_or(true))
            .filter_map(|row| row.percent_frequency)
            .max();

        let rows = vec![
            KeyValue::new("module.hw_cpu.model", text(cpu.name.as_deref())),
            KeyValue::new(
                "module.hw_cpu.vendor",
                vendor_name(cpu.manufacturer.as_deref()),
            ),
            KeyValue::new("module.hw_cpu.caption", text(cpu.caption.as_deref())),
            KeyValue::new("module.hw_cpu.socket", text(cpu.socket.as_deref())),
            KeyValue::new("module.hw_cpu.cores", number(cpu.cores)),
            KeyValue::new("module.hw_cpu.threads", number(cpu.threads)),
            KeyValue::new("module.hw_cpu.max_clock", format_mhz(cpu.max_clock_mhz)),
            KeyValue::new(
                "module.hw_cpu.current_clock",
                live_frequency
                    .or(cpu.current_clock_mhz)
                    .map_or_else(|| "-".to_string(), |mhz| format!("{mhz} MHz")),
            ),
            KeyValue::new(
                "module.hw_cpu.frequency_percent",
                live_percent.map_or_else(|| "-".to_string(), |percent| format!("{percent} %")),
            ),
            KeyValue::new("module.hw_cpu.bus_clock", format_mhz(cpu.bus_clock_mhz)),
            KeyValue::new(
                "module.hw_cpu.l2",
                format_kib(cpu.l2_cache_kib.unwrap_or(0)),
            ),
            KeyValue::new(
                "module.hw_cpu.l3",
                format_kib(cpu.l3_cache_kib.unwrap_or(0)),
            ),
            KeyValue::new(
                "module.hw_cpu.architecture",
                architecture_name(cpu.architecture, cpu.address_width),
            ),
            KeyValue::new(
                "module.hw_cpu.widths",
                widths(cpu.address_width, cpu.data_width),
            ),
            KeyValue::new(
                "module.hw_cpu.processor_id",
                text(cpu.processor_id.as_deref()),
            ),
            bool_row("module.hw_cpu.virtualization", cpu.virtualization_firmware),
            bool_row("module.hw_cpu.slat", cpu.slat),
            bool_row("module.hw_cpu.vm_extensions", cpu.vm_monitor_extensions),
            KeyValue::new(
                "module.hw_cpu.load",
                cpu.load_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent} %")),
            ),
        ];

        let mut blocks = vec![Block::key_values(rows)];
        if !live_available {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_cpu.note.no_performance",
            ));
        }
        blocks.push(Block::note(
            NoteLevel::Info,
            "module.hw_cpu.note.temperature",
        ));

        let view = View::new(vec![
            Section::new("module.hw_cpu.section.cpu").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "name": crate::util::trimmed(cpu.name),
            "caption": crate::util::trimmed(cpu.caption),
            "manufacturer": crate::util::trimmed(cpu.manufacturer),
            "socket": crate::util::trimmed(cpu.socket),
            "cores": cpu.cores,
            "threads": cpu.threads,
            "max_clock_mhz": cpu.max_clock_mhz,
            "rated_clock_mhz": cpu.current_clock_mhz,
            "live_clock_mhz": live_frequency,
            "live_percent_of_rated": live_percent,
            "bus_clock_mhz": cpu.bus_clock_mhz,
            "l2_cache_kib": cpu.l2_cache_kib,
            "l3_cache_kib": cpu.l3_cache_kib,
            "address_width_bits": cpu.address_width,
            "data_width_bits": cpu.data_width,
            "architecture": cpu.architecture,
            "processor_id": crate::util::trimmed(cpu.processor_id),
            "virtualization_firmware_enabled": cpu.virtualization_firmware,
            "slat": cpu.slat,
            "vm_monitor_extensions": cpu.vm_monitor_extensions,
            "load_percent": cpu.load_percent,
        });

        Ok(ModuleOutput { data, view })
    }
}

/// `AuthenticAMD`/`GenuineIntel` to identyfikatory CPUID, nie nazwy producentów.
fn vendor_name(vendor: Option<&str>) -> String {
    match vendor.map(str::trim) {
        Some("AuthenticAMD") => "AMD".to_string(),
        Some("GenuineIntel") => "Intel".to_string(),
        Some("HygonGenuine") => "Hygon".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "-".to_string(),
    }
}

/// Kody `Win32_Processor.Architecture`.
fn architecture_name(architecture: Option<u16>, address_width: Option<u16>) -> String {
    match architecture {
        Some(0) => "x86".to_string(),
        Some(5) => "ARM".to_string(),
        Some(6) => "Itanium".to_string(),
        Some(9) => "x64".to_string(),
        Some(12) => "ARM64".to_string(),
        _ => address_width.map_or_else(|| "-".to_string(), |bits| format!("{bits}-bit")),
    }
}

fn widths(address: Option<u16>, data: Option<u16>) -> String {
    match (address, data) {
        (Some(address), Some(data)) => format!("{address} / {data} bit"),
        (Some(address), None) => format!("{address} bit"),
        _ => "-".to_string(),
    }
}

fn format_mhz(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_string(), |mhz| format!("{mhz} MHz"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::boolean_key;
    use sk_core::i18n::Translator;
    use sk_core::testing::{Harness, MockWmi};

    fn harness() -> Harness {
        let wmi = MockWmi::new()
            .with_rows("Win32_Processor", fixture_rows!("win32_processor.json"))
            .with_rows(
                "ProcessorPerformance",
                fixture_rows!("wmi_processorperformance.json"),
            );
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn renders_cpu_inventory_from_real_fixture() {
        let output = Cpu.collect(&harness().ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_cpu.model"));
        assert!(output.view.uses_key("module.hw_cpu.cores"));
        assert_eq!(output.data["cores"], 6);
        assert_eq!(output.data["threads"], 12);
        assert_eq!(output.data["socket"], "AM4");
        assert_eq!(output.data["manufacturer"], "AuthenticAMD");
        assert_eq!(output.data["l2_cache_kib"], 3072);
        assert_eq!(output.data["l3_cache_kib"], 32768);
        assert_eq!(output.data["architecture"], 9);
    }

    #[test]
    fn uses_live_frequency_from_performance_counters() {
        let output = Cpu.collect(&harness().ctx()).expect("zbieranie danych");
        assert_eq!(output.data["live_clock_mhz"], 3501);
        assert_eq!(output.data["live_percent_of_rated"], 100);
    }

    #[test]
    fn works_without_performance_counters() {
        let wmi =
            MockWmi::new().with_rows("Win32_Processor", fixture_rows!("win32_processor.json"));
        let harness = Harness::new().with_wmi(wmi);
        let output = Cpu.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.data["live_clock_mhz"].is_null());
        assert!(output.view.uses_key("module.hw_cpu.current_clock"));
    }

    #[test]
    fn maps_vendor_and_architecture_codes() {
        assert_eq!(vendor_name(Some("AuthenticAMD")), "AMD");
        assert_eq!(vendor_name(Some("GenuineIntel")), "Intel");
        assert_eq!(vendor_name(None), "-");
        assert_eq!(architecture_name(Some(9), Some(64)), "x64");
        assert_eq!(architecture_name(Some(12), Some(64)), "ARM64");
        assert_eq!(architecture_name(None, Some(32)), "32-bit");
        assert_eq!(architecture_name(None, None), "-");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = Cpu.collect(&harness().ctx()).expect("zbieranie danych");
        let translator = Translator::load().expect("tłumaczenia");
        for key in output.view.translation_keys() {
            for locale in Locale::ALL {
                assert!(
                    translator.has(locale, &key),
                    "brak klucza {key} w języku {}",
                    locale.code()
                );
            }
        }
        assert_eq!(boolean_key(true), "common.value.yes");
        assert_eq!(boolean_key(false), "common.value.no");
    }
}
