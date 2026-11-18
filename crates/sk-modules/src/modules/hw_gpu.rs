//! `hw.gpu` — adaptery graficzne, sterowniki, tryb pracy i realny VRAM.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{date_only, decode_all, format_bytes, text};

const GPU_QUERY: &str = queries::WIN32_VIDEO_CONTROLLER;

/// Wartość, od której `AdapterRAM` jest już obcięte. Pole jest 32-bitowe,
/// a sterowniki raportują maksimum 4 GiB minus 1 MiB (4 293 918 720 B) —
/// potwierdzone na RX 6700 XT 12 GB. Od tej granicy nie traktujemy jej jako faktu.
const ADAPTER_RAM_CAP_BYTES: u64 = 4_293_918_720;

#[derive(Debug, Default, Deserialize)]
struct VideoController {
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "AdapterCompatibility", default)]
    vendor: Option<String>,
    #[serde(rename = "VideoProcessor", default)]
    processor: Option<String>,
    #[serde(rename = "DriverVersion", default)]
    driver_version: Option<String>,
    #[serde(rename = "DriverDate", default)]
    driver_date: Option<String>,
    #[serde(rename = "AdapterRAM", default)]
    adapter_ram: Option<u32>,
    #[serde(rename = "CurrentHorizontalResolution", default)]
    width: Option<u32>,
    #[serde(rename = "CurrentVerticalResolution", default)]
    height: Option<u32>,
    #[serde(rename = "CurrentRefreshRate", default)]
    refresh_hz: Option<u32>,
    #[serde(rename = "CurrentBitsPerPixel", default)]
    bits_per_pixel: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Gpu;

impl Module for Gpu {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.gpu",
            name_key: "module.hw_gpu.name",
            desc_key: "module.hw_gpu.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let adapters: Vec<VideoController> = decode_all(
            &ctx.wmi.query(NS_CIMV2, GPU_QUERY)?,
            "Win32_VideoController",
        )?;

        if adapters.is_empty() {
            return Ok(ModuleOutput {
                data: serde_json::json!({
                    "schema_version": 1,
                    "adapters": [],
                }),
                view: View::new(vec![Section::new("module.hw_gpu.section").with_blocks(
                    vec![Block::note(NoteLevel::Warn, "module.hw_gpu.note.none")],
                )]),
            });
        }

        let registry_memory = ctx.machine.adapter_memory();
        let mut vram_note = false;
        let mut rows = Vec::new();
        let mut data_adapters = Vec::new();

        for adapter in &adapters {
            let name = text(adapter.name.as_deref());
            let (vram, source) = resolve_vram(adapter, &registry_memory);
            if source == VramSource::WmiCapped {
                vram_note = true;
            }

            rows.push(vec![
                name.clone(),
                text(adapter.vendor.as_deref()),
                text(adapter.driver_version.as_deref()),
                adapter
                    .driver_date
                    .as_deref()
                    .map_or_else(|| "-".to_string(), date_only),
                mode_description(adapter),
                vram.clone(),
            ]);

            data_adapters.push(serde_json::json!({
                "name": crate::util::trimmed(adapter.name.clone()),
                "vendor": crate::util::trimmed(adapter.vendor.clone()),
                "processor": crate::util::trimmed(adapter.processor.clone()),
                "driver_version": crate::util::trimmed(adapter.driver_version.clone()),
                "driver_date": adapter.driver_date.as_deref().map(date_only),
                "vram_bytes": match source {
                    VramSource::Registry(bytes) => Some(bytes),
                    _ => None,
                },
                "adapter_ram_wmi_bytes": adapter.adapter_ram,
                "vram_source": source.label(),
                "width": adapter.width,
                "height": adapter.height,
                "refresh_hz": adapter.refresh_hz,
                "bits_per_pixel": adapter.bits_per_pixel,
            }));
        }

        let mut blocks = vec![Block::Table {
            table: Table::new(
                vec![
                    "module.hw_gpu.col.adapter".to_string(),
                    "module.hw_gpu.col.vendor".to_string(),
                    "module.hw_gpu.col.driver".to_string(),
                    "module.hw_gpu.col.driver_date".to_string(),
                    "module.hw_gpu.col.mode".to_string(),
                    "module.hw_gpu.col.vram".to_string(),
                ],
                rows,
            ),
        }];

        if vram_note {
            blocks.push(Block::note(NoteLevel::Warn, "module.hw_gpu.note.vram"));
        }

        let view = View::new(vec![
            Section::new("module.hw_gpu.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "adapters": data_adapters,
        });

        Ok(ModuleOutput { data, view })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VramSource {
    Registry(u64),
    WmiBelowCap(u64),
    WmiCapped,
    Unknown,
}

impl VramSource {
    fn label(self) -> &'static str {
        match self {
            Self::Registry(_) => "registry",
            Self::WmiBelowCap(_) => "wmi",
            Self::WmiCapped => "wmi_capped",
            Self::Unknown => "unknown",
        }
    }
}

