//! Surowe odczyty z dysku fizycznego: mapa prędkości powierzchni (faza
//! sekwencyjna) i czas dostępu do bloku 4K (faza losowa).
//!
//! To wyłącznie transport — typy wyniku żyją w [`sk_core::surface`], a widok
//! i analiza w module `storage.bench`. Uchwyt otwieramy z
//! `FILE_FLAG_NO_BUFFERING`, żeby mierzyć nośnik, a nie pamięć podręczną
//! systemu; ta flaga wymaga wyrównania bufora, offsetu i długości odczytu do
//! rozmiaru sektora (4 KiB pokrywa dyski 512e i 4Kn).
//!
//! Błędy pojedynczych odczytów są **danymi**, nie powodem przerwania: mapa
//! powierzchni ma pokazać, gdzie dysk nie odpowiada, a nie zniknąć w połowie.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sk_core::error::{ModuleError, Result};
use sk_core::surface::{
    MAX_SAMPLES, RANDOM_BLOCK_BYTES, RANDOM_PHASE_RESERVE_MS, SurfaceAccess, SurfaceParams,
    SurfaceReadError, SurfaceSample, SurfaceScan,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileSizeEx, OPEN_EXISTING, ReadFile, SetFilePointerEx,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO};

/// Wyrównanie bufora, offsetu i długości odczytu dla `FILE_FLAG_NO_BUFFERING`.
///
/// Bierzemy rozmiar bloku fazy losowej z `sk_core` (4 KiB): jest wielokrotnością
/// zarówno sektora 512 B, jak i 4Kn, a jednocześnie to dokładnie ten odczyt,
/// którym mierzymy czas dostępu.
const ALIGNMENT: usize = RANDOM_BLOCK_BYTES as usize;

/// Twardy limit wpisów błędów. Mapa powierzchni potrzebuje miejsca na próbki,
/// a dysk sypiący błędami nie może wyczerpać pamięci procesu.
const MAX_READ_ERRORS: usize = 128;

/// Po tylu blokach pod rząd bez sukcesu dalszy odczyt nie ma sensu — kończymy
/// fazę sekwencyjną z tym, co już zmierzone (bez błędu, to nadal wynik).
const MAX_CONSECUTIVE_FAILURES: u32 = 8;

/// Dolna granica czasu odczytu w zegarze (1 ns). `Instant` ma skończoną
/// rozdzielczość; bez tego zabezpieczenia dzielenie mogłoby dać nieskończoność.
const TIMER_FLOOR_SECS: f64 = 1e-9;

/// Ziarno xorshift64 fazy losowej. Stałe, żeby przy tych samych parametrach
/// przebieg trafiał w te same offsety (powtarzalność pomiaru). Musi być
/// niezerowe — xorshift z zerem zostaje w zerze.
const RANDOM_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// `ERROR_ACCESS_DENIED` w postaci `HRESULT_FROM_WIN32` — tak kody błędów
/// zwraca `windows-rs` (patrz `storage.rs`).
const HRESULT_ACCESS_DENIED: u32 = 0x8007_0005;

