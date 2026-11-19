//! Skanowanie powierzchni dysku w poszukiwaniu nieczytelnych sektorów.
//!
//! Ten moduł zawiera wyłącznie plan przebiegu, typy wyniku i magazyn stanu.
//! Sam odczyt urządzenia żyje w `sk-win` (jedyne miejsce z `unsafe`), a widok —
//! w module `storage.sectors`. Podział jest ten sam co przy [`crate::surface`]:
//! skan testuje się na przygotowanym wyniku, bez dysku i bez administratora.
//!
//! Dwa tryby, jak w Error Scan HD Tune:
//! - **dokładny** czyta każdą jednostkę [`SECTOR_UNIT_BYTES`] od początku do
//!   końca zakresu — pełne pokrycie kosztem czasu (dysk 1 TB to godziny),
//! - **szybki** czyta [`QUICK_PROBES`] fragmentów rozrzuconych po całej
//!   powierzchni — kilkanaście sekund, ale bez gwarancji pokrycia.
//!
//! Skan tylko czyta: nie zapisuje na dysk ani nie zmienia danych.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::view::{MapCell, SurfaceMap};

/// Jednostka odczytu skanu: 4 MiB.
///
/// Mniejsza jednostka daje dokładniejszą pozycję błędu, ale mnoży wywołania
/// systemowe; 4 MiB to kompromis — pozycja na liście błędów ma rozdzielczość
/// 4 MiB, jak wiersz „Error at … MB” w HD Tune. Jest to wielokrotność sektora
/// 512 B i 4 KiB, więc mieści się w wymaganiach odczytu bez buforowania.
pub const SECTOR_UNIT_BYTES: u64 = 4 << 20;

/// Ile jednostek czyta jedna próbka trybu szybkiego (32 MiB).
pub const QUICK_PROBE_UNITS: u64 = 8;

/// Ile próbek rozrzuca tryb szybki (64 × 32 MiB = 2 GiB czytania).
pub const QUICK_PROBES: u64 = 64;

/// Twardy limit wpisów listy nieczytelnych fragmentów.
///
/// Dysk sypiący błędami nie może wyczerpać pamięci procesu, a lista i tak ma
/// pokazać, gdzie są problemy — nie zastąpi mapy powierzchni. Po przekroczeniu
/// limitu przebieg liczy dalej, tylko bez dopisywania wpisów
/// ([`SectorScan::errors_truncated`]).
pub const MAX_SECTOR_ERRORS: usize = 128;

/// Szerokość mapy powierzchni w komórkach.
///
/// Mapa jest kwadratową siatką jak w HD Tune: [`MAP_CELLS`] komórek niezależnie
/// od rozmiaru dysku, więc jej kształt nie zależy od pojemności nośnika.
pub const MAP_COLUMNS: u32 = 64;

/// Liczba komórek mapy powierzchni (64 × 64).
pub const MAP_CELLS: u32 = MAP_COLUMNS * MAP_COLUMNS;

/// Sposób pokrycia powierzchni.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SectorMode {
    /// Próbki rozrzucone po całej powierzchni — krótki test przesiewowy.
    Quick,
    /// Każda jednostka zakresu — pełne pokrycie.
    #[default]
    Full,
}

/// Parametry jednego przebiegu. `span_bytes` jest zawsze wielokrotnością
/// [`SECTOR_UNIT_BYTES`]; urządzenie może dodatkowo ograniczyć zakres (patrz
/// [`SectorParams::clamped_to`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorParams {
    pub drive: u32,
    pub span_bytes: u64,
    pub mode: SectorMode,
}

impl SectorParams {
    pub fn new(drive: u32, span_bytes: u64, mode: SectorMode) -> Self {
        Self {
            drive,
            span_bytes,
            mode,
        }
    }

    /// Dopasowuje zakres do rzeczywistego rozmiaru dysku.
    ///
    /// Zakres jest zaokrąglany w dół do całych jednostek — odczyt poza końcem
    /// urządzenia kończy się błędem, a nie skróceniem danych.
    pub fn clamped_to(&self, disk_bytes: u64) -> Self {
        let mut params = *self;
        params.span_bytes = self.span_bytes.min(disk_bytes) / SECTOR_UNIT_BYTES * SECTOR_UNIT_BYTES;
        params
    }
}

/// Liczba jednostek odczytu w zakresie.
pub fn unit_count(span_bytes: u64) -> u64 {
    span_bytes / SECTOR_UNIT_BYTES
}

