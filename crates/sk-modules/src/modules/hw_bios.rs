//! `hw.bios` — BIOS/UEFI, wersja SMBIOS, kontroler EC, tryb firmware.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{date_only, decode_first, number, text};

const BIOS_QUERY: &str = "SELECT Manufacturer, SMBIOSBIOSVersion, Version, ReleaseDate, \
     SMBIOSMajorVersion, SMBIOSMinorVersion, SystemBiosMajorVersion, SystemBiosMinorVersion, \
     EmbeddedControllerMajorVersion, EmbeddedControllerMinorVersion FROM Win32_BIOS";

#[derive(Debug, Default, Deserialize)]
struct Bios {
    #[serde(rename = "Manufacturer", default)]
    manufacturer: Option<String>,
    #[serde(rename = "SMBIOSBIOSVersion", default)]
    smbios_version: Option<String>,
    #[serde(rename = "Version", default)]
    version: Option<String>,
    #[serde(rename = "ReleaseDate", default)]
    release_date: Option<String>,
    #[serde(rename = "SMBIOSMajorVersion", default)]
    smbios_major: Option<u16>,
    #[serde(rename = "SMBIOSMinorVersion", default)]
    smbios_minor: Option<u16>,
    #[serde(rename = "SystemBiosMajorVersion", default)]
    bios_major: Option<u16>,
    #[serde(rename = "SystemBiosMinorVersion", default)]
    bios_minor: Option<u16>,
    #[serde(rename = "EmbeddedControllerMajorVersion", default)]
    ec_major: Option<u16>,
    #[serde(rename = "EmbeddedControllerMinorVersion", default)]
    ec_minor: Option<u16>,
}

#[derive(Debug, Default)]
pub struct BiosModule;

impl Module for BiosModule {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.bios",
            name_key: "module.hw_bios.name",
            desc_key: "module.hw_bios.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let bios: Bios = decode_first(&ctx.wmi.query(NS_CIMV2, BIOS_QUERY)?, "Win32_BIOS")?;

        let mut blocks = vec![Block::key_values(vec![
            KeyValue::new(
                "module.hw_bios.manufacturer",
                text(bios.manufacturer.as_deref()),
            ),
            KeyValue::new(
                "module.hw_bios.version",
                text(bios.smbios_version.as_deref()),
            ),
            KeyValue::new(
                "module.hw_bios.date",
                bios.release_date
                    .as_deref()
                    .map_or_else(|| "-".to_string(), date_only),
            ),
            KeyValue::new(
                "module.hw_bios.smbios",
                format_version(bios.smbios_major, bios.smbios_minor),
            ),
            KeyValue::new(
                "module.hw_bios.system_version",
                format_version(bios.bios_major, bios.bios_minor),
            ),
            KeyValue::state(
                "module.hw_bios.firmware",
                firmware_key(ctx.machine.firmware()),
            ),
            KeyValue::new(
                "module.hw_bios.ec",
                format_version(bios.ec_major, bios.ec_minor),
            ),
            KeyValue::new("module.hw_bios.full_version", text(bios.version.as_deref())),
        ])];

        // 255.255 to w SMBIOS „nieznane”, a nie wersja 255.
        if !ec_is_known(&bios) {
            blocks.push(Block::note(NoteLevel::Info, "module.hw_bios.note.ec"));
        }

        let view = View::new(vec![
            Section::new("module.hw_bios.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "manufacturer": crate::util::trimmed(bios.manufacturer),
            "version": crate::util::trimmed(bios.smbios_version),
            "release_date": bios.release_date.as_deref().map(date_only),
            "smbios_major": bios.smbios_major,
            "smbios_minor": bios.smbios_minor,
            "bios_major": bios.bios_major,
            "bios_minor": bios.bios_minor,
            "embedded_controller_major": bios.ec_major,
            "embedded_controller_minor": bios.ec_minor,
            "firmware": ctx.machine.firmware(),
        });

        Ok(ModuleOutput { data, view })
    }
}

fn firmware_key(firmware: Firmware) -> &'static str {
    match firmware {
        Firmware::Uefi => "common.value.uefi",
        Firmware::Bios => "common.value.bios_legacy",
        Firmware::Unknown => "common.value.unknown",
    }
}

fn format_version(major: Option<u16>, minor: Option<u16>) -> String {
    match (major, minor) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        (Some(major), None) => number(Some(major)),
        _ => "-".to_string(),
    }
}

fn ec_is_known(bios: &Bios) -> bool {
    fn known(value: Option<u16>) -> bool {
        value.is_some_and(|value| value != 255 && value != 0xFFFF)
    }
    known(bios.ec_major) || known(bios.ec_minor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockMachine, MockWmi};

    fn harness(firmware: Firmware) -> Harness {
        let wmi = MockWmi::new().with_rows("Win32_BIOS", fixture_rows!("win32_bios.json"));
        Harness::new()
            .with_wmi(wmi)
            .with_machine(MockMachine::new().with_firmware(firmware))
    }

    #[test]
    fn renders_bios_data_from_fixture() {
        let output = BiosModule
            .collect(&harness(Firmware::Uefi).ctx())
            .expect("zbieranie danych");
        assert_eq!(output.data["version"], "1.B1");
        assert_eq!(output.data["release_date"], "2023-05-16");
        assert_eq!(output.data["smbios_major"], 2);
        assert_eq!(output.data["smbios_minor"], 8);
        assert_eq!(output.data["firmware"], "uefi");
        assert!(output.view.uses_key("common.value.uefi"));
    }

    #[test]
    fn unknown_controller_version_is_explained() {
        let output = BiosModule
            .collect(&harness(Firmware::Uefi).ctx())
            .expect("zbieranie danych");
        assert!(output.view.uses_key("module.hw_bios.note.ec"));
    }

    #[test]
    fn reports_legacy_firmware() {
        let output = BiosModule
            .collect(&harness(Firmware::Bios).ctx())
            .expect("zbieranie danych");
        assert!(output.view.uses_key("common.value.bios_legacy"));
        assert_eq!(output.data["firmware"], "bios");
    }

    #[test]
    fn formats_versions() {
        assert_eq!(format_version(Some(2), Some(8)), "2.8");
        assert_eq!(format_version(Some(5), None), "5");
        assert_eq!(format_version(None, None), "-");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = BiosModule
            .collect(&harness(Firmware::Uefi).ctx())
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