/// Sekwencyjna mapa prędkości odczytu i opcjonalna faza losowa 4K.
///
/// `on_sample(sample, bytes_read_total)` jest wołane po każdym poprawnie
/// odczytanym bloku, więc wykres rośnie w trakcie pomiaru. Parametry w wyniku
/// są już dopasowane do rzeczywistego rozmiaru urządzenia
/// ([`SurfaceParams::clamped_to`]), żeby UI pokazywał realny zakres.
///
/// Przebieg jest ograniczony czasem ([`SurfaceParams::time_limit_ms`]).
/// Z limitem dysk jest **próbkowany przez całą powierzchnię**: krok wynika
/// z pozostałego czasu i zmierzonej prędkości kroku, więc mapa zawsze sięga
/// końca dysku, a liczba próbek zależy od budżetu, nie od rozmiaru nośnika.
/// Bez limitu to zwykły odczyt sekwencyjny całego zakresu.
///
/// Na dysku 2 TB to różnica między minutą a godzinami.
///
/// Przerwanie (`cancel`) zwraca dane częściowe z `cancelled: true`, nie błąd.
pub fn scan_surface(
    params: &SurfaceParams,
    cancel: &AtomicBool,
    on_sample: &mut dyn FnMut(&SurfaceSample, u64),
) -> Result<SurfaceScan> {
    let device = RawDevice::open(params.drive)?;
    let disk_bytes = device.size()?;
    let effective = params.clamped_to(disk_bytes);
    if effective.span_bytes == 0 {
        return Err(ModuleError::unavailable("zakres mniejszy niż jeden blok"));
    }

    let block_len = effective.block_bytes as usize;
    // Zapas na wyrównanie: początek bufora wybieramy z adresu alokacji.
    let mut buffer = vec![0u8; block_len + ALIGNMENT];
    let start = alignment_offset(buffer.as_ptr() as usize);
    let started = Instant::now();
    let deadlines = Deadlines::new(effective.time_limit_ms, started);

    let sweep = sweep(
        &device,
        &effective,
        &mut buffer[start..start + block_len],
        &deadlines,
        cancel,
        on_sample,
    );
    let mut cancelled = sweep.cancelled;

    let mut access = None;
    if !cancelled && effective.random_reads > 0 && effective.span_bytes >= ALIGNMENT as u64 {
        let mut latencies: Vec<f64> = Vec::new();
        let mut state = RANDOM_SEED;
        for _ in 0..effective.random_reads {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            // Faza losowa ma własną rezerwę czasu (patrz [`Deadlines`]) —
            // po jej wyczerpaniu statystyki liczą się z tego, co zdążono.
            if deadlines.access_expired(Instant::now()) {
                break;
            }
            let offset = next_random_offset(&mut state, effective.span_bytes);
            // Błędy fazy losowej ignorujemy: mapę powierzchni tworzy faza
            // sekwencyjna, a pojedynczy nieudany odczyt 4K to nie „czerwony”
            // blok. Liczą się tylko udane odczyty.
            match timed_read(&device, offset, &mut buffer[start..start + ALIGNMENT]) {
                Ok((read, secs)) if read > 0 => latencies.push(secs * 1000.0),
                _ => {}
            }
        }
        access = access_stats(&latencies);
    }

    // Czas całego przebiegu, z fazą losową — tak samo mierzy HD Tune.
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(SurfaceScan {
        params: effective,
        disk_bytes,
        bytes_read: sweep.bytes_read,
        duration_ms,
        samples: sweep.samples,
        errors: sweep.errors,
        access,
        cancelled,
        time_limit_reached: sweep.time_limit_reached,
    })
}

/// Podział limitu czasu między fazy przebiegu.
///
/// Faza losowa dostaje [`RANDOM_PHASE_RESERVE_MS`] (ale nie więcej niż czwartą
/// część limitu), a mapa prędkości — resztę. Bez tego szybki dysk zjadłby cały
/// budżet na fazę sekwencyjną i pomiar czasu dostępu nie odbyłby się wcale.
struct Deadlines {
    sweep: Option<Instant>,
    access: Option<Instant>,
}

impl Deadlines {
    fn new(limit_ms: u64, started: Instant) -> Self {
        if limit_ms == 0 {
            return Self {
                sweep: None,
                access: None,
            };
        }
        let total = Duration::from_millis(limit_ms);
        let reserve = Duration::from_millis(RANDOM_PHASE_RESERVE_MS.min(limit_ms / 4));
        Self {
            // Absurdalnie wielki limit nie może przepełnić zegara — wtedy
            // po prostu nie ma terminu.
            sweep: started.checked_add(total - reserve),
            access: started.checked_add(total),
        }
    }

    fn sweep_expired(&self, now: Instant) -> bool {
        expired(self.sweep, now)
    }

    fn access_expired(&self, now: Instant) -> bool {
        expired(self.access, now)
    }
}