/// Plan przebiegu: co i w jakiej kolejności czyta transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectorPlan {
    /// Tryb dokładny: `units` jednostek od początku zakresu, po kolei.
    Full { units: u64 },
    /// Tryb szybki: rozrzucone fragmenty `(offset, długość)` w bajtach.
    Probes(Vec<(u64, u64)>),
}

impl SectorPlan {
    /// Liczba kroków, czyli jednostek do odczytania — mianownik postępu.
    pub fn steps(&self) -> u64 {
        match self {
            Self::Full { units } => *units,
            Self::Probes(probes) => probes.iter().map(|(_, len)| len / SECTOR_UNIT_BYTES).sum(),
        }
    }
}

/// Plan dla parametrów: cała powierzchnia albo rozrzucone próbki.
pub fn plan(params: &SectorParams) -> SectorPlan {
    match params.mode {
        SectorMode::Full => SectorPlan::Full {
            units: unit_count(params.span_bytes),
        },
        SectorMode::Quick => SectorPlan::Probes(quick_probes(params.span_bytes)),
    }
}

/// Fragmenty trybu szybkiego: [`QUICK_PROBES`] próbek rozłożonych równomiernie
/// od początku do końca zakresu, każda do [`QUICK_PROBE_UNITS`] jednostek.
///
/// Krok jest liczony z liczby jednostek, więc próbki nie zachodzą na siebie
/// i mieszczą się w zakresie. Na dysku mniejszym niż 2 GiB próbki pokrywają
/// całą powierzchnię — wtedy tryb szybki jest po prostu pełnym odczytem
/// małego nośnika.
pub fn quick_probes(span_bytes: u64) -> Vec<(u64, u64)> {
    let units = unit_count(span_bytes);
    if units == 0 {
        return Vec::new();
    }
    let stride = units.div_ceil(QUICK_PROBES).max(1);
    let probe_units = QUICK_PROBE_UNITS.min(stride);

    let mut probes = Vec::new();
    let mut start = 0u64;
    while start < units {
        let len = probe_units.min(units - start);
        probes.push((start * SECTOR_UNIT_BYTES, len * SECTOR_UNIT_BYTES));
        start += stride;
    }
    probes
}

/// Zakres, którego nie udało się odczytać. `detail` to komunikat systemu
/// (dana techniczna, nie tekst interfejsu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectorError {
    pub offset_bytes: u64,
    pub length_bytes: u64,
    pub detail: String,
}

impl SectorError {
    /// Czy zakres błędu zahacza o `[start, end)`.
    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.offset_bytes < end && start < self.offset_bytes.saturating_add(self.length_bytes)
    }
}

/// Zakończony (lub przerwany) przebieg.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorScan {
    pub params: SectorParams,
    /// Rozmiar urządzenia odczytany z uchwytu w chwili startu.
    pub disk_bytes: u64,
    pub steps_done: u64,
    pub steps_total: u64,
    pub bytes_read: u64,
    pub duration_ms: u64,
    pub errors: Vec<SectorError>,
    /// Przerwane przez użytkownika — dane częściowe, nie wynik końcowy.
    pub cancelled: bool,
    /// Lista błędów osiągnęła [`MAX_SECTOR_ERRORS`] — dalsze wpisy pominięto.
    #[serde(default)]
    pub errors_truncated: bool,
}

impl SectorScan {
    /// Średnia prędkość odczytu w MB/s (1 MB = 1 000 000 B, jak w HD Tune).
    pub fn average_mbps(&self) -> f64 {
        if self.duration_ms == 0 {
            return 0.0;
        }
        self.bytes_read as f64 / (self.duration_ms as f64 / 1000.0) / 1_000_000.0
    }

    /// Łączny rozmiar nieczytelnych zakresów.
    pub fn damaged_bytes(&self) -> u64 {
        damaged_bytes(&self.errors)
    }

    /// Mapa powierzchni gotowego przebiegu.
    pub fn map(&self) -> SurfaceMap {
        map_for(&self.params, self.steps_done, &self.errors)
    }
}

/// Łączny rozmiar nieczytelnych zakresów z listy błędów.
pub fn damaged_bytes(errors: &[SectorError]) -> u64 {
    errors.iter().fold(0u64, |total, error| {
        total.saturating_add(error.length_bytes)
    })
}

