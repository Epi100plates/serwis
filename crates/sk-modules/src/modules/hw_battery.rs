//! `hw.battery` — bateria: pojemność projektowa i rzeczywista, zużycie,
//! napięcie, moc ładowania/rozładowania.
//!
//! Źródłem są klasy WMI (`BatteryStaticData`, `BatteryFullChargedCapacity`,
//! `BatteryStatus`, `Win32_Battery`) — działają bez uprawnień administratora.
//! Szczegóły dostępne tylko przez `IOCTL_BATTERY_QUERY_INFORMATION` (m.in. liczba
//! cykli) dochodzą w późniejszej fazie; do tego czasu moduł mówi o braku wprost.

use serde::Deserialize;
use sk_core::prelude::*;

use crate::util::{decode_first, text};

const STATIC_QUERY: &str = "SELECT DesignedCapacity, DeviceName, ManufactureName, Chemistry, \
     SerialNumber FROM BatteryStaticData";
const FULL_QUERY: &str = "SELECT FullChargedCapacity FROM BatteryFullChargedCapacity";
const STATUS_QUERY: &str = "SELECT RemainingCapacity, Voltage, Rate, Charging, Discharging, \
     PowerOnline, Critical FROM BatteryStatus";
const WIN32_QUERY: &str = "SELECT Name, EstimatedChargeRemaining, BatteryStatus, DesignVoltage \
     FROM Win32_Battery";

