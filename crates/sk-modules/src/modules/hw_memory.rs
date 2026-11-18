//! `hw.memory` — moduły RAM, sloty, taktowanie i limity płyty.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{decode_all, decode_first, format_bytes, text};

const MODULES_QUERY: &str = "SELECT BankLabel, DeviceLocator, Capacity, Speed, \
     ConfiguredClockSpeed, Manufacturer, PartNumber, SerialNumber, SMBIOSMemoryType, \
     FormFactor, ConfiguredVoltage FROM Win32_PhysicalMemory";
const ARRAY_QUERY: &str = "SELECT MaxCapacity, MaxCapacityEx, MemoryDevices, \
     MemoryErrorCorrection FROM Win32_PhysicalMemoryArray";

#[derive(Debug, Default, Deserialize)]
struct MemoryModule {
    #[serde(rename = "BankLabel", default)]
    bank_label: Option<String>,
    #[serde(rename = "DeviceLocator", default)]
    device_locator: Option<String>,
    #[serde(rename = "Capacity", default)]
    capacity: Option<u64>,
    #[serde(rename = "Speed", default)]
    speed_mhz: Option<u32>,
    #[serde(rename = "ConfiguredClockSpeed", default)]
    configured_clock_mhz: Option<u32>,
    #[serde(rename = "Manufacturer", default)]
    manufacturer: Option<String>,
    #[serde(rename = "PartNumber", default)]
    part_number: Option<String>,
    #[serde(rename = "SerialNumber", default)]
    serial_number: Option<String>,
    #[serde(rename = "SMBIOSMemoryType", default)]
    memory_type: Option<u16>,
    #[serde(rename = "FormFactor", default)]
    form_factor: Option<u16>,
    #[serde(rename = "ConfiguredVoltage", default)]
    configured_voltage_mv: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct MemoryArray {
    #[serde(rename = "MaxCapacity", default)]
    max_capacity_kib: Option<u32>,
    #[serde(rename = "MaxCapacityEx", default)]
    max_capacity_ex_kib: Option<u32>,
    #[serde(rename = "MemoryDevices", default)]
    slots: Option<u16>,
    #[serde(rename = "MemoryErrorCorrection", default)]
    error_correction: Option<u16>,
}

#[derive(Debug, Default)]
pub struct Memory;

impl Module for Memory {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.memory",
            name_key: "module.hw_memory.name",
            desc_key: "module.hw_memory.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let modules: Vec<MemoryModule> = decode_all(
            &ctx.wmi.query(NS_CIMV2, MODULES_QUERY)?,
            "Win32_PhysicalMemory",
        )?;
        let array: Option<MemoryArray> = ctx
            .wmi
            .query(NS_CIMV2, ARRAY_QUERY)
            .ok()
            .and_then(|rows| decode_first(&rows, "Win32_PhysicalMemoryArray").ok());

        let total_bytes: u64 = modules.iter().filter_map(|module| module.capacity).sum();
        let slots_total = array.as_ref().and_then(|array| array.slots);
        let max_capacity_kib = array.as_ref().and_then(|array| {
            array
                .max_capacity_ex_kib
                .filter(|value| *value > 0)
                .or(array.max_capacity_kib)
        });

        let slots_value = match slots_total {
            Some(total) => format!("{} / {total}", modules.len()),
            None => modules.len().to_string(),
        };

        let mut summary = vec![
            KeyValue::new("module.hw_memory.modules_count", modules.len().to_string()),
            KeyValue::new("module.hw_memory.total", format_bytes(total_bytes)),
            KeyValue::new("module.hw_memory.slots", slots_value),
        ];
        if let Some(kib) = max_capacity_kib {
            let bytes = u64::from(kib) * 1024;
            summary.push(KeyValue::new(
                "module.hw_memory.max_capacity",
                format_bytes(bytes),
            ));
        }
        if let Some(code) = array.as_ref().and_then(|array| array.error_correction) {
            summary.push(KeyValue::new(
                "module.hw_memory.ecc",
                error_correction_name(code),
            ));
        }

