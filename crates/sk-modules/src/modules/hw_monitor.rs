//! `hw.monitor` — monitory: model, producent, rok, przekątna, tryb natywny.
//!
//! WMI podaje model jako tablicę kodów UTF-16, a rozdzielczość natywną tylko
//! w EDID — dlatego moduł łączy `WmiMonitorID`, `WmiMonitorBasicDisplayParams`,
//! blok EDID z rejestru (`MachineInfo::display_edid`) i bieżący tryb z karty
//! graficznej.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::edid;
use crate::util::{decode_all, number};

const MONITOR_QUERY: &str = "SELECT Active, InstanceName, ManufacturerName, ProductCodeID, \
     SerialNumberID, UserFriendlyName, WeekOfManufacture, YearOfManufacture FROM WmiMonitorID";
const DISPLAY_PARAMS_QUERY: &str = "SELECT InstanceName, MaxHorizontalImageSize, \
     MaxVerticalImageSize, VideoInputType FROM WmiMonitorBasicDisplayParams";
const MODE_QUERY: &str = queries::WIN32_VIDEO_CONTROLLER;

#[derive(Debug, Default, Deserialize)]
struct MonitorId {
    #[serde(rename = "Active", default)]
    active: Option<bool>,
    #[serde(rename = "InstanceName", default)]
    instance_name: Option<String>,
    #[serde(rename = "ManufacturerName", default)]
    manufacturer: Option<Vec<u16>>,
    #[serde(rename = "ProductCodeID", default)]
    product_code: Option<Vec<u16>>,
    #[serde(rename = "UserFriendlyName", default)]
    name: Option<Vec<u16>>,
    #[serde(rename = "WeekOfManufacture", default)]
    week: Option<u8>,
    #[serde(rename = "YearOfManufacture", default)]
    year: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
struct DisplayParams {
    #[serde(rename = "InstanceName", default)]
    instance_name: Option<String>,
    #[serde(rename = "MaxHorizontalImageSize", default)]
    width_cm: Option<u8>,
    #[serde(rename = "MaxVerticalImageSize", default)]
    height_cm: Option<u8>,
    #[serde(rename = "VideoInputType", default)]
    video_input_type: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
struct CurrentMode {
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
pub struct Monitor;

impl Module for Monitor {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.monitor",
            name_key: "module.hw_monitor.name",
            desc_key: "module.hw_monitor.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let monitors: Vec<MonitorId> =
            decode_all(&ctx.wmi.query(NS_WMI, MONITOR_QUERY)?, "WmiMonitorID")?;

        if monitors.is_empty() {
            return Ok(ModuleOutput {
                data: serde_json::json!({ "schema_version": 1, "monitors": [] }),
                view: View::new(vec![Section::new("module.hw_monitor.section").with_blocks(
                    vec![Block::note(NoteLevel::Warn, "module.hw_monitor.note.none")],
                )]),
            });
        }

        // Parametry fizyczne i tryb bieżący są opcjonalne — brak nie psuje raportu.
        let params = ctx
            .wmi
            .query(NS_WMI, DISPLAY_PARAMS_QUERY)
            .and_then(|rows| decode_all::<DisplayParams>(&rows, "WmiMonitorBasicDisplayParams"))
            .unwrap_or_default();
        let current_mode = ctx
            .wmi
            .query(NS_CIMV2, MODE_QUERY)
            .ok()
            .and_then(|rows| decode_all::<CurrentMode>(&rows, "Win32_VideoController").ok())
            .and_then(|modes| modes.into_iter().next())
            .unwrap_or_default();

        let mut rows = Vec::new();
        let mut input_keys: Vec<Option<&'static str>> = Vec::new();
        let mut data_monitors = Vec::new();

        for monitor in &monitors {
            let instance = monitor.instance_name.as_deref().unwrap_or_default();
            let info = ctx
                .machine
                .display_edid(instance)
                .and_then(|bytes| edid::parse(&bytes));
            let physical = params
                .iter()
                .find(|params| params.instance_name.as_deref() == Some(instance));

            let manufacturer = monitor
                .manufacturer
                .as_deref()
                .map(decode_chars)
                .filter(|text| !text.is_empty())
                .or_else(|| info.as_ref().map(|info| info.manufacturer.clone()))
                .unwrap_or_else(|| "-".to_string());
            let name = monitor
                .name
                .as_deref()
                .map(decode_chars)
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    monitor
                        .product_code
                        .as_deref()
                        .map(decode_chars)
                        .filter(|code| !code.is_empty())
                })
                .unwrap_or_else(|| "-".to_string());