fn expired(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

/// Wynik fazy sekwencyjnej: mapa próbek albo powód, dla którego się skończyła.
struct SweepOutcome {
    samples: Vec<SurfaceSample>,
    errors: Vec<SurfaceReadError>,
    bytes_read: u64,
    cancelled: bool,
    time_limit_reached: bool,
}

/// Margines planu: celujemy w 90 % czasu do terminu. Dzięki temu zmiana
/// prędkości w trakcie (HDD zwalnia na wewnętrznych ścieżkach) nie urwie
/// końca mapy — ostatnia próbka ma zdążyć na koniec powierzchni.
const PLAN_SAFETY: f64 = 0.9;

/// Następny offset próbki: dzieli pozostałą odległość na `remaining_blocks`
/// kroków, żeby ostatnia próbka wypadła przy końcu dysku razem z terminem.
///
/// Krok jest wyrównany do sektora i nigdy mniejszy od bloku; `remaining_blocks
/// == 1` oznacza skok do ostatniego bloku, więc pokrycie końca powierzchni jest
/// gwarantowane przez samą arytmetykę planu.
fn next_sample_offset(
    current: u64,
    span_bytes: u64,
    block_bytes: u64,
    remaining_blocks: u64,
) -> u64 {
    let last = span_bytes.saturating_sub(block_bytes);
    if current >= last {
        return last;
    }
    let steps = remaining_blocks.max(1);
    let align = ALIGNMENT as u64;
    // Krok zaokrąglamy w górę do sektora: lepiej przeskoczyć o kilka KiB niż
    // zostawić na końcu niepróbkowany ogon.
    let step = (last - current).div_ceil(steps).div_ceil(align) * align;
    let step = step.max(block_bytes);
    (current + step).min(last)
}

/// Czyta próbki rozrzucone po całej powierzchni, dopóki nie skończy się
/// zakres, limit czasu albo cierpliwość ([`MAX_CONSECUTIVE_FAILURES`]).
///
/// Bez limitu to zwykły odczyt sekwencyjny całego zakresu. Z limitem krok jest
/// planowany z pozostałego czasu i zmierzonej prędkości kroku: dysk jest
/// próbkowany równomiernie od początku do końca, a liczba próbek wynika z
/// budżetu, nie z rozmiaru dysku.
fn sweep(
    device: &RawDevice,
    params: &SurfaceParams,
    buffer: &mut [u8],
    deadlines: &Deadlines,
    cancel: &AtomicBool,
    on_sample: &mut dyn FnMut(&SurfaceSample, u64),
) -> SweepOutcome {
    let mut outcome = SweepOutcome {
        samples: Vec::new(),
        errors: Vec::new(),
        bytes_read: 0,
        cancelled: false,
        time_limit_reached: false,
    };
    let last_offset = params.span_bytes.saturating_sub(params.block_bytes);
    let mut failures_in_row = 0u32;
    // Średnia kroku (pozycjonowanie + odczyt) z wykładniczym wygładzaniem —
    // na jej podstawie planujemy, ile próbek zmieści się w limicie.
    let mut step_seconds = 0.0_f64;
    let mut offset = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let step_started = Instant::now();
        match timed_read(device, offset, buffer) {
            Ok((read, secs)) if read > 0 => {
                failures_in_row = 0;
                outcome.bytes_read += u64::from(read);
                // MB dziesiętne (1 MB = 1 000 000 B), jak w HD Tune; mierzymy
                // sam odczyt, bez pozycjonowania.
                let sample = SurfaceSample {
                    offset_bytes: offset,
                    speed_mbps: f64::from(read) / secs.max(TIMER_FLOOR_SECS) / 1_000_000.0,
                };
                outcome.samples.push(sample);
                // `SurfaceSample` jest `Copy`, więc próbka trafia i do wyniku,
                // i do podglądu na żywo.
                on_sample(&sample, outcome.bytes_read);
            }
            Ok(_) => {
                // `ReadFile` bez błędu i bez bajtów to koniec urządzenia —
                // dla mapy powierzchni to samo co nieudany odczyt.
                push_read_error(
                    &mut outcome.errors,
                    offset,
                    format!(
                        "odczytano 0 z {} B — koniec zakresu urządzenia",
                        buffer.len()
                    ),
                );
                failures_in_row += 1;
            }
            Err(err) => {
                push_read_error(&mut outcome.errors, offset, err.to_string());
                failures_in_row += 1;
            }
        }
        let elapsed = step_started.elapsed().as_secs_f64();
        step_seconds = if step_seconds > 0.0 {
            step_seconds * 0.7 + elapsed * 0.3
        } else {
            elapsed
        };

        if failures_in_row >= MAX_CONSECUTIVE_FAILURES || offset >= last_offset {
            break;
        }

        offset = match deadlines.sweep {
            None => (offset + params.block_bytes).min(last_offset),
            Some(deadline) => {
                let now = Instant::now();
                if deadlines.sweep_expired(now) {
                    // Termin minął: jeden skok na koniec powierzchni, żeby mapa
                    // zawsze obejmowała cały dysk, nawet gdy dysk zwolnił.
                    outcome.time_limit_reached = true;
                    last_offset
                } else {
                    let budget = (deadline - now).as_secs_f64() * PLAN_SAFETY;
                    let positions = (last_offset - offset) / params.block_bytes;
                    let planned = (budget / step_seconds.max(TIMER_FLOOR_SECS)).floor();
                    let cap = MAX_SAMPLES.saturating_sub(outcome.samples.len()).max(1) as u64;
                    let blocks = (planned as u64).clamp(1, positions.max(1).min(cap));
                    next_sample_offset(offset, params.span_bytes, params.block_bytes, blocks)
                }
            }
        };
    }
    outcome
}

/// Uchwyt do `\\.\PhysicalDriveN` z `FILE_FLAG_NO_BUFFERING`, zamykany w `Drop`.
///
/// Własny, minimalny typ zamiast `storage::DeviceHandle`: tamten otwiera dysk
/// do zapisu (SMART) i zna IOCTL-e, a ten potrzebuje tylko odczytu
/// sekwencyjnego i losowego.
struct RawDevice(HANDLE);