/// Mapa powierzchni dla stanu przebiegu: komórki już odczytane są zielone
/// (albo czerwone, gdy trafił w nie błąd), pozostałe czekają.
///
/// Postęp liczy się z kroków, nie z odczytanych bajtów: w trybie dokładnym
/// kroki idą po kolei, a nieudany odczyt to nadal krok — inaczej komórka
/// z błędem zostałaby na zawsze „w kolejce”.
fn map_for(params: &SectorParams, steps_done: u64, errors: &[SectorError]) -> SurfaceMap {
    match params.mode {
        SectorMode::Full => full_map(
            params.span_bytes,
            steps_done.saturating_mul(SECTOR_UNIT_BYTES),
            errors,
        ),
        SectorMode::Quick => quick_map(&quick_probes(params.span_bytes), steps_done, errors),
    }
}

/// Mapa trybu dokładnego: stała siatka [`MAP_CELLS`] komórek na cały zakres.
///
/// Komórka obejmuje tyle jednostek, ile trzeba, żeby siatka została kwadratowa
/// (na dysku 2 TB to 128 jednostek = 512 MiB); błąd wewnątrz czyni ją czerwoną.
fn full_map(span_bytes: u64, bytes_read: u64, errors: &[SectorError]) -> SurfaceMap {
    let units = unit_count(span_bytes);
    if units == 0 {
        return SurfaceMap {
            cells: Vec::new(),
            columns: MAP_COLUMNS,
        };
    }
    let cell_units = units.div_ceil(u64::from(MAP_CELLS)).max(1);
    let cell_bytes = cell_units * SECTOR_UNIT_BYTES;
    let count = units.div_ceil(cell_units);
    let mut cells = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for index in 0..count {
        let start = index * cell_bytes;
        let end = (start + cell_bytes).min(span_bytes);
        let cell = if bytes_read < end {
            MapCell::Pending
        } else if errors.iter().any(|error| error.overlaps(start, end)) {
            MapCell::Bad
        } else {
            MapCell::Ok
        };
        cells.push(cell);
    }
    SurfaceMap {
        cells,
        columns: MAP_COLUMNS,
    }
}

/// Mapa trybu szybkiego: jedna komórka na próbkę, w kolejności offsetów.
///
/// Próbki są rozrzucone po całej powierzchni, więc odległości między nimi nie
/// da się przedstawić w skali dysku — mapa pokazuje ich stan, nie położenie.
fn quick_map(probes: &[(u64, u64)], steps_done: u64, errors: &[SectorError]) -> SurfaceMap {
    let mut cells = Vec::with_capacity(probes.len());
    let mut consumed = 0u64;
    for (offset, len) in probes {
        let end = offset.saturating_add(*len);
        consumed = consumed.saturating_add(len / SECTOR_UNIT_BYTES);
        let cell = if consumed > steps_done {
            MapCell::Pending
        } else if errors.iter().any(|error| error.overlaps(*offset, end)) {
            MapCell::Bad
        } else {
            MapCell::Ok
        };
        cells.push(cell);
    }
    let columns = 8.min(u32::try_from(cells.len()).unwrap_or(1)).max(1);
    SurfaceMap { cells, columns }
}

/// Stan skanu widziany przez moduł i UI.
///
/// `Running` niesie częściowy wynik (kroki, błędy), dzięki czemu mapa rośnie
/// w trakcie skanu — ten sam wzorzec co `SurfaceState`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SectorState {
    #[default]
    Idle,
    Running {
        params: SectorParams,
        disk_bytes: u64,
        steps_done: u64,
        steps_total: u64,
        bytes_read: u64,
        /// Czas od startu — z niego liczy się pozostały czas przebiegu.
        elapsed_ms: u64,
        errors: Vec<SectorError>,
    },
    Done(SectorScan),
    Failed {
        params: Option<SectorParams>,
        message: String,
    },
}

impl SectorState {
    /// Postęp skanu w procentach (0–100) albo `None`, gdy skan nie jest w toku.
    ///
    /// Miernikiem są odczytane jednostki: skan nie ma limitu czasu, bo ma
    /// pokryć całą powierzchnię, więc pasek dochodzi do 100 % dopiero z końcem
    /// zakresu.
    pub fn progress_percent(&self) -> Option<f64> {
        match self {
            Self::Running {
                steps_done,
                steps_total,
                ..
            } => percent_of(*steps_done as f64, *steps_total as f64),
            // Przerwany skan nie może raportować 100 % — postęp liczymy
            // z faktycznie odczytanych jednostek.
            Self::Done(scan) if scan.cancelled => {
                percent_of(scan.steps_done as f64, scan.steps_total as f64)
            }
            Self::Done(_) => Some(100.0),
            _ => None,
        }
    }