/// VRAM: najpierw rejestr (`qwMemorySize`), potem `AdapterRAM` — ale tylko
/// wtedy, gdy nie widać obcięcia do 4 GiB.
fn resolve_vram(
    adapter: &VideoController,
    registry_memory: &[AdapterMemory],
) -> (String, VramSource) {
    let name = adapter.name.as_deref().unwrap_or_default().trim();
    if let Some(found) = registry_memory
        .iter()
        .find(|entry| entry.name.trim().eq_ignore_ascii_case(name))
    {
        return (format_bytes(found.bytes), VramSource::Registry(found.bytes));
    }

    match adapter.adapter_ram {
        Some(bytes) => {
            let bytes = u64::from(bytes);
            if bytes > 0 && bytes < ADAPTER_RAM_CAP_BYTES {
                (format_bytes(bytes), VramSource::WmiBelowCap(bytes))
            } else if bytes >= ADAPTER_RAM_CAP_BYTES {
                (
                    format!("≥ {}", format_bytes(ADAPTER_RAM_CAP_BYTES)),
                    VramSource::WmiCapped,
                )
            } else {
                ("-".to_string(), VramSource::Unknown)
            }
        }
        None => ("-".to_string(), VramSource::Unknown),
    }
}

fn mode_description(adapter: &VideoController) -> String {
    let resolution = match (adapter.width, adapter.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => format!("{width} × {height}"),
        _ => "-".to_string(),
    };
    let refresh = adapter
        .refresh_hz
        .map_or_else(|| "-".to_string(), |hz| format!("{hz} Hz"));
    let depth = adapter
        .bits_per_pixel
        .map_or_else(|| "-".to_string(), |bits| format!("{bits} bit"));
    format!("{resolution} @ {refresh} ({depth})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockMachine, MockWmi};

    fn harness(registry_memory: Option<u64>) -> Harness {
        let wmi = MockWmi::new().with_rows(
            "Win32_VideoController",
            fixture_rows!("win32_videocontroller.json"),
        );
        let machine = match registry_memory {
            Some(bytes) => MockMachine::new().with_adapter_memory("AMD Radeon RX 6700 XT", bytes),
            None => MockMachine::new(),
        };
        Harness::new().with_wmi(wmi).with_machine(machine)
    }

    #[test]
    fn reads_real_vram_from_registry_instead_of_capped_wmi() {
        let output = Gpu
            .collect(&harness(Some(12_868_124_672)).ctx())
            .expect("zbieranie danych");
        let adapter = &output.data["adapters"][0];
        assert_eq!(adapter["vram_source"], "registry");
        assert_eq!(adapter["vram_bytes"], 12_868_124_672u64);
        assert_eq!(adapter["adapter_ram_wmi_bytes"], 4_293_918_720u32);
        assert!(!output.view.uses_key("module.hw_gpu.note.vram"));
    }

    #[test]
    fn warns_when_only_capped_wmi_value_is_available() {
        let output = Gpu.collect(&harness(None).ctx()).expect("zbieranie danych");
        let adapter = &output.data["adapters"][0];
        assert_eq!(adapter["vram_source"], "wmi_capped");
        assert!(adapter["vram_bytes"].is_null());
        assert!(output.view.uses_key("module.hw_gpu.note.vram"));
    }

    #[test]
    fn describes_current_mode() {
        let output = Gpu
            .collect(&harness(Some(12_868_124_672)).ctx())
            .expect("zbieranie danych");
        let adapter = &output.data["adapters"][0];
        assert_eq!(adapter["width"], 2560);
        assert_eq!(adapter["height"], 1440);
        assert_eq!(adapter["refresh_hz"], 180);
    }

    #[test]
    fn reports_missing_adapter_as_note_not_error() {
        let wmi = MockWmi::new().with_rows("Win32_VideoController", vec![]);
        let harness = Harness::new().with_wmi(wmi);
        let output = Gpu
            .collect(&harness.ctx())
            .expect("pusta lista to nie błąd");
        assert!(output.view.uses_key("module.hw_gpu.note.none"));
        assert_eq!(output.data["adapters"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn formats_mode_and_handles_missing_values() {
        let adapter = VideoController {
            width: Some(1920),
            height: Some(1080),
            refresh_hz: None,
            bits_per_pixel: Some(32),
            ..VideoController::default()
        };
        assert_eq!(mode_description(&adapter), "1920 × 1080 @ - (32 bit)");
        assert_eq!(mode_description(&VideoController::default()), "- @ - (-)");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = Gpu
            .collect(&harness(Some(12_868_124_672)).ctx())
            .expect("zbieranie danych");
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