#[derive(Debug, Default, Deserialize)]
struct BatteryStatic {
    #[serde(rename = "DesignedCapacity", default)]
    designed_capacity_mwh: Option<u32>,
    #[serde(rename = "DeviceName", default)]
    device_name: Option<String>,
    #[serde(rename = "ManufactureName", default)]
    manufacture_name: Option<String>,
    #[serde(rename = "Chemistry", default)]
    chemistry: Option<u32>,
    #[serde(rename = "SerialNumber", default)]
    serial_number: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BatteryFull {
    #[serde(rename = "FullChargedCapacity", default)]
    full_charged_capacity_mwh: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct BatteryStatus {
    #[serde(rename = "RemainingCapacity", default)]
    remaining_capacity_mwh: Option<u32>,
    #[serde(rename = "Voltage", default)]
    voltage_mv: Option<u32>,
    #[serde(rename = "Rate", default)]
    rate_mw: Option<u32>,
    #[serde(rename = "Charging", default)]
    charging: Option<bool>,
    #[serde(rename = "Discharging", default)]
    discharging: Option<bool>,
    #[serde(rename = "PowerOnline", default)]
    power_online: Option<bool>,
    #[serde(rename = "Critical", default)]
    critical: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct Win32Battery {
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "EstimatedChargeRemaining", default)]
    estimated_charge_percent: Option<u16>,
    #[serde(rename = "BatteryStatus", default)]
    status: Option<u16>,
    #[serde(rename = "DesignVoltage", default)]
    design_voltage_mv: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Battery;

impl Module for Battery {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "hw.battery",
            name_key: "module.hw_battery.name",
            desc_key: "module.hw_battery.desc",
            group: Group::Hardware,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        // Na desktopie klasy baterii po prostu nie istnieją — to nie błąd.
        let static_data: Option<BatteryStatic> = ctx
            .wmi
            .query(NS_WMI, STATIC_QUERY)
            .ok()
            .and_then(|rows| decode_first(&rows, "BatteryStaticData").ok());
        let full: Option<BatteryFull> = ctx
            .wmi
            .query(NS_WMI, FULL_QUERY)
            .ok()
            .and_then(|rows| decode_first(&rows, "BatteryFullChargedCapacity").ok());
        let status: Option<BatteryStatus> = ctx
            .wmi
            .query(NS_WMI, STATUS_QUERY)
            .ok()
            .and_then(|rows| decode_first(&rows, "BatteryStatus").ok());
        let win32: Option<Win32Battery> = ctx
            .wmi
            .query(NS_CIMV2, WIN32_QUERY)
            .ok()
            .and_then(|rows| decode_first(&rows, "Win32_Battery").ok());

        let design_mwh = static_data
            .as_ref()
            .and_then(|data| data.designed_capacity_mwh);
        let full_mwh = full
            .as_ref()
            .and_then(|data| data.full_charged_capacity_mwh);
        let present =
            design_mwh.is_some() || full_mwh.is_some() || status.is_some() || win32.is_some();

        if !present {
            return Ok(ModuleOutput {
                data: serde_json::json!({
                    "schema_version": 1,
                    "present": false,
                }),
                view: View::new(vec![Section::new("module.hw_battery.section").with_blocks(
                    vec![
                        Block::note(NoteLevel::Warn, "module.hw_battery.none"),
                        Block::note(NoteLevel::Info, "module.hw_battery.note.desktop"),
                    ],
                )]),
            });
        }

        let wear_percent = match (design_mwh, full_mwh) {
            (Some(design), Some(full)) if design > 0 => {
                let worn = 100.0 - (f64::from(full) / f64::from(design) * 100.0);
                Some(worn.clamp(0.0, 100.0))
            }
            _ => None,
        };
        let charge_percent = match (
            full_mwh,
            status.as_ref().and_then(|s| s.remaining_capacity_mwh),
        ) {
            (Some(full), Some(remaining)) if full > 0 => {
                Some((f64::from(remaining) / f64::from(full) * 100.0).clamp(0.0, 100.0))
            }
            _ => win32
                .as_ref()
                .and_then(|battery| battery.estimated_charge_percent)
                .map(f64::from),
        };
        let state_key = battery_state_key(status.as_ref(), win32.as_ref());

        let name = static_data
            .as_ref()
            .and_then(|data| data.device_name.clone())
            .or_else(|| win32.as_ref().and_then(|battery| battery.name.clone()));

        let rows = vec![
            KeyValue::new("module.hw_battery.device", text(name.as_deref())),
            KeyValue::new("module.hw_battery.design_capacity", format_mwh(design_mwh)),
            KeyValue::new("module.hw_battery.full_capacity", format_mwh(full_mwh)),
            KeyValue::new(
                "module.hw_battery.wear",
                wear_percent.map_or_else(|| "-".to_string(), |wear| format!("{wear:.1} %")),
            ),
            KeyValue::new(
                "module.hw_battery.charge",
                charge_percent.map_or_else(|| "-".to_string(), |charge| format!("{charge:.0} %")),
            ),
            KeyValue::new(
                "module.hw_battery.voltage",
                status
                    .as_ref()
                    .and_then(|status| status.voltage_mv)
                    .or_else(|| win32.as_ref().and_then(|battery| battery.design_voltage_mv))
                    .map_or_else(
                        || "-".to_string(),
                        |mv| format!("{:.2} V", f64::from(mv) / 1000.0),
                    ),
            ),
            KeyValue::new(
                "module.hw_battery.rate",
                status
                    .as_ref()
                    .and_then(|status| status.rate_mw)
                    .map_or_else(|| "-".to_string(), |mw| format!("{mw} mW")),
            ),
            KeyValue::state("module.hw_battery.state", state_key),
            KeyValue::new(
                "module.hw_battery.vendor",
                text(
                    static_data
                        .as_ref()
                        .and_then(|data| data.manufacture_name.as_deref()),
                ),
            ),
            KeyValue::new(
                "module.hw_battery.chemistry",
                chemistry_name(static_data.as_ref().and_then(|data| data.chemistry)),
            ),
        ];

        let mut blocks = vec![
            Block::key_values(rows),
            Block::note(NoteLevel::Info, "module.hw_battery.note.cycles"),
        ];
        if status
            .as_ref()
            .and_then(|status| status.critical)
            .unwrap_or(false)
        {
            blocks.push(Block::note(
                NoteLevel::Error,
                "module.hw_battery.note.critical",
            ));
        }

        let view = View::new(vec![
            Section::new("module.hw_battery.section").with_blocks(blocks),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "present": true,
            "name": crate::util::trimmed(name),
            "designed_capacity_mwh": design_mwh,
            "full_charged_capacity_mwh": full_mwh,
            "wear_percent": wear_percent,
            "charge_percent": charge_percent,
            "voltage_mv": status.as_ref().and_then(|s| s.voltage_mv),
            "rate_mw": status.as_ref().and_then(|s| s.rate_mw),
            "charging": status.as_ref().and_then(|s| s.charging),
            "discharging": status.as_ref().and_then(|s| s.discharging),
            "power_online": status.as_ref().and_then(|s| s.power_online),
            "critical": status.as_ref().and_then(|s| s.critical),
            "manufacturer": crate::util::trimmed(
                static_data.as_ref().and_then(|data| data.manufacture_name.clone())
            ),
            "chemistry": static_data.as_ref().and_then(|data| data.chemistry),
            "serial_number": crate::util::trimmed(
                static_data.as_ref().and_then(|data| data.serial_number.clone())
            ),
        });

        Ok(ModuleOutput { data, view })
    }
}

fn battery_state_key(status: Option<&BatteryStatus>, win32: Option<&Win32Battery>) -> &'static str {
    if let Some(status) = status {
        if status.charging.unwrap_or(false) {
            return "module.hw_battery.state.charging";
        }
        if status.discharging.unwrap_or(false) {
            return "module.hw_battery.state.discharging";
        }
        if status.power_online.unwrap_or(false) {
            return "module.hw_battery.state.idle_ac";
        }
    }
    // `Win32_Battery.BatteryStatus = 2` oznacza zasilanie z sieci.
    if win32.and_then(|battery| battery.status) == Some(2) {
        return "module.hw_battery.state.idle_ac";
    }
    "common.value.unknown"
}

