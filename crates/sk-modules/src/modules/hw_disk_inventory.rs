//! `hw.disk_inventory` — spis dysków: model, interfejs, rozmiar, sektor, firmware.
//!
//! To inwentarz, nie diagnostyka: SMART i testy powierzchni są osobnymi modułami
//! (Faza 2), bo wymagają uprawnień administratora i długich operacji.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{decode_all, format_bytes, number, text};

const DISK_QUERY: &str = "SELECT Index, Model, InterfaceType, MediaType, Size, BytesPerSector, \
     FirmwareRevision, SerialNumber, Partitions, Status, PNPDeviceID FROM Win32_DiskDrive";

#[derive(Debug, Default, Deserialize)]
struct DiskDrive {
    #[serde(rename = "Index", default)]
    index: Option<u32>,
    #[serde(rename = "Model", default)]
    model: Option<String>,
    #[serde(rename = "InterfaceType", default)]
    interface_type: Option<String>,
    #[serde(rename = "MediaType", default)]
    media_type: Option<String>,
    #[serde(rename = "Size", default)]
    size_bytes: Option<u64>,
    #[serde(rename = "BytesPerSector", default)]
    bytes_per_sector: Option<u32>,
    #[serde(rename = "FirmwareRevision", default)]
    firmware: Option<String>,
    #[serde(rename = "SerialNumber", default)]
    serial_number: Option<String>,
    #[serde(rename = "Partitions", default)]
    partitions: Option<u32>,
    #[serde(rename = "Status", default)]
    status: Option<String>,
}

#[derive(Debug, Default)]
pub struct DiskInventory;

impl Module for DiskInventory {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.disk_inventory",
            name_key: "module.hw_disk_inventory.name",
            desc_key: "module.hw_disk_inventory.desc",
            group: Group::Storage,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let disks: Vec<DiskDrive> =
            decode_all(&ctx.wmi.query(NS_CIMV2, DISK_QUERY)?, "Win32_DiskDrive")?;

        if disks.is_empty() {
            return Ok(ModuleOutput {
                data: serde_json::json!({ "schema_version": 1, "disks": [] }),
                view: View::new(vec![
                    Section::new("module.hw_disk_inventory.section").with_blocks(vec![
                        Block::note(NoteLevel::Warn, "module.hw_disk_inventory.note.none"),
                    ]),
                ]),
            });
        }

        let mut rows = Vec::new();
        let mut data_disks = Vec::new();
        let mut has_usb = false;

        for disk in &disks {
            let interface = text(disk.interface_type.as_deref());
            if interface.eq_ignore_ascii_case("USB") {
                has_usb = true;
            }
            rows.push(vec![
                number(disk.index),
                text(disk.model.as_deref()),
                interface.clone(),
                disk.size_bytes
                    .map_or_else(|| "-".to_string(), format_bytes),
                disk.bytes_per_sector
                    .map_or_else(|| "-".to_string(), |bytes| format!("{bytes} B")),
                text(disk.firmware.as_deref()),
                text(disk.serial_number.as_deref()),
                number(disk.partitions),
                text(disk.status.as_deref()),
            ]);

            data_disks.push(serde_json::json!({
                "index": disk.index,
                "model": crate::util::trimmed(disk.model.clone()),
                "interface_type": crate::util::trimmed(disk.interface_type.clone()),
                "media_type": crate::util::trimmed(disk.media_type.clone()),
                "size_bytes": disk.size_bytes,
                "bytes_per_sector": disk.bytes_per_sector,
                "firmware": crate::util::trimmed(disk.firmware.clone()),
                "serial_number": crate::util::trimmed(disk.serial_number.clone()),
                "partitions": disk.partitions,
                "status": crate::util::trimmed(disk.status.clone()),
            }));
        }

        let mut blocks = vec![Block::Table {
            table: Table::new(
                vec![
                    "module.hw_disk_inventory.col.index".to_string(),
                    "module.hw_disk_inventory.col.model".to_string(),
                    "module.hw_disk_inventory.col.interface".to_string(),
                    "module.hw_disk_inventory.col.size".to_string(),
                    "module.hw_disk_inventory.col.sector".to_string(),
                    "module.hw_disk_inventory.col.firmware".to_string(),
                    "module.hw_disk_inventory.col.serial".to_string(),
                    "module.hw_disk_inventory.col.partitions".to_string(),
                    "module.hw_disk_inventory.col.status".to_string(),
                ],
                rows,
            ),
        }];

        if has_usb {
            blocks.push(Block::note(
                NoteLevel::Info,
                "module.hw_disk_inventory.note.usb",
            ));
        }

        let view = View::new(vec![
            Section::new("module.hw_disk_inventory.section").with_blocks(blocks),
        ]);

        let total_bytes: u64 = disks.iter().filter_map(|disk| disk.size_bytes).sum();
        let data = serde_json::json!({
            "schema_version": 1,
            "disks_total": disks.len(),
            "total_bytes": total_bytes,
            "disks": data_disks,
        });

        Ok(ModuleOutput { data, view })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockWmi};

    fn harness() -> Harness {
        let wmi =
            MockWmi::new().with_rows("Win32_DiskDrive", fixture_rows!("win32_diskdrive.json"));
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn lists_all_four_drives_from_fixture() {
        let output = DiskInventory
            .collect(&harness().ctx())
            .expect("zbieranie danych");
        assert_eq!(output.data["disks_total"], 4);
        let disks = output.data["disks"].as_array().expect("lista dysków");
        assert_eq!(disks[0]["model"], "WDC PC SN530 SDBPNPZ-512G-1114");
        assert_eq!(disks[0]["size_bytes"], 512_105_932_800u64);
        assert_eq!(disks[0]["firmware"], "21113000");
        assert_eq!(disks[3]["model"], "CT1000MX500SSD1");
        assert_eq!(disks[3]["partitions"], 1);
    }

    #[test]
    fn handles_removable_device_without_size() {
        let output = DiskInventory
            .collect(&harness().ctx())
            .expect("zbieranie danych");
        let disks = output.data["disks"].as_array().expect("lista dysków");
        let usb = disks
            .iter()
            .find(|disk| disk["interface_type"] == "USB")
            .expect("dysk USB w fixtures");
        assert!(usb["size_bytes"].is_null());
        assert!(output.view.uses_key("module.hw_disk_inventory.note.usb"));
    }

    #[test]
    fn renders_table_with_headers() {
        let output = DiskInventory
            .collect(&harness().ctx())
            .expect("zbieranie danych");
        let table = output.view.sections[0]
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { table } => Some(table),
                _ => None,
            })
            .expect("tabela dysków");
        assert_eq!(table.headers.len(), 9);
        assert_eq!(table.rows.len(), 4);
        assert_eq!(table.rows[0][3], "476.9 GB");
        assert_eq!(table.rows[0][4], "512 B");
    }

    #[test]
    fn empty_list_shows_note() {
        let wmi = MockWmi::new().with_rows("Win32_DiskDrive", vec![]);
        let output = DiskInventory
            .collect(&Harness::new().with_wmi(wmi).ctx())
            .expect("pusta lista to nie błąd");
        assert!(output.view.uses_key("module.hw_disk_inventory.note.none"));
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = DiskInventory
            .collect(&harness().ctx())
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
