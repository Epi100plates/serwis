//! Minimalny parser EDID 1.x dla modułu `hw.monitor`.
//!
//! WMI nie podaje rozdzielczości natywnej ani przekątnej w calach — te dane są
//! tylko w bloku EDID monitora (`HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\...`).
//! Parser czyta bazowy blok 128 B: producenta, model, rok/tydzień, rozmiar
//! fizyczny, pierwszy timing szczegółowy (tryb natywny) i limity odświeżania.
//!
//! Świadomie nie obsługuje HDR, CTA-861 i rozszerzeń poza policzeniem ich liczby.

/// Dane wyciągnięte z EDID. Wszystkie pola poza producentem są opcjonalne,
/// bo monitory potrafią mieć niepełne deskryptory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EdidInfo {
    /// Trzyliterowy kod producenta, np. `SAM`, `GBT`.
    pub manufacturer: String,
    /// Kod produktu z EDID (dla `SAM0928` = `0x0928`).
    pub product_code: u16,
    pub serial: u32,
    pub year: u16,
    pub week: u8,
    /// Rozmiar widoczny w centymetrach `(szerokość, wysokość)`.
    pub size_cm: Option<(u8, u8)>,
    /// Tryb natywny z pierwszego timing-u szczegółowego.
    pub native: Option<(u16, u16)>,
    /// Częstotliwość odświeżania trybu natywnego (Hz), wyliczona z pixel clocku.
    pub native_refresh_hz: Option<u32>,
    /// Górna granica z deskryptora „Display Range Limits” (Hz).
    pub max_refresh_hz: Option<u32>,
    /// Wejście cyfrowe (HDMI/DP/DVI) vs analogowe (VGA).
    pub digital: bool,
    pub extensions: u8,
    pub checksum_ok: bool,
}

impl EdidInfo {
    /// Przekątna w calach, zaokrąglona do 0,1".
    pub fn diagonal_inches(&self) -> Option<f64> {
        let (width, height) = self.size_cm?;
        if width == 0 || height == 0 {
            return None;
        }
        let diagonal_cm = ((width as f64).powi(2) + (height as f64).powi(2)).sqrt();
        Some((diagonal_cm / 2.54 * 10.0).round() / 10.0)
    }
}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
const DESCRIPTORS_START: usize = 54;
const DESCRIPTOR_LEN: usize = 18;

pub fn parse(bytes: &[u8]) -> Option<EdidInfo> {
    if bytes.len() < 128 || bytes[0..8] != HEADER {
        return None;
    }

    let manufacturer_code = u16::from_be_bytes([bytes[8], bytes[9]]);
    let product_code = u16::from_le_bytes([bytes[10], bytes[11]]);
    let serial = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let week = bytes[16];
    let year = u16::from(bytes[17]).checked_add(1990)?;

    let size_cm = {
        let width = bytes[21];
        let height = bytes[22];
        if width > 0 && height > 0 {
            Some((width, height))
        } else {
            None
        }
    };

    let mut native = None;
    let mut native_refresh_hz = None;
    let mut max_refresh_hz = None;

    for index in 0..4 {
        let start = DESCRIPTORS_START + index * DESCRIPTOR_LEN;
        let end = start + DESCRIPTOR_LEN;
        let Some(descriptor) = bytes.get(start..end) else {
            break;
        };

        if descriptor[0] == 0x00 && descriptor[1] == 0x00 {
            if descriptor[3] == 0xFD {
                max_refresh_hz = Some(u32::from(descriptor[6]));
            }
            continue;
        }

        // Timing szczegółowy — pierwszy opisuje tryb natywny.
        if native.is_none() {
            let pixel_clock_khz =
                u32::from(u16::from_le_bytes([descriptor[0], descriptor[1]])) * 10;
            let horizontal_active =
                u16::from(descriptor[2]) | (u16::from(descriptor[4] & 0xF0) << 4);
            let horizontal_blank =
                u16::from(descriptor[3]) | (u16::from(descriptor[4] & 0x0F) << 8);
            let vertical_active = u16::from(descriptor[5]) | (u16::from(descriptor[7] & 0xF0) << 4);
            let vertical_blank = u16::from(descriptor[6]) | (u16::from(descriptor[7] & 0x0F) << 8);

            if horizontal_active > 0 && vertical_active > 0 {
                native = Some((horizontal_active, vertical_active));
                let total_pixels = u32::from(horizontal_active + horizontal_blank)
                    * u32::from(vertical_active + vertical_blank);
                if total_pixels > 0 {
                    let refresh =
                        (pixel_clock_khz as f64 * 1000.0 / f64::from(total_pixels)).round();
                    if refresh.is_finite() && refresh > 0.0 && refresh < 1000.0 {
                        native_refresh_hz = Some(refresh as u32);
                    }
                }
            }
        }
    }

    let checksum_ok = bytes[..128]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
        == 0;

    Some(EdidInfo {
        manufacturer: decode_manufacturer(manufacturer_code),
        product_code,
        serial,
        year,
        week,
        size_cm,
        native,
        native_refresh_hz,
        max_refresh_hz,
        digital: bytes[20] & 0x80 != 0,
        extensions: bytes[126],
        checksum_ok,
    })
}