impl RawDevice {
    fn open(index: u32) -> Result<Self> {
        let path = format!("\\\\.\\PhysicalDrive{index}");
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: `wide` jest zakończone zerem i żyje do końca wywołania;
        // `CreateFileW` nie wymaga atrybutów bezpieczeństwa ani szablonu.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING,
                None,
            )
        }
        .map_err(|err| match err.code().0 as u32 {
            HRESULT_ACCESS_DENIED => ModuleError::access_denied(path),
            code => ModuleError::provider("CreateFileW", format!("{path}: {code:#010x}")),
        })?;

        Ok(Self(handle))
    }

    /// Rozmiar urządzenia w bajtach.
    ///
    /// Najpierw `GetFileSizeEx` (działa na większości sterowników dysków), a gdy
    /// zwróci zero albo błąd — `IOCTL_DISK_GET_LENGTH_INFO`, dokumentowane
    /// zapytanie o rozmiar dysku fizycznego. Bez rozmiaru nie da się przyciąć
    /// zakresu, więc brak obu odpowiedzi jest błędem, nie zgadywaniem.
    fn size(&self) -> Result<u64> {
        let mut size = 0i64;
        // SAFETY: `size` to żywa zmienna na stosie, a uchwyt jest otwarty
        // (zamknięcie dopiero w `Drop`).
        if unsafe { GetFileSizeEx(self.0, &mut size) }.is_ok()
            && let Ok(bytes) = u64::try_from(size)
            && bytes > 0
        {
            return Ok(bytes);
        }

        let mut length = GET_LENGTH_INFORMATION::default();
        let mut returned = 0u32;
        // SAFETY: zapytanie nie ma bufora wejściowego; `length` jest żywą
        // strukturą o rozmiarze podanym w argumencie.
        let result = unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_DISK_GET_LENGTH_INFO,
                None,
                0,
                Some((&raw mut length).cast()),
                size_of::<GET_LENGTH_INFORMATION>() as u32,
                Some(&mut returned),
                None,
            )
        };
        result.map_err(|err| {
            ModuleError::provider(
                "IOCTL_DISK_GET_LENGTH_INFO",
                format!("{:#010x}", err.code().0 as u32),
            )
        })?;

        u64::try_from(length.Length).map_err(|_| {
            ModuleError::provider(
                "IOCTL_DISK_GET_LENGTH_INFO",
                format!("niepoprawny rozmiar urządzenia: {}", length.Length),
            )
        })
    }

    fn seek(&self, offset: u64) -> Result<()> {
        let distance = i64::try_from(offset).map_err(|_| {
            ModuleError::provider("SetFilePointerEx", format!("offset {offset} poza zakresem"))
        })?;

        // SAFETY: uchwyt jest otwarty; `FILE_BEGIN` ustawia pozycję względem
        // początku urządzenia, a wskaźnik na nową pozycję nie jest potrzebny.
        unsafe { SetFilePointerEx(self.0, distance, None, FILE_BEGIN) }.map_err(|err| {
            ModuleError::provider("SetFilePointerEx", format!("{:#010x}", err.code().0 as u32))
        })
    }

    /// Jeden `ReadFile`; zwraca liczbę faktycznie odczytanych bajtów.
    fn read(&self, buffer: &mut [u8]) -> Result<u32> {
        let mut read = 0u32;
        // SAFETY: `buffer` jest żywy, ma długość co najmniej sektora i zaczyna
        // się pod adresem wyrównanym do `ALIGNMENT` (patrz `alignment_offset`)
        // — tego wymaga uchwyt otwarty z `FILE_FLAG_NO_BUFFERING`.
        unsafe { ReadFile(self.0, Some(buffer), Some(&mut read), None) }.map_err(|err| {
            ModuleError::provider("ReadFile", format!("{:#010x}", err.code().0 as u32))
        })?;
        Ok(read)
    }
}

