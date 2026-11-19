//! Zapis wykresu do PNG — zrzut obszaru wykresu z bufora klatki egui.
//!
//! egui oddaje całą klatkę (okno w pikselach urządzenia), więc kadr liczymy
//! z proporcji: obszar w punktach → piksele. Dzięki temu działa też przy
//! skalowaniu Windows (125 %, 150 %), gdzie punkt ≠ piksel.

use std::io::BufWriter;
use std::path::Path;

use eframe::egui;

/// Wycina obszar `area` (punkty) z klatki `image` (piksele).
///
/// `screen` to cały obszar okna w punktach — skala liczy się z proporcji
/// `image.size / screen`, a nie z `pixels_per_point`, żeby nie zależeć od
/// zaokrągleń integracji. Zwraca `None`, gdy obszar leży poza klatką.
pub fn crop(
    image: &egui::ColorImage,
    screen: egui::Rect,
    area: egui::Rect,
) -> Option<egui::ColorImage> {
    let [width, height] = image.size;
    if width == 0 || height == 0 || screen.width() <= 0.0 || screen.height() <= 0.0 {
        return None;
    }

    let scale_x = width as f32 / screen.width();
    let scale_y = height as f32 / screen.height();
    let to_px = |value: f32, origin: f32, scale: f32, limit: usize| {
        ((value - origin) * scale).clamp(0.0, limit as f32).round() as usize
    };

    let x0 = to_px(area.min.x, screen.min.x, scale_x, width);
    let x1 = to_px(area.max.x, screen.min.x, scale_x, width);
    let y0 = to_px(area.min.y, screen.min.y, scale_y, height);
    let y1 = to_px(area.max.y, screen.min.y, scale_y, height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for row in y0..y1 {
        let start = row * width + x0;
        pixels.extend_from_slice(&image.pixels[start..start + (x1 - x0)]);
    }
    Some(egui::ColorImage::new([x1 - x0, y1 - y0], pixels))
}

/// Zapis obrazu jako PNG (RGB, 8 bitów na kanał).
pub fn write_png(path: &Path, image: &egui::ColorImage) -> Result<(), String> {
    let [width, height] = image.size;
    if width == 0 || height == 0 {
        return Err("pusty obraz".to_string());
    }

    let mut rgb = Vec::with_capacity(width * height * 3);
    for pixel in &image.pixels {
        let [red, green, blue, _] = pixel.to_srgba_unmultiplied();
        rgb.extend_from_slice(&[red, green, blue]);
    }

    let error = |err: png::EncodingError| format!("{}: {err}", path.display());
    let file = std::fs::File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(error)?;
    writer.write_image_data(&rgb).map_err(error)?;
    writer.finish().map_err(error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quadrants() -> egui::ColorImage {
        let mut pixels = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                pixels.push(egui::Color32::from_rgb(y, x, 0));
            }
        }
        egui::ColorImage::new([4, 4], pixels)
    }

    fn screen(points: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(points, points))
    }

    #[test]
    fn crop_takes_the_requested_region() {
        let out = crop(
            &quadrants(),
            screen(4.0),
            egui::Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(3.0, 3.0)),
        )
        .expect("kadr");
        assert_eq!(out.size, [2, 2]);
        assert_eq!(out.pixels[0], egui::Color32::from_rgb(1, 1, 0));
        assert_eq!(out.pixels[3], egui::Color32::from_rgb(2, 2, 0));
    }

    #[test]
    fn crop_scales_by_the_framebuffer_ratio() {
        // Okno 2×2 punkty, klatka 4×4 piksele (skalowanie 200 %): obszar
        // o boku 1 punktu to 2×2 piksele.
        let out = crop(
            &quadrants(),
            screen(2.0),
            egui::Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(2.0, 2.0)),
        )
        .expect("kadr");
        assert_eq!(out.size, [2, 2]);
        assert_eq!(out.pixels[0], egui::Color32::from_rgb(2, 2, 0));
        assert_eq!(out.pixels[3], egui::Color32::from_rgb(3, 3, 0));
    }

    #[test]
    fn crop_clamps_to_the_window_and_rejects_empty_areas() {
        // Obszar wychodzący za prawo i dół zostaje przycięty do klatki.
        let out = crop(
            &quadrants(),
            screen(4.0),
            egui::Rect::from_min_max(egui::pos2(2.0, 2.0), egui::pos2(9.0, 9.0)),
        )
        .expect("kadr");
        assert_eq!(out.size, [2, 2]);

        // Całkowicie poza oknem albo bez wymiaru — nie ma czego zapisać.
        assert!(
            crop(
                &quadrants(),
                screen(4.0),
                egui::Rect::from_min_max(egui::pos2(6.0, 0.0), egui::pos2(8.0, 2.0)),
            )
            .is_none()
        );
        assert!(
            crop(
                &quadrants(),
                screen(4.0),
                egui::Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(1.0, 1.0)),
            )
            .is_none()
        );
        assert!(crop(&quadrants(), screen(0.0), screen(0.0)).is_none());
    }

    #[test]
    fn write_png_produces_a_valid_header_and_pixels() {
        let path = std::env::temp_dir().join(format!(
            "serwiskit-chart-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        write_png(&path, &quadrants()).expect("zapis PNG");
        let bytes = std::fs::read(&path).expect("odczyt PNG");
        let _ = std::fs::remove_file(&path);

        assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10], "sygnatura");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!((width, height), (4, 4));
        // Nagłówek IHDR: 8 B sygnatury + 4 B długości + 4 B typu + 13 B danych.
        assert_eq!(&bytes[12..16], b"IHDR");
        assert!(bytes.len() > 4 * 4 * 3, "brak danych obrazu");
    }
}
