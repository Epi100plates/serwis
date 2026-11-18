//! Pomocniki wspólne dla testów binarki: fixtures odpowiedzi WMI z `docs/`.

/// Wczytuje fixture WMI jako wiersze JSON. Ścieżki muszą być literałami,
/// bo `include_str!` wkompilowuje plik w binarkę testową.
pub fn fixture_rows(name: &str) -> Vec<serde_json::Value> {
    let text = match name {
        "win32_operatingsystem.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_operatingsystem.json"
        )),
        "win32_computersystem.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_computersystem.json"
        )),
        "win32_perfformatteddata_perfos_processor.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/win32_perfformatteddata_perfos_processor.json"
        )),
        "wmi_processorperformance.json" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/wmi/wmi_processorperformance.json"
        )),
        other => panic!("brak fixture WMI: {other}"),
    };

    serde_json::from_str(text).expect("fixture WMI musi być poprawnym JSON-em")
}