    pub fn params(&self) -> Option<&SectorParams> {
        match self {
            Self::Running { params, .. } => Some(params),
            Self::Done(scan) => Some(&scan.params),
            Self::Failed { params, .. } => params.as_ref(),
            Self::Idle => None,
        }
    }

    /// Mapa powierzchni w aktualnym stanie — `None`, gdy nie ma czego rysować.
    pub fn map(&self) -> Option<SurfaceMap> {
        match self {
            Self::Running {
                params,
                steps_done,
                errors,
                ..
            } => Some(map_for(params, *steps_done, errors)),
            Self::Done(scan) => Some(scan.map()),
            _ => None,
        }
    }

    /// Lista nieczytelnych zakresów zebranych do tej chwili.
    pub fn errors(&self) -> &[SectorError] {
        match self {
            Self::Running { errors, .. } => errors,
            Self::Done(scan) => &scan.errors,
            _ => &[],
        }
    }
}

/// Procent wartości względem całości; `None`, gdy całość jest nieznana.
fn percent_of(value: f64, total: f64) -> Option<f64> {
    if total <= 0.0 {
        return None;
    }
    Some((value / total * 100.0).clamp(0.0, 100.0))
}

/// Postęp przekazywany z transportu w trakcie przebiegu.
///
/// `errors` to migawka listy błędów — UI kopiuje ją do stanu, żeby mapa
/// czerwieniła się od razu, a nie dopiero po zakończeniu skanu.
#[derive(Debug, Clone, Copy)]
pub struct SectorProgress<'a> {
    pub steps_done: u64,
    pub steps_total: u64,
    pub bytes_read: u64,
    pub errors: &'a [SectorError],
}

/// Źródło ostatniego skanu. Implementacja realna ([`SectorStore`]) jest
/// zapisywana przez wątek skanu, a odczytywana przez moduł.
pub trait SectorSource: Send + Sync {
    fn state(&self) -> SectorState;
}

/// Magazyn stanu współdzielony między wątkiem skanu a modułem.
#[derive(Debug, Default)]
pub struct SectorStore {
    state: Mutex<SectorState>,
}