impl Drop for RawDevice {
    fn drop(&mut self) {
        // SAFETY: uchwyt pochodzi z `CreateFileW` i nie jest używany po zamknięciu.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Odczyt jednego bloku z pomiarem: pozycjonowanie poza zegarem (jego koszt nie
/// jest prędkością nośnika), w zegarze tylko `ReadFile`.
fn timed_read(device: &RawDevice, offset: u64, buffer: &mut [u8]) -> Result<(u32, f64)> {
    device.seek(offset)?;
    let started = Instant::now();
    let read = device.read(buffer)?;
    Ok((read, started.elapsed().as_secs_f64()))
}

/// Przesunięcie od początku alokacji do najbliższego adresu wyrównanego do
/// [`ALIGNMENT`].
///
/// `FILE_FLAG_NO_BUFFERING` odrzuca odczyt, którego bufor, offset lub długość
/// nie są wielokrotnością rozmiaru sektora. `Vec<u8>` nie ma gwarantowanego
/// wyrównania, więc rezerwujemy [`ALIGNMENT`] bajtów zapasu i liczymy początek
/// z adresu — zwykła arytmetyka na `usize`, bez `unsafe`.
fn alignment_offset(address: usize) -> usize {
    match address % ALIGNMENT {
        0 => 0,
        misalign => ALIGNMENT - misalign,
    }
}

/// Dopisuje błąd odczytu do limitu [`MAX_READ_ERRORS`]; licznik kolejnych
/// porażek rośnie w wywołującym, niezależnie od tego limitu.
fn push_read_error(errors: &mut Vec<SurfaceReadError>, offset_bytes: u64, detail: String) {
    if errors.len() < MAX_READ_ERRORS {
        errors.push(SurfaceReadError {
            offset_bytes,
            detail,
        });
    }
}

/// Krok xorshift64 (Marsaglia). Bez zależności i bez czasu — przebieg jest
/// powtarzalny przy tym samym ziarnie.
fn xorshift64(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

/// Kolejny offset fazy losowej: wyrównany do [`ALIGNMENT`] i mniejszy od
/// `span_bytes`. Zakres krótszy niż jeden blok daje offset 0 (wywołujący
/// i tak nie wchodzi w fazę losową).
fn next_random_offset(state: &mut u64, span_bytes: u64) -> u64 {
    let blocks = (span_bytes / ALIGNMENT as u64).max(1);
    *state = xorshift64(*state);
    (*state % blocks) * ALIGNMENT as u64
}

/// Statystyki fazy losowej z listy czasów odczytu (ms).
///
/// `None` przy braku udanych odczytów — osobne pole w wyniku, bo „brak pomiaru”
/// to nie to samo co „zerowy czas dostępu”.
fn access_stats(latencies: &[f64]) -> Option<SurfaceAccess> {
    if latencies.is_empty() {
        return None;
    }
    let avg_ms = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let min_ms = latencies.iter().copied().fold(f64::INFINITY, f64::min);
    let max_ms = latencies.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(SurfaceAccess {
        reads: u32::try_from(latencies.len()).unwrap_or(u32::MAX),
        avg_ms,
        min_ms,
        max_ms,
        // IOPS przy kolejce QD1; zerowy czas średni nie może dać nieskończoności.
        iops: if avg_ms > 0.0 { 1000.0 / avg_ms } else { 0.0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprawdza całą ścieżkę odczytu (CreateFileW z `FILE_FLAG_NO_BUFFERING`,
    /// wyrównany bufor, `SetFilePointerEx`, `ReadFile`) na pliku tymczasowym —
    /// bez administratora i bez dysku. Nie zastępuje testu sprzętowego, ale
    /// łapie błąd we flagach i wyrównaniu, który inaczej wyszedłby dopiero
    /// u użytkownika.
    #[test]
    fn unbuffered_read_pattern_works_on_a_temp_file() {
        use std::os::windows::ffi::OsStrExt;

        let path = std::env::temp_dir().join(format!("sk-surface-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..(2 << 20)).map(|index| (index % 251) as u8).collect();
        if std::fs::write(&path, &data).is_err() {
            return;
        }

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` jest zakończone zerem i żyje do końca wywołania.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING,
                None,
            )
        };
        let Ok(handle) = handle else {
            let _ = std::fs::remove_file(&path);
            return;
        };
        let device = RawDevice(handle);

        let block = 1usize << 20;
        let mut buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);
        for offset in [0u64, block as u64] {
            let result = timed_read(&device, offset, &mut buffer[start..start + block]);
            assert!(
                result.is_ok(),
                "odczyt pliku testowego pod offsetem {offset}"
            );
            let Ok((read, _)) = result else {
                return;
            };
            assert_eq!(read as usize, block, "offset {offset}");
            let expected = &data[offset as usize..offset as usize + block];
            assert_eq!(&buffer[start..start + block], expected, "offset {offset}");
        }

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    /// Otwiera plik tymczasowy jako [`RawDevice`] — pełna ścieżka uchwytu bez
    /// administratora i bez dysku. `None`, gdy system nie pozwolił zapisać
    /// pliku albo go otworzyć (wtedy test nie ma czego sprawdzać).
    fn temp_device(name: &str, len: usize) -> Option<(RawDevice, std::path::PathBuf)> {
        use std::os::windows::ffi::OsStrExt;

        let path =
            std::env::temp_dir().join(format!("sk-surface-{name}-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..len).map(|index| (index % 251) as u8).collect();
        std::fs::write(&path, &data).ok()?;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` jest zakończone zerem i żyje do końca wywołania.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING,
                None,
            )
        };
        let Ok(handle) = handle else {
            let _ = std::fs::remove_file(&path);
            return None;
        };
        Some((RawDevice(handle), path))
    }

    #[test]
    fn deadlines_reserve_the_tail_of_the_budget_for_the_random_phase() {
        let started = Instant::now();
        let deadlines = Deadlines::new(60_000, started);
        assert_eq!(
            deadlines.sweep,
            Some(started + Duration::from_millis(57_000)),
            "mapa dostaje limit pomniejszony o rezerwę 3 s"
        );
        assert_eq!(
            deadlines.access,
            Some(started + Duration::from_millis(60_000))
        );

        // Krótki limit: rezerwa to czwarta część, a nie 3 s — inaczej mapa
        // nie dostałaby prawie żadnego czasu.
        let short = Deadlines::new(8_000, started);
        assert_eq!(short.sweep, Some(started + Duration::from_millis(6_000)));
        assert_eq!(short.access, Some(started + Duration::from_millis(8_000)));

        let unlimited = Deadlines::new(0, started);
        assert_eq!(unlimited.sweep, None);
        assert_eq!(unlimited.access, None);
        assert!(!unlimited.sweep_expired(started + Duration::from_secs(3_600)));

        assert!(deadlines.sweep_expired(started + Duration::from_millis(57_000)));
        assert!(!deadlines.sweep_expired(started + Duration::from_millis(56_999)));
        assert!(!deadlines.access_expired(started + Duration::from_millis(59_999)));
        assert!(deadlines.access_expired(started + Duration::from_millis(60_000)));
    }

    #[test]
    fn sweep_with_an_expired_budget_still_covers_the_end() {
        let Some((device, path)) = temp_device("limit", 8 << 20) else {
            return;
        };
        let params = SurfaceParams::new(0, 8 << 20);
        let block = params.block_bytes as usize;
        let mut buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);

        // Termin już minął — pierwszy blok ma wejść do mapy, a plan i tak musi
        // dobić do końca powierzchni, żeby użytkownik nie dostał mapy 0–1 MB.
        let now = Instant::now();
        let past = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        let deadlines = Deadlines::new(1, past);
        let cancel = AtomicBool::new(false);
        let mut ignore = |_: &SurfaceSample, _: u64| {};
        let outcome = sweep(
            &device,
            &params,
            &mut buffer[start..start + block],
            &deadlines,
            &cancel,
            &mut ignore,
        );

        assert!(outcome.time_limit_reached);
        assert!(!outcome.cancelled);
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.samples.len(), 2, "początek i koniec powierzchni");
        assert_eq!(outcome.samples[0].offset_bytes, 0);
        assert_eq!(
            outcome.samples[1].offset_bytes,
            params.span_bytes - params.block_bytes
        );
        assert_eq!(outcome.bytes_read, 2 * params.block_bytes);

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn short_budget_spreads_samples_over_the_whole_temp_file() {
        let Some((device, path)) = temp_device("spread", 32 << 20) else {
            return;
        };
        let params = SurfaceParams::new(0, 32 << 20).with_time_limit_ms(3);
        let block = params.block_bytes as usize;
        let mut buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);

        let deadlines = Deadlines::new(3, Instant::now());
        let cancel = AtomicBool::new(false);
        let mut ignore = |_: &SurfaceSample, _: u64| {};
        let outcome = sweep(
            &device,
            &params,
            &mut buffer[start..start + block],
            &deadlines,
            &cancel,
            &mut ignore,
        );

        assert!(!outcome.cancelled);
        assert!(outcome.samples.len() >= 2, "{:?}", outcome.samples);
        // Mapa musi sięgać końca pliku, a offsety rosnąć i być wyrównane.
        let last = outcome
            .samples
            .last()
            .expect("ostatnia próbka")
            .offset_bytes;
        assert_eq!(last + params.block_bytes, params.span_bytes);
        for pair in outcome.samples.windows(2) {
            assert!(pair[1].offset_bytes > pair[0].offset_bytes);
        }
        for sample in &outcome.samples {
            assert_eq!(sample.offset_bytes % ALIGNMENT as u64, 0);
        }

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sample_offsets_divide_the_remaining_distance_evenly() {
        let span = 1 << 40;
        let block = 4 << 20;
        let last = span - block;

        // Plan maleje o jedną próbkę na krok (tak liczy `sweep`: pozostały czas
        // dzielony przez zmierzony czas kroku) — dziesięć kroków trafia w koniec.
        let mut offset = 0u64;
        for remaining in (1..=10).rev() {
            offset = next_sample_offset(offset, span, block, remaining);
        }
        assert_eq!(offset, last);

        // Krok jest wyrównany do sektora i nie mniejszy od bloku.
        let next = next_sample_offset(0, span, block, 10);
        assert_eq!(next % ALIGNMENT as u64, 0);
        assert!(next >= block);
        assert!(next < last);

        // Jeden krok do końca = skok na ostatni blok.
        assert_eq!(next_sample_offset(0, span, block, 1), last);

        // Więcej kroków niż pozycji nie daje kroku mniejszego od bloku.
        assert_eq!(next_sample_offset(0, span, block, span / block), block);

        // Koniec zakresu zostaje na końcu, a nie za nim.
        assert_eq!(next_sample_offset(last, span, block, 5), last);
        assert_eq!(next_sample_offset(last - block, span, block, 5), last);
    }

    #[test]
    fn sweep_without_a_limit_reads_the_whole_span() {
        let Some((device, path)) = temp_device("full", 8 << 20) else {
            return;
        };
        let params = SurfaceParams::new(0, 8 << 20).with_time_limit_ms(0);
        let block = params.block_bytes as usize;
        let mut buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);

        let deadlines = Deadlines::new(0, Instant::now());
        let cancel = AtomicBool::new(false);
        let mut reported = Vec::new();
        let outcome = sweep(
            &device,
            &params,
            &mut buffer[start..start + block],
            &deadlines,
            &cancel,
            &mut |sample, bytes_read| reported.push((sample.offset_bytes, bytes_read)),
        );

        assert!(!outcome.time_limit_reached);
        assert!(!outcome.cancelled);
        assert_eq!(outcome.samples.len(), 8);
        assert_eq!(outcome.bytes_read, 8 << 20);
        assert_eq!(
            outcome
                .samples
                .last()
                .map_or(0, |sample| sample.offset_bytes)
                + params.block_bytes,
            8 << 20,
            "bez limitu ostatnia próbka musi obejmować koniec zakresu"
        );
        assert_eq!(reported.len(), 8);
        assert_eq!(reported.last().map(|(_, total)| *total), Some(8 << 20));
        for sample in &outcome.samples {
            assert!(
                sample.speed_mbps.is_finite() && sample.speed_mbps > 0.0,
                "offset {}: prędkość {}",
                sample.offset_bytes,
                sample.speed_mbps
            );
        }

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sweep_honours_a_cancel_request_before_the_first_block() {
        let Some((device, path)) = temp_device("cancel", 2 << 20) else {
            return;
        };
        let params = SurfaceParams::new(0, 2 << 20);
        let block = params.block_bytes as usize;
        let mut buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);

        let deadlines = Deadlines::new(0, Instant::now());
        let cancel = AtomicBool::new(true);
        let mut ignore = |_: &SurfaceSample, _: u64| {};
        let outcome = sweep(
            &device,
            &params,
            &mut buffer[start..start + block],
            &deadlines,
            &cancel,
            &mut ignore,
        );

        assert!(outcome.cancelled);
        assert!(!outcome.time_limit_reached);
        assert!(outcome.samples.is_empty());
        assert_eq!(outcome.bytes_read, 0);

        drop(device);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn alignment_offset_lands_on_sector_boundary() {
        assert_eq!(alignment_offset(0), 0);
        assert_eq!(alignment_offset(ALIGNMENT), 0);
        assert_eq!(alignment_offset(1), ALIGNMENT - 1);
        assert_eq!(alignment_offset(ALIGNMENT + 7), ALIGNMENT - 7);
        // 4K musi pokrywać sektor 4Kn, a rozmiar odczytu losowego to ten sam blok.
        assert_eq!(ALIGNMENT, 4096);
        assert_eq!(ALIGNMENT, RANDOM_BLOCK_BYTES as usize);
    }

    #[test]
    fn aligned_slice_fits_in_buffer_with_spare() {
        let block = 1 << 20;
        let buffer = vec![0u8; block + ALIGNMENT];
        let start = alignment_offset(buffer.as_ptr() as usize);
        assert_eq!((buffer.as_ptr() as usize + start) % ALIGNMENT, 0);
        assert!(
            start + block <= buffer.len(),
            "start {start} + blok {block} przekracza bufor {}",
            buffer.len()
        );
    }

    #[test]
    fn random_offsets_stay_inside_span_and_aligned() {
        let span = 8 << 20;
        let mut state = RANDOM_SEED;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let offset = next_random_offset(&mut state, span);
            assert!(offset < span, "offset {offset} poza zakresem {span}");
            assert_eq!(
                offset % ALIGNMENT as u64,
                0,
                "offset {offset} nie jest wyrównany do sektora"
            );
            seen.insert(offset);
        }
        // 8 MiB / 4 KiB = 2048 możliwych pozycji; tysiąc losowań musi trafiać
        // w różne miejsca, a nie w jeden offset.
        assert!(seen.len() > 1, "generator utknął na jednym offsecie");

        // Zakres mniejszy niż blok nie może dać dzielenia przez zero.
        let mut state = RANDOM_SEED;
        assert_eq!(next_random_offset(&mut state, 0), 0);
        let mut state = RANDOM_SEED;
        assert_eq!(next_random_offset(&mut state, ALIGNMENT as u64), 0);
    }

    #[test]
    fn access_stats_averages_known_latencies() {
        let Some(stats) = access_stats(&[0.5, 1.0, 1.5]) else {
            panic!("trzy pomiary muszą dać statystyki");
        };
        assert_eq!(stats.reads, 3);
        assert!((stats.avg_ms - 1.0).abs() < 1e-9, "avg {}", stats.avg_ms);
        assert!((stats.min_ms - 0.5).abs() < 1e-9, "min {}", stats.min_ms);
        assert!((stats.max_ms - 1.5).abs() < 1e-9, "max {}", stats.max_ms);
        assert!((stats.iops - 1000.0).abs() < 1e-6, "iops {}", stats.iops);
    }

    #[test]
    fn access_stats_handles_single_read() {
        let Some(stats) = access_stats(&[2.0]) else {
            panic!("jeden pomiar musi dać statystyki");
        };
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.avg_ms, 2.0);
        assert_eq!(stats.min_ms, 2.0);
        assert_eq!(stats.max_ms, 2.0);
        assert_eq!(stats.iops, 500.0);
    }

    #[test]
    fn access_stats_returns_none_without_successful_reads() {
        assert_eq!(access_stats(&[]), None);
    }

    #[test]
    fn access_stats_guards_zero_average_iops() {
        let Some(stats) = access_stats(&[0.0, 0.0]) else {
            panic!("dwa zerowe pomiary muszą dać statystyki");
        };
        assert_eq!(stats.reads, 2);
        assert_eq!(stats.avg_ms, 0.0);
        assert_eq!(
            stats.iops, 0.0,
            "zerowy czas średni nie może dać nieskończonego IOPS"
        );
        assert!(stats.iops.is_finite());
    }

    /// Test sprzętowy: `cargo test -p sk-win --lib -- --ignored surface`.
    ///
    /// `cargo test` bez podniesienia procesu nie widzi dysków fizycznych —
    /// wtedy test przechodzi po zgłoszeniu `AccessDenied`, bo to oczekiwany
    /// wynik (brak administratora), a nie awaria transportu. Z administratorem
    /// wymaga próbek z prawdziwego dysku 0.
    #[test]
    #[ignore = "wymaga uprawnień administratora i prawdziwego dysku"]
    fn maps_first_blocks_of_a_real_drive() {
        let params = SurfaceParams {
            random_reads: 20,
            ..SurfaceParams::new(0, 8 << 20)
        };
        let cancel = AtomicBool::new(false);
        let mut reported = 0usize;
        let result = scan_surface(&params, &cancel, &mut |_sample, total| {
            assert!(total > 0);
            reported += 1;
        });

        let scan = match result {
            Ok(scan) => scan,
            Err(ModuleError::AccessDenied { what }) => {
                println!("pomiar pominięty — brak uprawnień administratora ({what})");
                return;
            }
            Err(err) => panic!("odczyt dysku 0 nie powiódł się: {err}"),
        };

        assert!(!scan.samples.is_empty(), "brak próbek z prawdziwego dysku");
        assert_eq!(reported, scan.samples.len());
        assert!(scan.params.span_bytes > 0);
        assert!(scan.params.span_bytes <= params.span_bytes);
        assert_eq!(scan.bytes_read, scan.params.span_bytes);
        assert!(!scan.cancelled);
        assert!(
            !scan.time_limit_reached,
            "8 MB w minutę musi się zmieścić bez sięgania do limitu"
        );
        for sample in &scan.samples {
            assert!(
                sample.speed_mbps.is_finite() && sample.speed_mbps > 0.0,
                "offset {}: prędkość {}",
                sample.offset_bytes,
                sample.speed_mbps
            );
        }
        if let Some(access) = scan.access {
            assert!(access.reads <= 20);
            assert!(access.avg_ms > 0.0);
            assert!(access.iops.is_finite());
        }
    }
}
