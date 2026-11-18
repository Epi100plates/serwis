//! `hw.board` — płyta główna.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{bool_row, decode_first, text};

const BOARD_QUERY: &str = "SELECT Manufacturer, Product, Version, SerialNumber, Tag, \
     HostingBoard FROM Win32_BaseBoard";

#[derive(Debug, Default, Deserialize)]
struct BaseBoard {
    #[serde(rename = "Manufacturer", default)]
    manufacturer: Option<String>,
    #[serde(rename = "Product", default)]
    product: Option<String>,
    #[serde(rename = "Version", default)]
    version: Option<String>,
    #[serde(rename = "SerialNumber", default)]
    serial_number: Option<String>,
    #[serde(rename = "Tag", default)]
    tag: Option<String>,
    #[serde(rename = "HostingBoard", default)]
    hosting_board: Option<bool>,
}

#[derive(Debug, Default)]
pub struct Board;

impl Module for Board {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.board",
            name_key: "module.hw_board.name",
            desc_key: "module.hw_board.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let board: BaseBoard =
            decode_first(&ctx.wmi.query(NS_CIMV2, BOARD_QUERY)?, "Win32_BaseBoard")?;

        let view = View::new(vec![Section::new("module.hw_board.section").with_blocks(
            vec![Block::key_values(vec![
                KeyValue::new(
                    "module.hw_board.manufacturer",
                    text(board.manufacturer.as_deref()),
                ),
                KeyValue::new("module.hw_board.product", text(board.product.as_deref())),
                KeyValue::new("module.hw_board.version", text(board.version.as_deref())),
                KeyValue::new(
                    "module.hw_board.serial",
                    text(board.serial_number.as_deref()),
                ),
                bool_row("module.hw_board.hosting", board.hosting_board),
                KeyValue::new("module.hw_board.tag", text(board.tag.as_deref())),
            ])],
        )]);

        let data = serde_json::json!({
            "schema_version": 1,
            "manufacturer": crate::util::trimmed(board.manufacturer),
            "product": crate::util::trimmed(board.product),
            "version": crate::util::trimmed(board.version),
            "serial_number": crate::util::trimmed(board.serial_number),
            "hosting_board": board.hosting_board,
        });

        Ok(ModuleOutput { data, view })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::i18n::Translator;
    use sk_core::testing::{Harness, MockWmi};

    fn harness() -> Harness {
        let wmi =
            MockWmi::new().with_rows("Win32_BaseBoard", fixture_rows!("win32_baseboard.json"));
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn renders_board_data_from_fixture() {
        let output = Board.collect(&harness().ctx()).expect("zbieranie danych");
        assert_eq!(
            output.data["manufacturer"],
            "Micro-Star International Co., Ltd."
        );
        assert_eq!(output.data["product"], "A320M-A PRO M2 (MS-7C52)");
        assert_eq!(output.data["version"], "1.0");
        assert_eq!(output.data["hosting_board"], true);
    }

    #[test]
    fn missing_board_is_an_error_with_class_name() {
        let error = Board
            .collect(&Harness::new().ctx())
            .expect_err("brak płyty głównej");
        assert!(error.to_string().contains("Win32_BaseBoard"), "{error}");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let output = Board.collect(&harness().ctx()).expect("zbieranie danych");
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