            let diagonal_cm = info.as_ref().and_then(|info| info.size_cm).or_else(|| {
                physical.and_then(|params| match (params.width_cm, params.height_cm) {
                    (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
                    _ => None,
                })
            });
            let diagonal = info
                .as_ref()
                .and_then(|info| info.diagonal_inches())
                .map(|inches| format!("{inches:.1}\""))
                .unwrap_or_else(|| diagonal_inches(diagonal_cm));
            let native = info.as_ref().and_then(|info| info.native).map_or_else(
                || "-".to_string(),
                |(width, height)| format!("{width} × {height}"),
            );
            let max_refresh = info
                .as_ref()
                .and_then(|info| info.max_refresh_hz)
                .map_or_else(|| "-".to_string(), |hz| format!("{hz} Hz"));
            // `VideoInputType` z WMI (0 = analogowe, 1 = cyfrowe) jest wiarygodniejszy
            // niż EDID; EDID zostaje jako fallback. Wartość to stan, więc jedzie
            // kluczem i18n, a nie słowem wpisanym w kod.
            let input_digital = physical
                .and_then(|params| params.video_input_type)
                .map(|input| input == 1)
                .or_else(|| info.as_ref().map(|info| info.digital));
            let input_key = match input_digital {
                Some(true) => Some("common.value.digital"),
                Some(false) => Some("common.value.analog"),
                None => None,
            };

            rows.push(vec![
                name.clone(),
                manufacturer.clone(),
                production_date(monitor),
                diagonal,
                native,
                max_refresh,
                input_digital.map_or_else(|| "-".to_string(), |digital| digital.to_string()),
            ]);
            input_keys.push(input_key);

            data_monitors.push(serde_json::json!({
                "instance_name": instance,
                "active": monitor.active,
                "name": name,
                "manufacturer": manufacturer,
                "year": monitor.year,
                "week": monitor.week,
                "diagonal_cm": diagonal_cm,
                "native_width": info.as_ref().and_then(|info| info.native.map(|n| n.0)),
                "native_height": info.as_ref().and_then(|info| info.native.map(|n| n.1)),
                "native_refresh_hz": info.as_ref().and_then(|info| info.native_refresh_hz),
                "max_refresh_hz": info.as_ref().and_then(|info| info.max_refresh_hz),
                "edid_year": info.as_ref().map(|info| info.year),
                "edid_present": info.is_some(),
                "input_digital": input_digital,
            }));
        }

        let mut table = Table::new(
            vec![
                "module.hw_monitor.col.monitor".to_string(),
                "module.hw_monitor.col.vendor".to_string(),
                "module.hw_monitor.col.production".to_string(),
                "module.hw_monitor.col.diagonal".to_string(),
                "module.hw_monitor.col.native".to_string(),
                "module.hw_monitor.col.max_refresh".to_string(),
                "module.hw_monitor.col.input".to_string(),
            ],
            rows,
        );
        for (row_index, key) in input_keys.iter().enumerate() {
            if let Some(key) = key {
                table = table.with_value_key(row_index, 6, *key);
            }
        }

        let mut blocks = vec![Block::Table { table }];

        if data_monitors
            .iter()
            .any(|monitor| monitor["edid_present"] == false)
        {
            blocks.push(Block::note(
                NoteLevel::Info,
                "module.hw_monitor.note.no_edid",
            ));
        }

        let mut mode_rows = vec![
            KeyValue::new(
                "module.hw_monitor.mode.resolution",
                match (current_mode.width, current_mode.height) {
                    (Some(width), Some(height)) if width > 0 && height > 0 => {
                        format!("{width} × {height}")
                    }
                    _ => "-".to_string(),
                },
            ),
            KeyValue::new(
                "module.hw_monitor.mode.refresh",
                current_mode
                    .refresh_hz
                    .map_or_else(|| "-".to_string(), |hz| format!("{hz} Hz")),
            ),
            KeyValue::new(
                "module.hw_monitor.mode.depth",
                current_mode
                    .bits_per_pixel
                    .map_or_else(|| "-".to_string(), |bits| format!("{bits} bit")),
            ),
        ];
        mode_rows.push(KeyValue::new(
            "module.hw_monitor.mode.monitors",
            number(Some(monitors.len())),
        ));

        let mode_missing = current_mode.width.is_none()
            && current_mode.height.is_none()
            && current_mode.refresh_hz.is_none();
        blocks.push(Block::key_values(mode_rows));
        if mode_missing {
            blocks.push(Block::note(
                NoteLevel::Info,
                "module.hw_monitor.note.no_mode",
            ));
        }

        let view = View::new(vec![
            Section::new("module.hw_monitor.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "monitors": data_monitors,
            "current_mode": {
                "width": current_mode.width,
                "height": current_mode.height,
                "refresh_hz": current_mode.refresh_hz,
                "bits_per_pixel": current_mode.bits_per_pixel,
            },
        });

        Ok(ModuleOutput { data, view })
    }
}

/// Tablica kodów UTF-16 z WMI → tekst. Zera kończą łańcuch, reszta jest
/// uzupełnieniem do 16 znaków.
fn decode_chars(codes: &[u16]) -> String {
    let chars: String = codes
        .iter()
        .take_while(|code| **code != 0)
        .filter_map(|code| char::from_u32(u32::from(*code)))
        .filter(|c| !c.is_control())
        .collect();
    chars.trim().to_string()
}

fn production_date(monitor: &MonitorId) -> String {
    match (monitor.year, monitor.week) {
        (Some(year), Some(week)) if year > 0 && week > 0 => format!("{year}-W{week:02}"),
        (Some(year), _) if year > 0 => year.to_string(),
        _ => "-".to_string(),
    }
}

fn diagonal_inches(size_cm: Option<(u8, u8)>) -> String {
    let Some((width, height)) = size_cm else {
        return "-".to_string();
    };
    if width == 0 || height == 0 {
        return "-".to_string();
    }
    let diagonal_cm = (f64::from(width).powi(2) + f64::from(height).powi(2)).sqrt();
    format!("{:.1}\"", diagonal_cm / 2.54)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockMachine, MockWmi};

