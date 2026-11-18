//! `system.info` — system operacyjny, model komputera, RAM, uptime, uprawnienia.

use chrono::{DateTime, Local};
use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{bool_row, decode_first, format_bytes, text};

const OS_QUERY: &str = "SELECT Caption, Version, BuildNumber, OSArchitecture \
     FROM Win32_OperatingSystem";
const COMPUTER_QUERY: &str = "SELECT Manufacturer, Model, TotalPhysicalMemory, \
     HypervisorPresent, PCSystemType, NumberOfLogicalProcessors FROM Win32_ComputerSystem";

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
    #[serde(rename = "HypervisorPresent", default)]
    hypervisor_present: Option<bool>,
    #[serde(rename = "PCSystemType", default)]
    pc_system_type: Option<u16>,
    #[serde(rename = "NumberOfLogicalProcessors", default)]
    logical_processors: Option<u32>,
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
            decode_first(&ctx.wmi.query(NS_CIMV2, OS_QUERY)?, "Win32_OperatingSystem")?;
        let computer: ComputerSystem = decode_first(
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
            .map_or_else(|| "-".to_string(), format_bytes);

        let mut host_rows = vec![
            KeyValue::new(
                "module.system_info.host.manufacturer",
                text(computer.manufacturer.as_deref()),
            ),
            KeyValue::new(
                "module.system_info.host.model",
                text(computer.model.as_deref()),
            ),
            KeyValue::new("module.system_info.host.ram_total", ram_total.clone()),
            KeyValue::new("module.system_info.host.uptime", format_uptime(uptime_secs)),
            KeyValue::new("module.system_info.host.local_time", local_time.clone()),
        ];
        host_rows.push(KeyValue::state(
            "module.system_info.host.system_type",
            system_type_key(computer.pc_system_type),
        ));
        host_rows.push(bool_row(
            "module.system_info.host.hypervisor",
            computer.hypervisor_present,
        ));

        let view = View::new(vec![
            Section::new("module.system_info.section.os").with_blocks(vec![
                Block::key_values(vec![
                    KeyValue::new("module.system_info.os.caption", text(os.caption.as_deref())),
                    KeyValue::new("module.system_info.os.version", text(os.version.as_deref())),
                    KeyValue::new(
                        "module.system_info.os.build",
                        text(os.build_number.as_deref()),
                    ),
                    KeyValue::new(
                        "module.system_info.os.arch",
                        text(os.os_architecture.as_deref()),
                    ),
                ]),
                Block::badge(if elevated {
                    "common.state.elevated"
                } else {
                    "common.state.userland"
                }),
            ]),
            Section::new("module.system_info.section.host").with_blocks(vec![
                Block::key_values(host_rows),
                Block::note(NoteLevel::Info, "module.system_info.note.hypervisor"),
            ]),
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
                "hypervisor_present": computer.hypervisor_present,
                "pc_system_type": computer.pc_system_type,
                "logical_processors": computer.logical_processors,
            },
            "uptime_secs": uptime_secs,
            "local_time": local_time,
            "elevated": elevated,
        });

        Ok(ModuleOutput { data, view })
    }
}

/// `Win32_ComputerSystem.PCSystemType` — 1 = desktop, 2 = laptop.
fn system_type_key(code: Option<u16>) -> &'static str {
    match code {
        Some(1) => "common.system_type.desktop",
        Some(2) => "common.system_type.mobile",
        Some(3) => "common.system_type.workstation",
        Some(4) => "common.system_type.enterprise_server",
        Some(5) => "common.system_type.soho_server",
        Some(6) => "common.system_type.appliance",
        Some(7) => "common.system_type.performance_server",
        Some(8) => "common.system_type.slate",
        _ => "common.value.unknown",
    }
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
                fixture_rows!("win32_operatingsystem.json"),
            )
            .with_rows(
                "Win32_ComputerSystem",
                fixture_rows!("win32_computersystem.json"),
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
        assert_eq!(output.data["host"]["ram_total_bytes"], 34_281_689_088u64);
    }

    #[test]
    fn describes_machine_type_and_hypervisor() {
        let output = SystemInfo
            .collect(&harness(true).ctx())
            .expect("zbieranie danych");
        assert_eq!(output.data["host"]["pc_system_type"], 1);
        assert_eq!(output.data["host"]["hypervisor_present"], true);
        assert!(output.view.uses_key("common.system_type.desktop"));
        assert!(output.view.uses_key("module.system_info.host.hypervisor"));
        assert!(output.view.uses_key("module.system_info.note.hypervisor"));
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
    fn maps_system_type_codes() {
        assert_eq!(system_type_key(Some(1)), "common.system_type.desktop");
        assert_eq!(system_type_key(Some(2)), "common.system_type.mobile");
        assert_eq!(system_type_key(Some(8)), "common.system_type.slate");
        assert_eq!(system_type_key(None), "common.value.unknown");
        assert_eq!(system_type_key(Some(99)), "common.value.unknown");
    }
}
