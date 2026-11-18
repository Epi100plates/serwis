//! `system.info` — system operacyjny, model komputera, RAM, uptime, uprawnienia.

use chrono::{DateTime, Local};
use serde::Deserialize;
use sk_core::prelude::*;

const OS_QUERY: &str =
    "SELECT Caption, Version, BuildNumber, OSArchitecture FROM Win32_OperatingSystem";
const COMPUTER_QUERY: &str =
    "SELECT Manufacturer, Model, TotalPhysicalMemory FROM Win32_ComputerSystem";

#[derive(Debug, Default, Deserialize)]
struct OperatingSystem {
    #[serde(rename = "Caption", default)]
    caption: Option<String>,
    #[serde(rename = "Version", default)]
    version: Option<String>,
    #[serde(rename = "BuildNumber", default)]
    build_number: Option<String>,
    #[serde(rename = "OSArchitecture", default)]
    os_architecture: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ComputerSystem {
    #[serde(rename = "Manufacturer", default)]
    manufacturer: Option<String>,
    #[serde(rename = "Model", default)]
    model: Option<String>,
    #[serde(rename = "TotalPhysicalMemory", default)]
    total_physical_memory: Option<u64>,
}

#[derive(Debug, Default)]
pub struct SystemInfo;

impl Module for SystemInfo {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "system.info",
            name_key: "module.system_info.name",
            desc_key: "module.system_info.desc",
            group: Group::System,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let os: OperatingSystem =
            decode_row(&ctx.wmi.query(NS_CIMV2, OS_QUERY)?, "Win32_OperatingSystem")?;
        let computer: ComputerSystem = decode_row(
            &ctx.wmi.query(NS_CIMV2, COMPUTER_QUERY)?,
            "Win32_ComputerSystem",
        )?;

        let uptime_secs = ctx.clock.uptime_secs();
        let local_time = DateTime::<Local>::from(ctx.clock.now())
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let elevated = ctx.host.is_elevated();
        let ram_total = computer
            .total_physical_memory
            .map(format_gib)
            .unwrap_or_else(|| "-".to_string());

        let view = View::new(vec![
            Section::new("module.system_info.section.os").with_blocks(vec![
                Block::key_values(vec![
                    KeyValue::new("module.system_info.os.caption", text(&os.caption)),
                    KeyValue::new("module.system_info.os.version", text(&os.version)),
                    KeyValue::new("module.system_info.os.build", text(&os.build_number)),
                    KeyValue::new("module.system_info.os.arch", text(&os.os_architecture)),
                ]),
                Block::badge(if elevated {
                    "common.state.elevated"
                } else {
                    "common.state.userland"
                }),
            ]),
            Section::new("module.system_info.section.host").with_blocks(vec![Block::key_values(
                vec![
                    KeyValue::new(
                        "module.system_info.host.manufacturer",
                        text(&computer.manufacturer),
                    ),
                    KeyValue::new("module.system_info.host.model", text(&computer.model)),
                    KeyValue::new("module.system_info.host.ram_total", ram_total.clone()),
                    KeyValue::new("module.system_info.host.uptime", format_uptime(uptime_secs)),
                    KeyValue::new("module.system_info.host.local_time", local_time.clone()),
                ],
            )]),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "os": {
                "caption": os.caption,
                "version": os.version,
                "build_number": os.build_number,
                "architecture": os.os_architecture,
            },
            "host": {
                "manufacturer": computer.manufacturer,
                "model": computer.model,
                "ram_total_bytes": computer.total_physical_memory,
            },
            "uptime_secs": uptime_secs,
            "local_time": local_time,
            "elevated": elevated,
        });

        Ok(ModuleOutput { data, view })
    }
}

fn decode_row<T: serde::de::DeserializeOwned>(
    rows: &[serde_json::Value],
    class: &str,
) -> Result<T> {
    let row = rows
        .first()
        .ok_or_else(|| ModuleError::unavailable(format!("{class}: brak instancji")))?;
    serde_json::from_value(row.clone())
        .map_err(|err| ModuleError::provider("WMI", format!("{class}: {err}")))
}

fn text(value: &Option<String>) -> String {
    match value {
        Some(text) if !text.trim().is_empty() => text.clone(),
        _ => "-".to_string(),
    }
}

fn format_gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

fn format_uptime(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    format!("{days}d {hours:02}:{minutes:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::i18n::Translator;
    use sk_core::testing::{Harness, MockHost, MockWmi};

    fn harness(elevated: bool) -> Harness {
        let wmi = MockWmi::new()
            .with_rows(
                "Win32_OperatingSystem",
                vec![serde_json::json!({
                    "Caption": "Microsoft Windows 11 Enterprise",
                    "Version": "10.0.26300",
                    "BuildNumber": "26300",
                    "OSArchitecture": "64-bit"
                })],
            )
            .with_rows(
                "Win32_ComputerSystem",
                vec![serde_json::json!({
                    "Manufacturer": "ASUSTeK COMPUTER INC.",
                    "Model": "PRIME B450M-A",
                    "TotalPhysicalMemory": 34_359_738_368u64
                })],
            );
        let host = if elevated {
            MockHost::elevated()
        } else {
            MockHost::userland()
        };
        Harness::new().with_wmi(wmi).with_host(host)
    }

    #[test]
    fn renders_both_sections_with_data() {
        let output = SystemInfo
            .collect(&harness(true).ctx())
            .expect("zbieranie danych");
        assert_eq!(output.view.sections.len(), 2);
        assert!(output.view.uses_key("module.system_info.os.caption"));
        assert!(output.view.uses_key("module.system_info.host.ram_total"));
        assert_eq!(output.data["os"]["build_number"], "26300");
        assert_eq!(output.data["host"]["ram_total_bytes"], 34_359_738_368u64);
    }

    #[test]
    fn shows_administrator_badge_when_elevated() {
        let output = SystemInfo
            .collect(&harness(true).ctx())
            .expect("zbieranie danych");
        assert!(output.view.uses_key("common.state.elevated"));
        assert!(!output.view.uses_key("common.state.userland"));
    }

    #[test]
    fn shows_user_badge_without_elevation() {
        let output = SystemInfo
            .collect(&harness(false).ctx())
            .expect("zbieranie danych");
        assert!(output.view.uses_key("common.state.userland"));
    }

    #[test]
    fn missing_wmi_instance_is_reported_not_panicked() {
        let harness = Harness::new();
        let error = SystemInfo
            .collect(&harness.ctx())
            .expect_err("brak instancji to błąd");
        assert!(
            error.to_string().contains("Win32_OperatingSystem"),
            "{error}"
        );
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = SystemInfo
            .collect(&harness(true).ctx())
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

    #[test]
    fn formats_uptime_as_days_hours_minutes() {
        assert_eq!(format_uptime(0), "0d 00:00");
        assert_eq!(format_uptime(3_661), "0d 01:01");
        assert_eq!(format_uptime(90_061), "1d 01:01");
    }

    #[test]
    fn formats_ram_in_gib() {
        assert_eq!(format_gib(34_359_738_368), "32.0 GB");
        assert_eq!(format_gib(0), "0.0 GB");
    }
}