        let mut blocks = vec![Block::key_values(summary)];

        if modules.is_empty() {
            blocks.push(Block::note(
                NoteLevel::Warn,
                "module.hw_memory.note.no_modules",
            ));
        } else {
            let rows = modules
                .iter()
                .map(|module| {
                    vec![
                        slot_name(module).to_string(),
                        format_bytes(module.capacity.unwrap_or(0)),
                        format_mhz(module.configured_clock_mhz.or(module.speed_mhz)),
                        text(module.manufacturer.as_deref()),
                        text(module.part_number.as_deref()),
                        memory_type_name(module.memory_type),
                        form_factor_name(module.form_factor),
                        format_voltage(module.configured_voltage_mv),
                    ]
                })
                .collect();
            blocks.push(Block::Table {
                table: Table::new(
                    vec![
                        "module.hw_memory.col.slot".to_string(),
                        "module.hw_memory.col.size".to_string(),
                        "module.hw_memory.col.speed".to_string(),
                        "module.hw_memory.col.vendor".to_string(),
                        "module.hw_memory.col.part".to_string(),
                        "module.hw_memory.col.type".to_string(),
                        "module.hw_memory.col.form_factor".to_string(),
                        "module.hw_memory.col.voltage".to_string(),
                    ],
                    rows,
                ),
            });
        }

        let view = View::new(vec![
            Section::new("module.hw_memory.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "total_bytes": total_bytes,
            "modules_count": modules.len(),
            "slots_total": slots_total,
            "max_capacity_bytes": max_capacity_kib.map(|kib| u64::from(kib) * 1024),
            "error_correction": array.as_ref().and_then(|array| array.error_correction),
            "modules": modules.iter().map(|module| serde_json::json!({
                "slot": crate::util::trimmed(module.device_locator.clone()),
                "bank": crate::util::trimmed(module.bank_label.clone()),
                "capacity_bytes": module.capacity,
                "speed_mhz": module.speed_mhz,
                "configured_clock_mhz": module.configured_clock_mhz,
                "manufacturer": crate::util::trimmed(module.manufacturer.clone()),
                "part_number": crate::util::trimmed(module.part_number.clone()),
                "serial_number": crate::util::trimmed(module.serial_number.clone()),
                "smbios_memory_type": module.memory_type,
                "form_factor": module.form_factor,
                "configured_voltage_mv": module.configured_voltage_mv,
            })).collect::<Vec<_>>(),
        });

        Ok(ModuleOutput { data, view })
    }
}

fn slot_name(module: &MemoryModule) -> String {
    let locator = text(module.device_locator.as_deref());
    let bank = text(module.bank_label.as_deref());
    if locator == "-" && bank == "-" {
        "-".to_string()
    } else if bank == "-" {
        locator
    } else {
        format!("{locator} ({bank})")
    }
}

fn format_mhz(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_string(), |mhz| format!("{mhz} MHz"))
}

fn format_voltage(millivolts: Option<u32>) -> String {
    millivolts.map_or_else(
        || "-".to_string(),
        |mv| format!("{:.2} V", f64::from(mv) / 1000.0),
    )
}

/// Kody `SMBIOSMemoryType` (DDR4 = 26, DDR5 = 34). Nieznane kody pokazujemy
/// jako liczbę, a nie zgadywany typ.
fn memory_type_name(code: Option<u16>) -> String {
    match code {
        Some(20) => "DDR".to_string(),
        Some(21) => "DDR2".to_string(),
        Some(22) => "DDR2 FB-DIMM".to_string(),
        Some(24) => "DDR3".to_string(),
        Some(26) => "DDR4".to_string(),
        Some(27) => "LPDDR".to_string(),
        Some(28) => "LPDDR2".to_string(),
        Some(29) => "LPDDR3".to_string(),
        Some(30) => "LPDDR4".to_string(),
        Some(32) => "HBM".to_string(),
        Some(33) => "HBM2".to_string(),
        Some(34) => "DDR5".to_string(),
        Some(35) => "LPDDR5".to_string(),
        Some(0) | None => "-".to_string(),
        Some(code) => format!("SMBIOS #{code}"),
    }
}