impl SectorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, state: SectorState) {
        match self.state.lock() {
            Ok(mut slot) => *slot = state,
            // Trucizna blokady nie może zgubić wyniku skanu — przejmujemy dane.
            Err(poisoned) => *poisoned.into_inner() = state,
        }
    }

    pub fn get(&self) -> SectorState {
        match self.state.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl SectorSource for SectorStore {
    fn state(&self) -> SectorState {
        self.get()
    }
}

/// Źródło bez skanu — dla trybu CLI i raportu, gdzie skan się nie uruchamia.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdleSectors;

impl SectorSource for IdleSectors {
    fn state(&self) -> SectorState {
        SectorState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(mode: SectorMode, span: u64) -> SectorParams {
        SectorParams::new(0, span, mode)
    }

    #[test]
    fn clamping_rounds_the_span_down_to_whole_units() {
        let params = SectorParams::new(1, 100 * SECTOR_UNIT_BYTES, SectorMode::Full);
        let clamped = params.clamped_to(30 * SECTOR_UNIT_BYTES + 123);
        assert_eq!(clamped.span_bytes, 30 * SECTOR_UNIT_BYTES);
        assert_eq!(clamped.mode, SectorMode::Full);
        assert_eq!(clamped.drive, 1);

        // Zakres mniejszy niż jednostka nie daje ani jednego kroku — wywołujący
        // musi to zgłosić, a nie czytać „prawie cały” dysk.
        assert_eq!(params.clamped_to(512).span_bytes, 0);
    }

    #[test]
    fn full_plan_covers_every_unit() {
        let plan = plan(&params(SectorMode::Full, 16 * SECTOR_UNIT_BYTES));
        assert_eq!(plan, SectorPlan::Full { units: 16 });
        assert_eq!(plan.steps(), 16);
    }

    #[test]
    fn quick_plan_spreads_probes_without_overlap() {
        // 2 TB: krok 4096 jednostek (16 GiB), próbka 8 jednostek (32 MiB).
        let span = 2 << 40;
        let probes = quick_probes(span);
        assert_eq!(probes.len(), QUICK_PROBES as usize);
        assert_eq!(probes[0], (0, QUICK_PROBE_UNITS * SECTOR_UNIT_BYTES));
        let last = probes.last().expect("próbki");
        assert!(last.0 + last.1 <= span, "próbka poza zakresem: {last:?}");
        for pair in probes.windows(2) {
            assert!(
                pair[0].0 + pair[0].1 <= pair[1].0,
                "próbki zachodzą na siebie: {:?}",
                pair
            );
        }
        // Postęp liczy się w jednostkach, więc każda próbka to 8 kroków.
        assert_eq!(
            plan(&params(SectorMode::Quick, span)).steps(),
            QUICK_PROBES * QUICK_PROBE_UNITS
        );
    }

    #[test]
    fn quick_plan_on_a_small_disk_reads_everything() {
        // 64 jednostki (256 MiB) to mniej niż liczba próbek: próbki pokrywają
        // cały zakres, więc tryb szybki jest pełnym odczytem małego nośnika.
        let span = 64 * SECTOR_UNIT_BYTES;
        let probes = quick_probes(span);
        assert_eq!(probes.len(), 64);
        assert!(probes.iter().all(|(_, len)| *len == SECTOR_UNIT_BYTES));
        assert_eq!(
            plan(&params(SectorMode::Quick, span)).steps(),
            unit_count(span)
        );
        assert!(quick_probes(0).is_empty());
    }

    #[test]
    fn full_map_is_a_square_grid_with_pending_and_bad_cells() {
        // 2 TB = 524288 jednostek; siatka 64×64 obejmuje po 128 jednostek,
        // czyli 512 MiB na komórkę.
        let span = 2 << 40;
        let errors = vec![SectorError {
            offset_bytes: 300 << 30,
            length_bytes: SECTOR_UNIT_BYTES,
            detail: "CRC".to_string(),
        }];
        let cell_bytes = 128 * SECTOR_UNIT_BYTES;
        let map = SectorScan {
            params: params(SectorMode::Full, span),
            disk_bytes: span,
            // Odczytane 601 komórek: błąd w komórce 600 jest już za nami.
            steps_done: 601 * 128,
            steps_total: unit_count(span),
            bytes_read: 601 * cell_bytes,
            duration_ms: 10,
            errors: errors.clone(),
            cancelled: false,
            errors_truncated: false,
        }
        .map();
        assert_eq!(map.columns, MAP_COLUMNS);
        assert_eq!(map.cells.len(), MAP_CELLS as usize);

        assert_eq!(map.cells[0], MapCell::Ok);
        // Błąd leży w komórce 600 (300 GiB / 512 MiB) i czyni ją czerwoną.
        assert_eq!(map.cells[600], MapCell::Bad);
        // Komórka 601 nie została jeszcze odczytana.
        assert_eq!(map.cells[601], MapCell::Pending);
        assert_eq!(map.cells[4095], MapCell::Pending);
    }

    #[test]
    fn full_map_marks_bad_cells_after_the_read_passes_them() {
        let span = 4 * SECTOR_UNIT_BYTES;
        let errors = vec![SectorError {
            offset_bytes: SECTOR_UNIT_BYTES,
            length_bytes: SECTOR_UNIT_BYTES,
            detail: "błąd".to_string(),
        }];
        // Cały zakres odczytany: po jednej jednostce na komórkę.
        let map = full_map(span, span, &errors);
        assert_eq!(
            map.cells,
            vec![MapCell::Ok, MapCell::Bad, MapCell::Ok, MapCell::Ok]
        );

        // Bez błędów wszystkie komórki są zielone.
        assert!(
            full_map(span, span, &[])
                .cells
                .iter()
                .all(|cell| *cell == MapCell::Ok)
        );
        // Pusty zakres nie rysuje komórek.
        assert!(full_map(0, 0, &[]).cells.is_empty());
    }

    #[test]
    fn quick_map_has_one_cell_per_probe() {
        let span = 640 * SECTOR_UNIT_BYTES;
        let probes = quick_probes(span);
        let errors = vec![SectorError {
            offset_bytes: probes[1].0,
            length_bytes: SECTOR_UNIT_BYTES,
            detail: "CRC".to_string(),
        }];
        let steps_total = plan(&params(SectorMode::Quick, span)).steps();
        let map = quick_map(&probes, steps_total, &errors);
        assert_eq!(map.cells.len(), probes.len());
        assert_eq!(map.columns, 8);
        assert_eq!(map.cells[0], MapCell::Ok);
        assert_eq!(map.cells[1], MapCell::Bad);

        // Przerwany skan: tylko pierwsza próbka odczytana, reszta czeka.
        let partial = quick_map(&probes, QUICK_PROBE_UNITS, &[]);
        assert_eq!(partial.cells[0], MapCell::Ok);
        assert!(
            partial.cells[1..]
                .iter()
                .all(|cell| *cell == MapCell::Pending)
        );
    }

    #[test]
    fn progress_follows_read_steps_and_partial_scans_are_honest() {
        let state = SectorState::Running {
            params: params(SectorMode::Full, 100 * SECTOR_UNIT_BYTES),
            disk_bytes: 100 * SECTOR_UNIT_BYTES,
            steps_done: 25,
            steps_total: 100,
            bytes_read: 25 * SECTOR_UNIT_BYTES,
            elapsed_ms: 1_000,
            errors: Vec::new(),
        };
        assert_eq!(state.progress_percent(), Some(25.0));

        let cancelled = SectorScan {
            params: params(SectorMode::Full, 100 * SECTOR_UNIT_BYTES),
            disk_bytes: 100 * SECTOR_UNIT_BYTES,
            steps_done: 40,
            steps_total: 100,
            bytes_read: 40 * SECTOR_UNIT_BYTES,
            duration_ms: 10,
            errors: Vec::new(),
            cancelled: true,
            errors_truncated: false,
        };
        assert_eq!(SectorState::Done(cancelled).progress_percent(), Some(40.0));
        assert_eq!(SectorState::Idle.progress_percent(), None);
    }

    #[test]
    fn scan_reports_speed_and_damaged_bytes() {
        let scan = SectorScan {
            params: params(SectorMode::Full, 10 * SECTOR_UNIT_BYTES),
            disk_bytes: 10 * SECTOR_UNIT_BYTES,
            steps_done: 10,
            steps_total: 10,
            bytes_read: 20_000_000,
            duration_ms: 2_000,
            errors: vec![
                SectorError {
                    offset_bytes: 0,
                    length_bytes: 1 << 20,
                    detail: "a".to_string(),
                },
                SectorError {
                    offset_bytes: 2 << 20,
                    length_bytes: 1 << 20,
                    detail: "b".to_string(),
                },
            ],
            cancelled: false,
            errors_truncated: true,
        };
        assert!((scan.average_mbps() - 10.0).abs() < 1e-9);
        assert_eq!(scan.damaged_bytes(), 2 << 20);
        assert!(!scan.errors.is_empty());

        // Zerowy czas nie może dać nieskończonej prędkości.
        let instant = SectorScan {
            duration_ms: 0,
            ..scan
        };
        assert_eq!(instant.average_mbps(), 0.0);
    }

    #[test]
    fn store_roundtrips_state_and_json() {
        let store = SectorStore::new();
        assert_eq!(store.get(), SectorState::Idle);

        let scan = SectorScan {
            params: params(SectorMode::Quick, 8 * SECTOR_UNIT_BYTES),
            disk_bytes: 8 * SECTOR_UNIT_BYTES,
            steps_done: 16,
            steps_total: 16,
            bytes_read: 64 << 20,
            duration_ms: 42,
            errors: vec![SectorError {
                offset_bytes: SECTOR_UNIT_BYTES,
                length_bytes: SECTOR_UNIT_BYTES,
                detail: "CRC".to_string(),
            }],
            cancelled: false,
            errors_truncated: false,
        };
        store.set(SectorState::Done(scan.clone()));
        assert_eq!(store.state(), SectorState::Done(scan.clone()));
        assert_eq!(IdleSectors.state(), SectorState::Idle);

        let json = serde_json::to_string(&scan).expect("serializacja");
        let back: SectorScan = serde_json::from_str(&json).expect("deserializacja");
        assert_eq!(scan, back);

        // Raport zapisany bez nowego pola nadal się czyta.
        let old: SectorScan = serde_json::from_str(
            r#"{"params":{"drive":0,"span_bytes":4194304,"mode":"full"},
                "disk_bytes":4194304,"steps_done":1,"steps_total":1,
                "bytes_read":4194304,"duration_ms":1,"errors":[],"cancelled":false}"#,
        )
        .expect("starszy format");
        assert!(!old.errors_truncated);
    }
}