    const SAMSUNG_INSTANCE: &str = "DISPLAY\\SAM0928\\7&1c9e91e2&0&UID264_0";
    const GIGABYTE_INSTANCE: &str = "DISPLAY\\GBT2730\\7&1c9e91e2&0&UID260_0";

    fn rows(name: &str) -> Vec<serde_json::Value> {
        match name {
            "id" => fixture_rows!("wmi_wmimonitorid.json"),
            "params" => fixture_rows!("wmi_wmimonitorbasicdisplayparams.json"),
            "mode" => fixture_rows!("win32_videocontroller.json"),
            _ => Vec::new(),
        }
    }

    fn harness() -> Harness {
        let wmi = MockWmi::new()
            .with_rows("WmiMonitorID", rows("id"))
            .with_rows("WmiMonitorBasicDisplayParams", rows("params"))
            .with_rows("Win32_VideoController", rows("mode"));
        let machine = MockMachine::new()
            .with_edid(
                SAMSUNG_INSTANCE,
                edid::hex_to_bytes(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../docs/fixtures/edid/sam0928.hex"
                ))),
            )
            .with_edid(
                GIGABYTE_INSTANCE,
                edid::hex_to_bytes(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../docs/fixtures/edid/gbt2730.hex"
                ))),
            );
        Harness::new().with_wmi(wmi).with_machine(machine)
    }

    #[test]
    fn reads_both_monitors_from_real_fixture() {
        let output = Monitor.collect(&harness().ctx()).expect("zbieranie danych");
        let monitors = output.data["monitors"].as_array().expect("lista monitorów");
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0]["name"], "T19B300");
        assert_eq!(monitors[0]["manufacturer"], "SAM");
        assert_eq!(monitors[0]["native_width"], 1366);
        assert_eq!(monitors[0]["native_height"], 768);
        assert_eq!(monitors[1]["name"], "GS27QCA");
        assert_eq!(monitors[1]["native_width"], 2560);
        assert_eq!(monitors[1]["native_height"], 1440);
        assert_eq!(monitors[1]["max_refresh_hz"], 180);
    }

    #[test]
    fn reports_current_mode_from_gpu() {
        let output = Monitor.collect(&harness().ctx()).expect("zbieranie danych");
        assert_eq!(output.data["current_mode"]["width"], 2560);
        assert_eq!(output.data["current_mode"]["height"], 1440);
        assert_eq!(output.data["current_mode"]["refresh_hz"], 180);
    }

    #[test]
    fn degrades_without_edid() {
        let wmi = MockWmi::new()
            .with_rows("WmiMonitorID", rows("id"))
            .with_rows("WmiMonitorBasicDisplayParams", rows("params"));
        let harness = Harness::new().with_wmi(wmi);
        let output = Monitor.collect(&harness.ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_monitor.note.no_edid"));
        // Rozmiar i tak jest — z WmiMonitorBasicDisplayParams.
        assert_eq!(output.data["monitors"][0]["diagonal_cm"][0], 41);
    }

    #[test]
    fn input_is_translated_state_not_prose() {
        let output = Monitor.collect(&harness().ctx()).expect("zbieranie danych");
        assert!(output.view.uses_key("common.value.digital"));
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela monitorów");
        for row in &table.rows {
            for cell in row {
                assert_ne!(cell, "Digital", "proza w komórce tabeli");
                assert_ne!(cell, "Analog", "proza w komórce tabeli");
            }
        }
        assert_eq!(table.value_key(0, 6), Some("common.value.digital"));
    }

    #[test]
    fn decodes_utf16_character_arrays() {
        assert_eq!(decode_chars(&[83, 65, 77, 0, 0]), "SAM");
        assert_eq!(decode_chars(&[]), "");
        assert_eq!(decode_chars(&[0, 65]), "");
    }

    #[test]
    fn formats_diagonal_in_inches() {
        assert_eq!(diagonal_inches(Some((60, 33))), "27.0\"");
        assert_eq!(diagonal_inches(Some((41, 23))), "18.5\"");
        assert_eq!(diagonal_inches(Some((0, 0))), "-");
        assert_eq!(diagonal_inches(None), "-");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = Monitor.collect(&harness().ctx()).expect("zbieranie danych");
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
