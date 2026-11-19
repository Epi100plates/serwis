//! Skan nieczytelnych sektorów: transport odczytu jednostek i rozrzuconych
//! próbek.
//!
//! To wyłącznie transport — plan przebiegu, typy wyniku i mapa powierzchni
//! żyją w [`sk_core::sector`], a widok w module `storage.sectors`. Skan tylko
//! czyta: nie zapisuje na dysk ani nie zmienia danych.
//!
//! Błędy pojedynczych odczytów są **danymi**, nie powodem przerwania: lista ma
//! pokazać, gdzie dysk nie odpowiada, a nie zniknąć w połowie przebiegu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use sk_core::error::{ModuleError, Result};
use sk_core::sector::{
    MAX_SECTOR_ERRORS, SECTOR_UNIT_BYTES, SectorError, SectorParams, SectorPlan, SectorProgress,
    SectorScan, plan,
};

use crate::surface::{ALIGNMENT, RawDevice, alignment_offset};

/// Opis błędu, gdy `ReadFile` nie zwrócił błędu, a mimo to nie odczytał bajtów.
const ZERO_READ_DETAIL: &str = "odczytano 0 B";

/// Skan nieczytelnych sektorów dysku `params.drive` — tylko odczyt.
///
/// Tryb dokładny czyta każdą jednostkę [`SECTOR_UNIT_BYTES`] od początku
/// zakresu, tryb szybki — rozrzucone próbki z [`sk_core::sector::quick_probes`].
/// Jedno pozycjonowanie na start przebiegu (tryb dokładny) albo na próbkę
/// (tryb szybki), bo kolejne `ReadFile` same przesuwają wskaźnik urządzenia.
///
/// `on_progress` jest wołane po każdej zakończonej jednostce — także po
/// nieudanym odczycie, bo UI rysuje mapę z kroków, nie z samych sukcesów.
///
/// Przerwanie (`cancel`) zwraca dane częściowe z `cancelled: true`, nie błąd.
/// Błędem jest tylko to, czego nie da się dalej czytać: otwarcie urządzenia,
/// jego rozmiar i pozycjonowanie.
pub fn scan_sectors(
    params: &SectorParams,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(SectorProgress<'_>),
) -> Result<SectorScan> {
    let device = RawDevice::open(params.drive)?;
    let disk_bytes = device.size()?;
    scan_device(&device, params, disk_bytes, cancel, on_progress)
}

/// Przebieg na otwartym urządzeniu — wydzielony, żeby testy mogły podstawić
/// plik tymczasowy zamiast dysku fizycznego.
fn scan_device(
    device: &RawDevice,
    params: &SectorParams,
    disk_bytes: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(SectorProgress<'_>),
) -> Result<SectorScan> {
    let effective = params.clamped_to(disk_bytes);
    if effective.span_bytes == 0 {
        return Err(ModuleError::unavailable(
            "zakres mniejszy niż jedna jednostka",
        ));
    }

    let unit = usize::try_from(SECTOR_UNIT_BYTES)
        .map_err(|_| ModuleError::internal("jednostka skanu nie mieści się w pamięci procesu"))?;
    // Zapas na wyrównanie — ten sam wzorzec co w `surface.rs`: `Vec<u8>` nie ma
    // gwarantowanego adresu, a `FILE_FLAG_NO_BUFFERING` wymaga go od bufora.
    let mut buffer = vec![0u8; unit + ALIGNMENT];
    let start = alignment_offset(buffer.as_ptr() as usize);
    let block = &mut buffer[start..start + unit];

    let run_plan = plan(&effective);
    let started = Instant::now();
    let mut run = Run {
        steps_done: 0,
        steps_total: run_plan.steps(),
        bytes_read: 0,
        errors: Vec::new(),
        errors_truncated: false,
        cancelled: false,
        unit_bytes: SECTOR_UNIT_BYTES,
    };

    match run_plan {
        SectorPlan::Full { units } => {
            // Jedno pozycjonowanie na cały przebieg: kolejne `ReadFile` same
            // przesuwają wskaźnik, więc seek na każdą jednostkę byłby zbędny.
            // Wyjątkiem jest odczyt nieudany — patrz `Run::read_unit`.
            device.seek(0)?;
            for index in 0..units {
                if cancel.load(Ordering::Relaxed) {
                    run.cancelled = true;
                    break;
                }
                let offset = index.saturating_mul(SECTOR_UNIT_BYTES);
                let next = if index + 1 < units {
                    Some(offset + SECTOR_UNIT_BYTES)
                } else {
                    None
                };
                run.read_unit(device, block, offset, next)?;
                report(on_progress, &run);
            }
        }
        SectorPlan::Probes(probes) => {
            // Plan szybki składa próbki z całych jednostek, więc każda porcja to
            // dokładnie `SECTOR_UNIT_BYTES` — inna długość i tak nie przeszłaby
            // odczytu bez buforowania (musi być wielokrotnością sektora).
            'probes: for (offset, len) in probes {
                if cancel.load(Ordering::Relaxed) {
                    run.cancelled = true;
                    break;
                }
                device.seek(offset)?;
                let probe_units = len / SECTOR_UNIT_BYTES;
                for index in 0..probe_units {
                    if cancel.load(Ordering::Relaxed) {
                        run.cancelled = true;
                        break 'probes;
                    }
                    let at = offset + index * SECTOR_UNIT_BYTES;
                    let next = if index + 1 < probe_units {
                        Some(at + SECTOR_UNIT_BYTES)
                    } else {
                        None
                    };
                    run.read_unit(device, block, at, next)?;
                    report(on_progress, &run);
                }
            }
        }
    }

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(SectorScan {
        params: effective,
        disk_bytes,
        steps_done: run.steps_done,
        steps_total: run.steps_total,
        bytes_read: run.bytes_read,
        duration_ms,
        errors: run.errors,
        cancelled: run.cancelled,
        errors_truncated: run.errors_truncated,
    })
}

/// Stan przebiegu: kroki, udanie odczytane bajty i lista nieczytelnych jednostek.
struct Run {
    steps_done: u64,
    steps_total: u64,
    bytes_read: u64,
    errors: Vec<SectorError>,
    errors_truncated: bool,
    cancelled: bool,
    /// Rozmiar jednego kroku — tyle bajtów obejmuje wpis błędu.
    unit_bytes: u64,
}

impl Run {
    /// Jeden `ReadFile` pod `offset`, zaliczony jako krok — sukces i porażka
    /// liczą się tak samo, bo postęp opisuje pokrycie zakresu, nie jakość nośnika.
    ///
    /// `next` to początek następnej jednostki. Po nieudanym odczycie wskaźnik
    /// urządzenia jest w nieokreślonym miejscu, więc ustawiamy go jawnie;
    /// pozycjonowanie jest jedynym błędem, który przerywa przebieg — bez
    /// działającego wskaźnika dalsze odczyty trafiłyby w nieznane offsety.
    fn read_unit(
        &mut self,
        device: &RawDevice,
        buffer: &mut [u8],
        offset: u64,
        next: Option<u64>,
    ) -> Result<()> {
        let read = match device.read(buffer) {
            Ok(read) => Some(read),
            Err(err) => {
                self.note_error(offset, read_error_detail(&err));
                None
            }
        };
        let complete = match read {
            Some(0) => {
                // Brak błędu i brak bajtów: dla listy to ten sam problem co
                // nieudany odczyt — jednostka jest nieczytelna.
                self.note_error(offset, ZERO_READ_DETAIL.to_string());
                false
            }
            Some(read) if u64::from(read) == self.unit_bytes => {
                self.bytes_read = self.bytes_read.saturating_add(u64::from(read));
                true
            }
            Some(read) => {
                // Odczyt częściowy: jednostka nie jest w całości czytelna, więc
                // trafia na listę — inaczej mapa pokazałaby ją jako zieloną.
                let unit = self.unit_bytes;
                self.bytes_read = self.bytes_read.saturating_add(u64::from(read));
                self.note_error(offset, format!("odczytano {read} z {unit} B"));
                false
            }
            None => false,
        };
        if !complete && let Some(next) = next {
            device.seek(next)?;
        }
        self.steps_done = self.steps_done.saturating_add(1);
        Ok(())
    }

    /// Dopisuje błąd jednostki; po [`MAX_SECTOR_ERRORS`] wpisów tylko oznacza
    /// obcięcie listy — przebieg liczy dalej.
    fn note_error(&mut self, offset: u64, detail: String) {
        if !record_error(&mut self.errors, offset, self.unit_bytes, detail) {
            self.errors_truncated = true;
        }
    }
}

/// Dopisuje wpis do listy błędów, dopóki mieści się w [`MAX_SECTOR_ERRORS`].
///
/// Zwraca `false`, gdy lista jest pełna — wywołujący oznacza wtedy
/// [`SectorScan::errors_truncated`]. Limit trzyma pamięć procesu: dysk sypiący
/// błędami nie może zjeść wszystkich wolnych bajtów.
fn record_error(
    errors: &mut Vec<SectorError>,
    offset_bytes: u64,
    length_bytes: u64,
    detail: String,
) -> bool {
    if errors.len() >= MAX_SECTOR_ERRORS {
        return false;
    }
    errors.push(SectorError {
        offset_bytes,
        length_bytes,
        detail,
    });
    true
}

/// Kod błędu odczytu w postaci, w jakiej trafia na listę.
///
/// `ReadFile` zwraca kod Windows, a `RawDevice::read` pakuje go w
/// `ModuleError::Provider` jako `{:#010x}` — bierzemy sam kod, bo lista niesie
/// dane techniczne, a nie zdanie z kontekstem. Inne błędy (nie powinny tu
/// trafić) zachowują pełny opis.
fn read_error_detail(err: &ModuleError) -> String {
    match err {
        ModuleError::Provider {
            provider: "ReadFile",
            message,
        } => message.clone(),
        other => other.to_string(),
    }
}

/// Przekazuje postęp po zakończonym kroku.
fn report(on_progress: &mut dyn FnMut(SectorProgress<'_>), run: &Run) {
    on_progress(SectorProgress {
        steps_done: run.steps_done,
        steps_total: run.steps_total,
        bytes_read: run.bytes_read,
        errors: &run.errors,
    });
}

#[cfg(test)]
mod tests {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::IntoRawHandle;
    use std::path::PathBuf;

    use sk_core::sector::{SectorMode, quick_probes};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_NO_BUFFERING;

    use super::*;

    /// Plik tymczasowy podpięty pod [`RawDevice`] — testy przechodzą bez
    /// administratora i bez dysku fizycznego.
    ///
    /// `RawDevice` otwiera tylko `\\.\PhysicalDriveN`, więc uchwyt bierzemy
    /// z pliku, ale z tą samą flagą co dysk: `FILE_FLAG_NO_BUFFERING` wymusza
    /// wyrównanie bufora i długości odczytu, więc test sprawdza realne
    /// wymagania transportu, a nie wersję „uproszczoną”.
    fn temp_device(name: &str, len: usize) -> Option<(RawDevice, PathBuf)> {
        let path =
            std::env::temp_dir().join(format!("sk-sector-{name}-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..len).map(|index| (index % 251) as u8).collect();
        std::fs::write(&path, &data).ok()?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_NO_BUFFERING.0)
            .open(&path)
            .ok()?;
        Some((RawDevice(HANDLE(file.into_raw_handle())), path))
    }

    /// Skan pliku tymczasowego z parametrami przyciętymi do jego rozmiaru.
    fn scan_temp(
        device: &RawDevice,
        params: &SectorParams,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(SectorProgress<'_>),
    ) -> Result<SectorScan> {
        let disk_bytes = device.size()?;
        scan_device(device, params, disk_bytes, cancel, on_progress)
    }

    #[test]
    fn full_scan_reads_the_whole_temp_file() {
        let Some((device, path)) = temp_device("full", 8 << 20) else {
            return;
        };

        let params = SectorParams::new(0, 8 << 20, SectorMode::Full);
        let cancel = AtomicBool::new(false);
        let mut reported: Vec<(u64, u64, u64)> = Vec::new();
        let scan = scan_temp(&device, &params, &cancel, &mut |progress| {
            assert!(progress.errors.is_empty());
            assert_eq!(progress.steps_total, 2);
            reported.push((
                progress.steps_done,
                progress.steps_total,
                progress.bytes_read,
            ));
        });

        let Ok(scan) = scan else {
            panic!("skan pliku tymczasowego nie powiódł się: {scan:?}");
        };
        assert_eq!(scan.steps_total, 2, "8 MiB to dwie jednostki po 4 MiB");
        assert_eq!(scan.steps_done, scan.steps_total);
        assert_eq!(scan.bytes_read, 8 << 20);
        assert!(scan.errors.is_empty());
        assert!(!scan.errors_truncated);
        assert!(!scan.cancelled);
        assert_eq!(scan.disk_bytes, 8 << 20);
        assert_eq!(scan.params, params);
        // Postęp po każdej jednostce: 1/2 i 2/2.
        assert_eq!(reported.len(), 2);
        assert_eq!(reported[0], (1, 2, 4 << 20));
        assert_eq!(reported[1], (2, 2, 8 << 20));

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_scan_covers_a_small_temp_file() {
        let Some((device, path)) = temp_device("quick", 8 << 20) else {
            return;
        };

        let params = SectorParams::new(0, 8 << 20, SectorMode::Quick);
        // 8 MiB to mniej niż liczba próbek, więc próbki pokrywają cały plik.
        // Logikę pomijania i rozkładu pokrywa test planu w `sk-core`.
        let covered: u64 = quick_probes(params.span_bytes)
            .iter()
            .map(|(_, len)| len)
            .sum();
        assert_eq!(covered, params.span_bytes);

        let cancel = AtomicBool::new(false);
        let mut calls = 0usize;
        let scan = scan_temp(&device, &params, &cancel, &mut |progress| {
            calls += 1;
            assert!(progress.errors.is_empty());
        });

        let Ok(scan) = scan else {
            panic!("skan próbkami nie powiódł się: {scan:?}");
        };
        assert_eq!(scan.steps_total, 2);
        assert_eq!(scan.steps_done, scan.steps_total);
        assert_eq!(scan.bytes_read, params.span_bytes);
        assert!(scan.errors.is_empty());
        assert!(!scan.cancelled);
        assert_eq!(calls as u64, scan.steps_done);

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_before_start_returns_partial_result() {
        let Some((device, path)) = temp_device("cancel", 8 << 20) else {
            return;
        };

        let params = SectorParams::new(0, 8 << 20, SectorMode::Full);
        let cancel = AtomicBool::new(true);
        let mut calls = 0usize;
        let scan = scan_temp(&device, &params, &cancel, &mut |_progress| calls += 1);

        let Ok(scan) = scan else {
            panic!("przerwanie to wynik częściowy, nie błąd: {scan:?}");
        };
        assert!(scan.cancelled);
        assert_eq!(scan.steps_done, 0);
        assert_eq!(scan.steps_total, 2);
        assert_eq!(scan.bytes_read, 0);
        assert!(scan.errors.is_empty());
        assert_eq!(calls, 0, "przerwany skan nie raportuje kroków");

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn error_list_stops_at_the_limit_and_flags_truncation() {
        let mut errors = Vec::new();
        let mut truncated = false;
        // Jedna porażka więcej niż limit: ostatnia nie ma już gdzie trafić.
        for index in 0..=MAX_SECTOR_ERRORS as u64 {
            let recorded = record_error(
                &mut errors,
                index * SECTOR_UNIT_BYTES,
                SECTOR_UNIT_BYTES,
                format!("{:#010x}", 0x8007_001F_u32),
            );
            if !recorded {
                truncated = true;
            }
        }

        assert_eq!(errors.len(), MAX_SECTOR_ERRORS);
        assert!(truncated, "129. porażka musi ustawić errors_truncated");
        assert_eq!(errors[0].offset_bytes, 0);
        assert_eq!(errors[MAX_SECTOR_ERRORS - 1].offset_bytes, {
            (MAX_SECTOR_ERRORS as u64 - 1) * SECTOR_UNIT_BYTES
        });
        assert_eq!(
            errors[MAX_SECTOR_ERRORS - 1].length_bytes,
            SECTOR_UNIT_BYTES,
            "wpis błędu obejmuje całą jednostkę"
        );
        assert_eq!(errors[0].detail, "0x8007001f");
    }

    #[test]
    fn read_error_detail_keeps_the_windows_code_only() {
        // `RawDevice::read` pakuje kod `ReadFile` w `message` jako hex — na
        // listę błędów trafia sam kod, bez zdania z kontekstem.
        let read = ModuleError::provider("ReadFile", format!("{:#010x}", 0x8007_001F_u32));
        assert_eq!(read_error_detail(&read), "0x8007001f");

        // Inny dostawca (nie powinien się tu zdarzyć) zachowuje pełny opis.
        let other = ModuleError::provider("SetFilePointerEx", format!("{:#010x}", 1_u32));
        assert!(read_error_detail(&other).contains("SetFilePointerEx"));
    }

    /// Test sprzętowy: `cargo test -p sk-win --lib -- --ignored sector`.
    ///
    /// `cargo test` bez podniesienia procesu nie widzi dysków fizycznych —
    /// wtedy test przechodzi po zgłoszeniu `AccessDenied`, bo to oczekiwany
    /// wynik (brak administratora), a nie awaria transportu. Z administratorem
    /// wymaga dwóch czytelnych jednostek z prawdziwego dysku 0.
    #[test]
    #[ignore = "wymaga uprawnień administratora i prawdziwego dysku"]
    fn scans_first_units_of_a_real_drive() {
        let params = SectorParams::new(0, 8 << 20, SectorMode::Full);
        let cancel = AtomicBool::new(false);
        let mut reported = 0usize;
        let result = scan_sectors(&params, &cancel, &mut |_progress| reported += 1);

        let scan = match result {
            Ok(scan) => scan,
            Err(ModuleError::AccessDenied { what }) => {
                println!("skan pominięty — brak uprawnień administratora ({what})");
                return;
            }
            Err(err) => panic!("odczyt dysku 0 nie powiódł się: {err}"),
        };

        assert_eq!(scan.steps_total, 2);
        assert_eq!(scan.steps_done, 2);
        assert_eq!(reported, 2);
        assert!(!scan.cancelled);
        assert!(scan.disk_bytes >= params.span_bytes);
    }
}
