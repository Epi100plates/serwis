//! Wspólne pomocniki modułów: dekodowanie WMI i formatowanie wartości.
//!
//! Wszystko, co trafia do widoku jako wartość, przechodzi przez te funkcje —
//! dzięki temu „brak danych” wygląda tak samo we wszystkich modułach
//! i nigdzie nie pojawia się panika na nieoczekiwanym typie z WMI.

use serde::de::DeserializeOwned;
use serde_json::Value;
use sk_core::prelude::*;

/// Pierwszy wiersz odpowiedzi WMI jako struktura. Brak instancji to
/// `ModuleError::Unavailable`, a nie panika.
pub(crate) fn decode_first<T: DeserializeOwned>(rows: &[Value], class: &str) -> Result<T> {
    let row = rows
        .first()
        .ok_or_else(|| ModuleError::unavailable(format!("{class}: brak instancji")))?;
    decode_row(row, class)
}

/// Wszystkie wiersze odpowiedzi WMI. Błąd wskazuje klasę i indeks wiersza,
/// żeby dało się znaleźć wadliwy wpis bez debuggera.
pub(crate) fn decode_all<T: DeserializeOwned>(rows: &[Value], class: &str) -> Result<Vec<T>> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            serde_json::from_value(row.clone())
                .map_err(|err| ModuleError::provider("WMI", format!("{class}[{index}]: {err}")))
        })
        .collect()
}

fn decode_row<T: DeserializeOwned>(row: &Value, class: &str) -> Result<T> {
    serde_json::from_value(row.clone())
        .map_err(|err| ModuleError::provider("WMI", format!("{class}: {err}")))
}

/// Tekst do wyświetlenia: przycięty (WMI potrafi dodać spacje), `-` gdy brak.
pub(crate) fn text(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "-".to_string(),
    }
}

/// To samo, ale bez wartości zastępczej — do budowania kluczy JSON.
pub(crate) fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Liczba jako tekst; `-` gdy brak.
pub(crate) fn number<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

/// Klucz i18n dla wartości logicznej — teksty widoczne dla użytkownika żyją
/// wyłącznie w plikach tłumaczeń.
pub(crate) fn boolean_key(value: bool) -> &'static str {
    if value {
        "common.value.yes"
    } else {
        "common.value.no"
    }
}

/// Pojemność w GB (1024³) lub TB (1024⁴) — tak, jak pokazuje je Windows.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let gib = bytes as f64 / GIB;
    if gib >= 1024.0 {
        format!("{:.2} TB", gib / 1024.0)
    } else {
        format!("{gib:.1} GB")
    }
}