/// Kody `FormFactor` (`DIMM` = 8, `SODIMM` = 12).
fn form_factor_name(code: Option<u16>) -> String {
    match code {
        Some(8) => "DIMM".to_string(),
        Some(12) => "SODIMM".to_string(),
        Some(13) => "SRIMM".to_string(),
        Some(23) => "LGA".to_string(),
        Some(24) => "FB-DIMM".to_string(),
        Some(0) | None => "-".to_string(),
        Some(code) => format!("SMBIOS #{code}"),
    }
}

/// Kody `MemoryErrorCorrection`.
fn error_correction_name(code: u16) -> String {
    match code {
        3 => "None".to_string(),
        4 => "Parity".to_string(),
        5 => "Single-bit ECC".to_string(),
        6 => "Multi-bit ECC".to_string(),
        other => format!("SMBIOS #{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::i18n::Translator;
    use sk_core::testing::{Harness, MockWmi};

    fn harness() -> Harness {
        let wmi = MockWmi::new()
            .with_rows(
                "Win32_PhysicalMemory",
                fixture_rows!("win32_physicalmemory.json"),
            )
            .with_rows(
                "Win32_PhysicalMemoryArray",
                fixture_rows!("win32_physicalmemoryarray.json"),
            );
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn summarises_two_modules_from_real_fixture() {
        let output = Memory.collect(&harness().ctx()).expect("zbieranie danych");
        assert_eq!(output.data["modules_count"], 2);
        assert_eq!(output.data["total_bytes"], 34_359_738_368u64);
        assert_eq!(output.data["slots_total"], 2);
        assert_eq!(output.data["max_capacity_bytes"], 137_438_953_472u64);
        assert_eq!(output.data["error_correction"], 3);
        assert!(output.view.uses_key("module.hw_memory.col.speed"));
    }

    #[test]
    fn renders_module_table() {
        let output = Memory.collect(&harness().ctx()).expect("zbieranie danych");
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela modułów");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][1], "16.0 GB");
        assert_eq!(table.rows[0][2], "3200 MHz");
        assert_eq!(table.rows[0][5], "DDR4");
    }

    #[test]
    fn degrades_without_any_module() {
        let harness = Harness::new();
        let error = Memory
            .collect(&harness.ctx())
            .expect_err("brak odpowiedzi WMI to błąd");
        assert!(
            error.to_string().contains("Win32_PhysicalMemory"),
            "{error}"
        );
    }

    #[test]
    fn empty_module_list_shows_note_instead_of_error() {
        let wmi = MockWmi::new().with_rows("Win32_PhysicalMemory", vec![]);
        let harness = Harness::new().with_wmi(wmi);
        let output = Memory
            .collect(&harness.ctx())
            .expect("puste dane to nie błąd");
        assert!(output.view.uses_key("module.hw_memory.note.no_modules"));
        assert_eq!(output.data["modules_count"], 0);
    }

    #[test]
    fn maps_smbios_codes() {
        assert_eq!(memory_type_name(Some(26)), "DDR4");
        assert_eq!(memory_type_name(Some(34)), "DDR5");
        assert_eq!(memory_type_name(Some(99)), "SMBIOS #99");
        assert_eq!(memory_type_name(None), "-");
        assert_eq!(form_factor_name(Some(8)), "DIMM");
        assert_eq!(form_factor_name(Some(12)), "SODIMM");
        assert_eq!(error_correction_name(3), "None");
        assert_eq!(error_correction_name(5), "Single-bit ECC");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = Memory.collect(&harness().ctx()).expect("zbieranie danych");
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
    }
}