fn format_mwh(value: Option<u32>) -> String {
    value.map_or_else(
        || "-".to_string(),
        |mwh| format!("{mwh} mWh ({:.1} Wh)", f64::from(mwh) / 1000.0),
    )
}

/// Kody `BatteryStaticData.Chemistry` (2 = SLA, 6 = Li-ion, 7 = Li-poly,
/// 8 = NiMH, 9 = NiCd).
fn chemistry_name(code: Option<u32>) -> String {
    match code {
        Some(2) => "SLA".to_string(),
        Some(3) => "NiCd".to_string(),
        Some(6) => "Li-ion".to_string(),
        Some(7) => "Li-poly".to_string(),
        Some(8) => "NiMH".to_string(),
        Some(9) => "NiCd".to_string(),
        Some(other) => format!("WMI #{other}"),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::{Harness, MockWmi};

    fn laptop_harness() -> Harness {
        let wmi = MockWmi::new()
            .with_rows(
                "BatteryStaticData",
                fixture_rows!("wmi_batterystaticdata_synthetic.json"),
            )
            .with_rows(
                "BatteryFullChargedCapacity",
                fixture_rows!("wmi_batteryfullchargedcapacity_synthetic.json"),
            )
            .with_rows(
                "BatteryStatus",
                fixture_rows!("wmi_batterystatus_synthetic.json"),
            )
            .with_rows(
                "Win32_Battery",
                fixture_rows!("win32_battery_synthetic.json"),
            );
        Harness::new().with_wmi(wmi)
    }

    #[test]
    fn desktop_without_battery_shows_note_not_error() {
        let output = Battery
            .collect(&Harness::new().ctx())
            .expect("brak baterii to nie błąd");
        assert_eq!(output.data["present"], false);
        assert!(output.view.uses_key("module.hw_battery.none"));
        assert!(output.view.uses_key("module.hw_battery.note.desktop"));
    }

    #[test]
    fn laptop_fixture_computes_wear_and_charge() {
        let output = Battery
            .collect(&laptop_harness().ctx())
            .expect("zbieranie danych");
        assert_eq!(output.data["present"], true);
        assert_eq!(output.data["designed_capacity_mwh"], 57_000);
        assert_eq!(output.data["full_charged_capacity_mwh"], 48_210);
        let wear = output.data["wear_percent"]
            .as_f64()
            .expect("procent zużycia");
        assert!((wear - 15.42).abs() < 0.01, "zużycie {wear}");
        let charge = output.data["charge_percent"].as_f64().expect("naładowanie");
        assert!((charge - 80.9).abs() < 0.5, "naładowanie {charge}");
        assert_eq!(output.data["power_online"], true);
    }

    #[test]
    fn states_charging_and_discharging() {
        let charging = BatteryStatus {
            charging: Some(true),
            ..BatteryStatus::default()
        };
        assert_eq!(
            battery_state_key(Some(&charging), None),
            "module.hw_battery.state.charging"
        );
        let discharging = BatteryStatus {
            discharging: Some(true),
            ..BatteryStatus::default()
        };
        assert_eq!(
            battery_state_key(Some(&discharging), None),
            "module.hw_battery.state.discharging"
        );
        let win32 = Win32Battery {
            status: Some(2),
            ..Win32Battery::default()
        };
        assert_eq!(
            battery_state_key(None, Some(&win32)),
            "module.hw_battery.state.idle_ac"
        );
        assert_eq!(battery_state_key(None, None), "common.value.unknown");
    }

    #[test]
    fn maps_chemistry_codes() {
        assert_eq!(chemistry_name(Some(6)), "Li-ion");
        assert_eq!(chemistry_name(Some(7)), "Li-poly");
        assert_eq!(chemistry_name(Some(42)), "WMI #42");
        assert_eq!(chemistry_name(None), "-");
    }

    #[test]
    fn all_view_keys_exist_in_both_locales() {
        let translator = Translator::load().expect("tłumaczenia");
        for harness in [laptop_harness(), Harness::new()] {
            let output = Battery.collect(&harness.ctx()).expect("zbieranie danych");
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
}