/// Rozmiar z jednostką dobraną do wielkości: od bajtów do TB.
///
/// [`format_bytes`] zawsze pokazuje GB (tak jak Windows), więc dla bloków
/// i zakresów (4 MiB, 512 MiB) dawałby „0.0 GB”. Tu jednostka rośnie razem
/// z wartością — do rozmiarów wewnątrz testu powierzchni.
pub(crate) fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.2} TB", value / TIB)
    } else if value >= GIB {
        format!("{:.1} GB", value / GIB)
    } else if value >= MIB {
        format!("{:.0} MB", value / MIB)
    } else if value >= KIB {
        format!("{:.0} KB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Rozmiar cache podany w KB przez WMI (`L2CacheSize`, `L3CacheSize`).
pub(crate) fn format_kib(kilobytes: u32) -> String {
    if kilobytes == 0 {
        return "-".to_string();
    }
    if kilobytes >= 1024 {
        format!("{:.1} MB", f64::from(kilobytes) / 1024.0)
    } else {
        format!("{kilobytes} KB")
    }
}

/// Data `YYYY-MM-DD` z formatu ISO 8601 albo WMI (`20230516020000.000000+120`).
/// Nieznany format wraca bez zmian — lepiej pokazać surową wartość niż zgadywać.
pub(crate) fn date_only(value: &str) -> String {
    let value = value.trim();
    let chars: Vec<char> = value.chars().collect();
    if chars.len() >= 10 && chars[4] == '-' {
        return chars[..10].iter().collect();
    }
    if chars.len() >= 8 && chars[..8].iter().all(char::is_ascii_digit) {
        return format!(
            "{}-{}-{}",
            chars[..4].iter().collect::<String>(),
            chars[4..6].iter().collect::<String>(),
            chars[6..8].iter().collect::<String>()
        );
    }
    value.to_string()
}

/// Wartość logiczna jako wiersz „etykieta → Tak/Nie”; brak danych to `-`.
pub(crate) fn bool_row(label_key: &str, value: Option<bool>) -> KeyValue {
    match value {
        Some(value) => KeyValue::state(label_key, boolean_key(value)),
        None => KeyValue::new(label_key, "-"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reports_missing_instance_with_class_name() {
        let error = decode_first::<serde_json::Value>(&[], "Win32_Fan").expect_err("brak danych");
        assert!(error.to_string().contains("Win32_Fan"), "{error}");
    }

    #[test]
    fn decode_all_points_at_bad_row() {
        let rows = vec![
            serde_json::json!({ "Size": 1 }),
            serde_json::json!({ "Size": "x" }),
        ];
        let error = decode_all::<DiskRow>(&rows, "Win32_DiskDrive").expect_err("zły typ");
        assert!(error.to_string().contains("Win32_DiskDrive[1]"), "{error}");
    }

    #[derive(Debug, serde::Deserialize)]
    struct DiskRow {
        #[serde(rename = "Size")]
        #[allow(dead_code)]
        size: u64,
    }

    #[test]
    fn text_trims_and_replaces_empty() {
        assert_eq!(text(Some("  AMD Ryzen  ")), "AMD Ryzen");
        assert_eq!(text(Some("   ")), "-");
        assert_eq!(text(None), "-");
        assert_eq!(trimmed(Some("  x ".to_string())), Some("x".to_string()));
        assert_eq!(trimmed(Some("  ".to_string())), None);
    }

    #[test]
    fn formats_capacity_like_windows() {
        assert_eq!(format_bytes(0), "0.0 GB");
        assert_eq!(format_bytes(512_105_932_800), "476.9 GB");
        assert_eq!(format_bytes(17_179_869_184), "16.0 GB");
        assert_eq!(format_bytes(2_000_398_934_016), "1.82 TB");
    }

    #[test]
    fn formats_size_with_a_unit_fitting_the_value() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1536), "2 KB");
        assert_eq!(format_size(8 << 20), "8 MB");
        assert_eq!(format_size(1536 << 20), "1.5 GB");
        assert_eq!(format_size(3 << 40), "3.00 TB");
    }

    #[test]
    fn formats_cache_size() {
        assert_eq!(format_kib(0), "-");
        assert_eq!(format_kib(512), "512 KB");
        assert_eq!(format_kib(3072), "3.0 MB");
        assert_eq!(format_kib(32768), "32.0 MB");
    }

    #[test]
    fn date_only_handles_iso_and_wmi_formats() {
        assert_eq!(date_only("2023-05-16T02:00:00+02:00"), "2023-05-16");
        assert_eq!(date_only("20230516020000.000000+120"), "2023-05-16");
        assert_eq!(date_only("nieznany"), "nieznany");
        assert_eq!(date_only(""), "");
    }

    #[test]
    fn bool_row_uses_translation_keys() {
        let row = bool_row("test.flag", Some(true));
        assert_eq!(row.value_key.as_deref(), Some("common.value.yes"));
        let row = bool_row("test.flag", Some(false));
        assert_eq!(row.value_key.as_deref(), Some("common.value.no"));
        let row = bool_row("test.flag", None);
        assert_eq!(row.value, "-");
    }
}