/// Trzy litery producenta: 3 × 5 bitów, `1 = A`.
fn decode_manufacturer(code: u16) -> String {
    let mut letters = String::with_capacity(3);
    for shift in [10, 5, 0] {
        let value = ((code >> shift) & 0x1F) as u8;
        if (1..=26).contains(&value) {
            letters.push(char::from(b'A' + value - 1));
        } else {
            letters.push('?');
        }
    }
    letters
}

/// Zamiana zapisu hex na bajty — używane przez testy fixtures EDID.
#[cfg(test)]
pub(crate) fn hex_to_bytes(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_digit(16).unwrap_or(0) as u8)
        .collect();
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(hex: &str) -> Vec<u8> {
        hex_to_bytes(hex)
    }

    fn samsung() -> EdidInfo {
        parse(&fixture(include_str!(
            "../../../docs/fixtures/edid/sam0928.hex"
        )))
        .expect("EDID Samsunga się parsuje")
    }

    fn gigabyte() -> EdidInfo {
        parse(&fixture(include_str!(
            "../../../docs/fixtures/edid/gbt2730.hex"
        )))
        .expect("EDID Gigabyte'a się parsuje")
    }

    #[test]
    fn decodes_samsung_monitor() {
        let info = samsung();
        assert_eq!(info.manufacturer, "SAM");
        assert_eq!(info.product_code, 0x0928);
        assert_eq!((info.year, info.week), (2010, 46));
        assert_eq!(info.size_cm, Some((41, 23)));
        assert_eq!(info.native, Some((1366, 768)));
        assert_eq!(info.native_refresh_hz, Some(60));
        assert_eq!(info.max_refresh_hz, Some(75));
        assert!(info.digital);
        assert!(info.checksum_ok);
        assert_eq!(info.extensions, 1);
        assert_eq!(info.diagonal_inches(), Some(18.5));
    }

    #[test]
    fn decodes_gigabyte_monitor() {
        let info = gigabyte();
        assert_eq!(info.manufacturer, "GBT");
        assert_eq!(info.product_code, 0x2730);
        assert_eq!((info.year, info.week), (2025, 6));
        assert_eq!(info.size_cm, Some((60, 33)));
        assert_eq!(info.native, Some((2560, 1440)));
        assert_eq!(info.native_refresh_hz, Some(60));
        assert_eq!(info.max_refresh_hz, Some(180));
        assert_eq!(info.extensions, 2);
        assert!(info.checksum_ok);
        assert_eq!(info.diagonal_inches(), Some(27.0));
    }

    #[test]
    fn rejects_short_or_foreign_block() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0u8; 64]).is_none());
        assert!(parse(&[0x12; 128]).is_none());
    }

    #[test]
    fn decodes_manufacturer_code() {
        assert_eq!(decode_manufacturer(0x4C2D), "SAM");
        assert_eq!(decode_manufacturer(0x1C54), "GBT");
        assert_eq!(decode_manufacturer(0), "???");
    }
}
