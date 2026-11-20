//! Okno aplikacji: lista modułów, generyczny renderer widoków, wątek roboczy.
//!
//! Zbieranie danych nigdy nie dzieje się na wątku UI — moduły odpyłują WMI
//! setki milisekund, a telemetria próbkuje co sekundę. Wątek roboczy trzyma
//! też historię próbek (`LiveHistory` i `SensorStore`), więc `system.live`
//! i `hw.sensors` są bezstanowe.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use sk_core::i18n::{Locale, Translator};
use sk_core::live::LiveHistory;
use sk_core::module::{Capability, CollectCtx, ModuleMeta, ModuleOutput};
use sk_core::providers::WmiProvider;
use sk_core::registry::ModuleRegistry;
use sk_core::sector::{SectorMode, SectorParams, SectorState, SectorStore};
use sk_core::sensors::SensorStore;
use sk_core::surface::{DEFAULT_TIME_LIMIT_MS, SurfaceParams, SurfaceState, SurfaceStore};
use sk_win::{SystemClock, WinHostEnv, WinMachine, WinSensors, WinStorage, WinWmi};

use crate::chart_image;
use crate::live::sample;
use crate::render;
use crate::report::{Report, ReportOptions};

const LIVE_MODULE_ID: &str = "system.live";
const SENSORS_MODULE_ID: &str = "hw.sensors";
const LIVE_INTERVAL: Duration = Duration::from_secs(1);
/// 600 próbek = 10 minut historii przy próbkowaniu co sekundę.
const LIVE_HISTORY_CAPACITY: usize = 600;
const BENCH_MODULE_ID: &str = "storage.bench";
/// Odświeżanie widoku w trakcie testu — 4 razy na sekundę wystarcza, żeby
/// wykres rósł płynnie, a wątek roboczy nie był zajęty ciągłym zbieraniem.
const BENCH_REFRESH_INTERVAL: Duration = Duration::from_millis(400);
const SECTOR_MODULE_ID: &str = "storage.sectors";
/// Mapa skanu ma rosnąć w tym samym tempie co wykres benchmarku: oba widoki
/// czytają magazyn wyniku, a nie mierzą nic na nowo.
const SECTOR_REFRESH_INTERVAL: Duration = BENCH_REFRESH_INTERVAL;
/// Ile czekamy na klatkę z wykresem, zanim uznamy, że zrzut się nie udał.
/// Integracja okna oddaje obraz w następnej klatce; przy ukrytym oknie nie
/// oddaje go wcale, więc potrzebny jest termin.
const CHART_SHOT_TIMEOUT: Duration = Duration::from_secs(3);

enum Job {
    CollectAll,
    SampleLive,
    CollectBench,
    CollectSectors,
    ListDrives,
    ExportHtml { path: PathBuf, locale: Locale },
}

enum Event {
    Module {
        id: &'static str,
        result: Result<ModuleOutput, String>,
    },
    /// Aktualizacja modułów czasu rzeczywistego: `system.live` i `hw.sensors`.
    /// Jedna wiadomość, bo obie próbki powstają w jednej rundzie WMI.
    Sample {
        live: Result<ModuleOutput, String>,
        sensors: Result<ModuleOutput, String>,
    },
    /// Aktualizacja `storage.bench` w trakcie lub po teście.
    Bench(Result<ModuleOutput, String>),
    /// Aktualizacja `storage.sectors` w trakcie lub po skanie.
    Sectors(Result<ModuleOutput, String>),
    /// Lista dysków do panelu testu (WMI czytane w wątku roboczym).
    Drives(Vec<DriveChoice>),
    Done,
    ExportDone(Result<String, String>),
}

/// Dysk w panelu testu prędkości. To osobny typ od `PhysicalDrive`, bo panel
/// potrzebuje nazwy i rozmiaru z WMI jeszcze przed otwarciem urządzenia.
#[derive(Debug, Clone)]
struct DriveChoice {
    index: u32,
    model: String,
    size_bytes: u64,
}

impl DriveChoice {
    fn label(&self) -> String {
        let model = if self.model.is_empty() {
            "PhysicalDrive".to_string()
        } else {
            self.model.clone()
        };
        let size = if self.size_bytes == 0 {
            "-".to_string()
        } else {
            format_gib(self.size_bytes)
        };
        format!("#{} · {} · {}", self.index, model, size)
    }
}

/// Zakres testu. Test idzie zawsze przez całą powierzchnię — o czasie decyduje
/// limit, a nie rozmiar dysku — więc nie ma czego wybierać w panelu. Dla dysku
/// o nieznanym rozmiarze bierzemy 1 GiB: `clamped_to` i tak przytnie zakres do
/// rzeczywistego urządzenia.
fn scan_span(disk_bytes: u64) -> u64 {
    disk_bytes.max(1 << 30)
}

/// Parametry skanu dla wybranego dysku i trybu.
///
/// `clamped_to` przycina zakres do urządzenia i zaokrągla go w dół do całych
/// jednostek, więc `None` oznacza, że po zaokrągleniu nie zostaje ani jeden
/// krok (nośnik mniejszy niż 4 MiB) — wtedy skanu nie ma czego uruchamiać.
fn sector_params(disk_bytes: u64, drive: u32, mode: SectorMode) -> Option<SectorParams> {
    let params = if disk_bytes > 0 {
        SectorParams::new(drive, scan_span(disk_bytes), mode).clamped_to(disk_bytes)
    } else {
        // Nieznany rozmiar: zakres bierzemy domyślny, a `clamped_to` przytnie
        // go do urządzenia dopiero przy otwarciu uchwytu.
        SectorParams::new(drive, scan_span(0), mode)
    };
    (params.span_bytes > 0).then_some(params)
}

/// Stan skanu w nagłówku karty: jedno słowo albo procent — ten sam podział ról
/// co w karcie benchmarku (`bench_state_label`), z tymi samymi kolorami.
fn sector_state_label(translator: &Translator, state: &SectorState) -> (String, egui::Color32) {
    match state {
        SectorState::Idle => (
            translator.t("ui.sector.waiting"),
            egui::Color32::from_rgb(150, 156, 162),
        ),
        SectorState::Running { .. } => (
            format!("{:.0} %", state.progress_percent().unwrap_or(0.0)),
            egui::Color32::from_rgb(120, 170, 220),
        ),
        SectorState::Done(_) => (
            translator.t("ui.sector.finished"),
            egui::Color32::from_rgb(110, 190, 120),
        ),
        SectorState::Failed { .. } => (
            translator.t("ui.sector.failed"),
            egui::Color32::from_rgb(220, 100, 100),
        ),
    }
}

/// Szybkość testu — limit czasu przebiegu wybierany w panelu.
///
/// To nie jest „jakość” pomiaru (blok dobiera się do zakresu), tylko budżet
/// czasu: po jego upływie próbkowanie kończy się na końcu powierzchni, więc
/// ten sam test na dysku 120 GB i 2 TB trwa tyle samo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchSpeed {
    Fast,
    Normal,
    Precise,
    Unlimited,
}

impl BenchSpeed {
    const ALL: [Self; 4] = [Self::Fast, Self::Normal, Self::Precise, Self::Unlimited];

    fn label_key(self) -> &'static str {
        match self {
            Self::Fast => "ui.bench.speed_fast",
            Self::Normal => "ui.bench.speed_normal",
            Self::Precise => "ui.bench.speed_precise",
            Self::Unlimited => "ui.bench.speed_unlimited",
        }
    }

    /// Limit w ms; 0 = bez limitu (test trwa tyle, ile zajmie cały zakres).
    fn limit_ms(self) -> u64 {
        match self {
            Self::Fast => 15_000,
            Self::Normal => 30_000,
            Self::Precise => DEFAULT_TIME_LIMIT_MS,
            Self::Unlimited => 0,
        }
    }
}

fn format_gib(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let gib = bytes as f64 / GIB;
    if gib >= 1024.0 {
        format!("{:.2} TB", gib / 1024.0)
    } else {
        format!("{gib:.1} GB")
    }
}

/// Nazwa pliku ze zrzutem wykresu: model dysku i znacznik czasu, żeby w
/// katalogu z kilkoma testami dało się je rozróżnić bez otwierania.
fn chart_file_name(model: &str) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("wykres-{}-{stamp}.png", file_slug(model))
}

/// Model dysku jako fragment nazwy pliku: tylko litery i cyfry, reszta to
/// pojedynczy myślnik (nazwy z WMI bywają z ukośnikami i spacjami).
fn file_slug(model: &str) -> String {
    let mut slug = String::new();
    for character in model.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "dysk".to_string()
    } else {
        slug.to_string()
    }
}

pub fn run_gui() -> Result<(), String> {
    let mut translator = Translator::load().map_err(|err| format!("tłumaczenia: {err}"))?;
    translator.set_locale(Locale::Pl);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("SerwisKit"),
        ..Default::default()
    };

    eframe::run_native(
        "SerwisKit",
        options,
        Box::new(move |_cc| Ok(Box::new(SerwisApp::new(translator)))),
    )
    .map_err(|err| format!("nie udało się uruchomić GUI: {err}"))
}

struct SerwisApp {
    translator: Translator,
    metas: Vec<ModuleMeta>,
    selected: Option<&'static str>,
    results: BTreeMap<&'static str, Result<ModuleOutput, String>>,
    jobs: Option<Sender<Job>>,
    events: Receiver<Event>,
    /// Ten sam magazyn widzi moduł (przez `CollectCtx`) i wątek benchmarku.
    surface: Arc<SurfaceStore>,
    /// Ten sam magazyn widzi moduł `storage.sectors` i wątek skanu.
    sector_store: Arc<SectorStore>,
    pending: bool,
    live_pending: bool,
    next_live_at: Instant,
    bench_drives: Vec<DriveChoice>,
    bench_drive: Option<u32>,
    bench_speed: BenchSpeed,
    bench_running: bool,
    bench_cancel: Option<Arc<AtomicBool>>,
    bench_pending: bool,
    /// Wejście w moduł ma odświeżyć widok raz, bez czekania na pełną rundę.
    bench_refresh_once: bool,
    next_bench_at: Instant,
    /// Tryb skanu wybierany w karcie: szybki (próbki) albo dokładny (całość).
    sector_mode: SectorMode,
    sector_running: bool,
    sector_cancel: Option<Arc<AtomicBool>>,
    sector_pending: bool,
    /// Wejście w moduł ma odświeżyć widok raz, bez czekania na pełną rundę.
    sector_refresh_once: bool,
    next_sector_at: Instant,
    /// Podgląd wykresu otwarty przyciskiem „Zapisz PNG”.
    chart_modal: bool,
    /// Zrzut w drodze: integracja okna oddaje klatkę dopiero w następnym kroku.
    chart_shot: Option<ChartShot>,
    chart_status: Option<Result<String, String>>,
    status: Option<Result<String, String>>,
    startup_error: Option<String>,
}

/// Zrzut wykresu w drodze. Kadr liczy się w callbacku integracji okna, więc
/// wynik wraca kanałem — UI tylko go odbiera i pokazuje.
struct ChartShot {
    result: Receiver<Result<String, String>>,
    started: Instant,
}

impl SerwisApp {
    fn new(translator: Translator) -> Self {
        let registry = sk_modules::registry();
        let metas: Vec<ModuleMeta> = registry
            .modules()
            .iter()
            .map(|module| module.meta())
            .collect();
        let selected = metas.first().map(|meta| meta.id);

        let (job_tx, job_rx) = channel::<Job>();
        let (event_tx, event_rx) = channel::<Event>();
        let surface = Arc::new(SurfaceStore::new());
        let sector_store = Arc::new(SectorStore::new());

        let worker = {
            let surface = Arc::clone(&surface);
            let sector_store = Arc::clone(&sector_store);
            thread::Builder::new()
                .name("serwiskit-worker".to_string())
                .spawn(move || worker_loop(registry, job_rx, event_tx, surface, sector_store))
        };

        let (jobs, startup_error) = match worker {
            Ok(_handle) => (Some(job_tx), None),
            Err(err) => (
                None,
                Some(format!("nie udało się uruchomić wątku roboczego: {err}")),
            ),
        };

        let mut app = Self {
            translator,
            metas,
            selected,
            results: BTreeMap::new(),
            jobs,
            events: event_rx,
            surface,
            sector_store,
            sector_mode: SectorMode::Full,
            sector_running: false,
            sector_cancel: None,
            sector_pending: false,
            sector_refresh_once: false,
            next_sector_at: Instant::now(),
            pending: false,
            live_pending: false,
            next_live_at: Instant::now(),
            bench_drives: Vec::new(),
            bench_drive: None,
            bench_speed: BenchSpeed::Precise,
            bench_running: false,
            bench_cancel: None,
            bench_pending: false,
            bench_refresh_once: false,
            next_bench_at: Instant::now(),
            chart_modal: false,
            chart_shot: None,
            chart_status: None,
            status: None,
            startup_error,
        };
        app.request_refresh();
        app
    }

    fn request_refresh(&mut self) {
        // Kliknięcia w trakcie rundy nie kolejkują kolejnych pełnych zbierzeń:
        // kanał jest nieograniczony, więc bez tego 5 kliknięć = 5 rund WMI.
        if self.pending {
            return;
        }
        self.pending = true;
        self.status = None;
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::CollectAll);
            // Lista dysków zmienia się rzadziej niż inwentarz, ale odświeżamy
            // ją razem z rundą — panel testu nie może pokazywać nieistniejącego
            // już dysku.
            let _ = jobs.send(Job::ListDrives);
        }
    }

    /// Jedna próbka telemetrii na sekundę, tylko gdy moduł live albo czujniki
    /// są otwarte. Aktualizuje oba widoki z jednej rundy WMI.
    fn request_live_sample(&mut self) {
        if self.live_pending || Instant::now() < self.next_live_at {
            return;
        }
        if let Some(jobs) = &self.jobs
            && jobs.send(Job::SampleLive).is_ok()
        {
            self.live_pending = true;
            // Termin liczymy od poprzedniego, żeby kadencja nie dryfowała;
            // jeśli UI zaspało, wracamy do „teraz + 1 s” bez serii próbek.
            let mut next = self.next_live_at + LIVE_INTERVAL;
            if next <= Instant::now() {
                next = Instant::now() + LIVE_INTERVAL;
            }
            self.next_live_at = next;
        }
    }

    /// Zleca przeliczenie widoku `storage.bench`. Moduł czyta magazyn wyniku,
    /// więc to nie jest ponowny pomiar — tylko odświeżenie widoku i wykresu.
    fn request_bench_refresh(&mut self) {
        if self.bench_pending || Instant::now() < self.next_bench_at {
            return;
        }
        if let Some(jobs) = &self.jobs
            && jobs.send(Job::CollectBench).is_ok()
        {
            self.bench_pending = true;
            self.next_bench_at = Instant::now() + BENCH_REFRESH_INTERVAL;
        }
    }

    /// Zleca przeliczenie widoku `storage.sectors`. Moduł czyta magazyn stanu,
    /// więc to nie jest ponowny skan — tylko odświeżenie mapy i listy błędów.
    fn request_sector_refresh(&mut self) {
        if self.sector_pending || Instant::now() < self.next_sector_at {
            return;
        }
        if let Some(jobs) = &self.jobs
            && jobs.send(Job::CollectSectors).is_ok()
        {
            self.sector_pending = true;
            self.next_sector_at = Instant::now() + SECTOR_REFRESH_INTERVAL;
        }
    }

    fn start_bench(&mut self) {
        if self.bench_running || self.sector_running {
            return;
        }
        let Some(index) = self.bench_drive else {
            return;
        };
        self.chart_modal = false;
        self.chart_status = None;
        let disk_bytes = self
            .bench_drives
            .iter()
            .find(|drive| drive.index == index)
            .map_or(0, |drive| drive.size_bytes);
        // Zawsze cała powierzchnia: o czasie decyduje limit, a nie rozmiar dysku.
        let span = scan_span(disk_bytes);
        let params = if disk_bytes > 0 {
            SurfaceParams::new(index, span).clamped_to(disk_bytes)
        } else {
            SurfaceParams::new(index, span)
        };
        let params = params.with_time_limit_ms(self.bench_speed.limit_ms());
        if params.span_bytes == 0 {
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.bench_cancel = Some(Arc::clone(&cancel));
        self.bench_running = true;
        self.next_bench_at = Instant::now();
        crate::bench::spawn(
            Arc::clone(&self.surface),
            params,
            disk_bytes.max(params.span_bytes),
            cancel,
        );
        self.request_bench_refresh();
    }

    fn cancel_bench(&mut self) {
        if let Some(cancel) = &self.bench_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Test kończy się w osobnym wątku, więc UI sprawdza stan magazynu.
    /// Po zakończeniu wymuszamy jednorazowe przeliczenie widoku.
    fn poll_bench(&mut self, ctx: &egui::Context) {
        if !self.bench_running {
            return;
        }
        let state = self.surface.get();
        if matches!(state, SurfaceState::Done(_) | SurfaceState::Failed { .. }) {
            self.bench_running = false;
            self.bench_cancel = None;
            self.next_bench_at = Instant::now();
            self.request_bench_refresh();
            ctx.request_repaint();
            return;
        }
        if self.selected == Some(BENCH_MODULE_ID) {
            self.request_bench_refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn start_scan(&mut self) {
        // Dwa testy naraz fałszują wynik: oba czytają ten sam nośnik.
        if self.sector_running || self.bench_running {
            return;
        }
        let Some(index) = self.bench_drive else {
            return;
        };
        let disk_bytes = self
            .bench_drives
            .iter()
            .find(|drive| drive.index == index)
            .map_or(0, |drive| drive.size_bytes);
        let Some(params) = sector_params(disk_bytes, index, self.sector_mode) else {
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.sector_cancel = Some(Arc::clone(&cancel));
        self.sector_running = true;
        self.next_sector_at = Instant::now();
        crate::sector::spawn(Arc::clone(&self.sector_store), params, cancel);
        self.request_sector_refresh();
    }

    fn cancel_scan(&mut self) {
        if let Some(cancel) = &self.sector_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Skan kończy się w osobnym wątku, więc UI sprawdza stan magazynu.
    /// Po zakończeniu wymuszamy jednorazowe przeliczenie widoku.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        if !self.sector_running {
            return;
        }
        let state = self.sector_store.get();
        if matches!(state, SectorState::Done(_) | SectorState::Failed { .. }) {
            self.sector_running = false;
            self.sector_cancel = None;
            self.next_sector_at = Instant::now();
            self.request_sector_refresh();
            ctx.request_repaint();
            return;
        }
        if self.selected == Some(SECTOR_MODULE_ID) {
            self.request_sector_refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// Karta sterowania skanem sektorów — ten sam układ co karta benchmarku:
    /// dysk, tryb i akcje, a pod nimi pasek postępu. Mapa i lista błędów są
    /// w widoku modułu poniżej, więc karta ich nie powtarza.
    fn sector_panel(&mut self, ui: &mut egui::Ui) {
        let state = self.sector_store.get();
        egui::Frame::group(ui.style())
            .fill(ui.style().visuals.faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Jeden wybór dysku dla obu testów: benchmark i skan
                    // otwierają ten sam `\\.\PhysicalDriveN`.
                    self.drive_combo(ui, "ui.sector.drive", "ui.sector.no_drives");
                });

                ui.horizontal(|ui| {
                    ui.label(self.translator.t("ui.sector.mode"));
                    // Dwa tryby jako przełącznik, nie lista rozwijana: wybór
                    // jest widoczny od razu i kosztuje jedno kliknięcie.
                    ui.selectable_value(
                        &mut self.sector_mode,
                        SectorMode::Quick,
                        self.translator.t("ui.sector.mode_quick"),
                    );
                    ui.selectable_value(
                        &mut self.sector_mode,
                        SectorMode::Full,
                        self.translator.t("ui.sector.mode_full"),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.sector_running {
                            if ui.button(self.translator.t("ui.sector.cancel")).clicked() {
                                self.cancel_scan();
                            }
                        } else {
                            let start = egui::Button::new(
                                egui::RichText::new(self.translator.t("ui.sector.start")).strong(),
                            )
                            .min_size(egui::vec2(92.0, 26.0));
                            // Start nieaktywny w trakcie benchmarku — dwa testy
                            // naraz fałszują wynik.
                            if ui
                                .add_enabled(
                                    self.bench_drive.is_some() && !self.bench_running,
                                    start,
                                )
                                .clicked()
                            {
                                self.start_scan();
                            }
                        }
                        // W trakcie skanu procent jest na pasku postępu —
                        // plakietka powtarzałaby tę samą liczbę.
                        if !matches!(state, SectorState::Running { .. }) {
                            ui.add_space(8.0);
                            let (text, color) = sector_state_label(&self.translator, &state);
                            ui.label(egui::RichText::new(text).small().color(color));
                        }
                    });
                });

                if matches!(state, SectorState::Running { .. })
                    && let Some(progress) = state.progress_percent()
                {
                    ui.add_space(2.0);
                    ui.add(
                        egui::ProgressBar::new((progress / 100.0) as f32)
                            .desired_width(ui.available_width())
                            .text(format!("{progress:.0} %")),
                    );
                }
            });
    }

    /// Panel sterowania testem. To jedyny fragment GUI wiedzący o konkretnym
    /// module — reszta interfejsu renderuje wyłącznie model widoku.
    ///
    /// Karta ma stały układ: dysk, czas testu i akcje, a pod nimi pasek
    /// postępu. Statystyki są w widoku modułu poniżej, więc panel ich nie
    /// powtarza — pokazuje tylko sterowanie i stan.
    fn bench_panel(&mut self, ui: &mut egui::Ui) {
        let state = self.surface.get();
        egui::Frame::group(ui.style())
            .fill(ui.style().visuals.faint_bg_color)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                self.bench_controls(ui, &state);
            });

        if let Some(status) = &self.chart_status {
            match status {
                Ok(path) => ui.label(
                    egui::RichText::new(format!(
                        "{}: {path}",
                        self.translator.t("ui.bench.chart_saved")
                    ))
                    .small()
                    .weak(),
                ),
                Err(message) => ui.colored_label(
                    egui::Color32::from_rgb(220, 90, 90),
                    format!("{}: {message}", self.translator.t("ui.bench.chart_failed")),
                ),
            };
        }
    }

    /// Stan testu w nagłówku karty: jedno słowo albo procent.
    fn bench_state_label(&self, state: &SurfaceState) -> (String, egui::Color32) {
        match state {
            SurfaceState::Idle => (
                self.translator.t("ui.bench.waiting"),
                egui::Color32::from_rgb(150, 156, 162),
            ),
            SurfaceState::Running { .. } => (
                format!("{:.0} %", state.progress_percent().unwrap_or(0.0)),
                egui::Color32::from_rgb(120, 170, 220),
            ),
            SurfaceState::Done(_) => (
                self.translator.t("ui.bench.finished"),
                egui::Color32::from_rgb(110, 190, 120),
            ),
            SurfaceState::Failed { .. } => (
                self.translator.t("ui.bench.failed"),
                egui::Color32::from_rgb(220, 100, 100),
            ),
        }
    }

    /// Wybór dysku wspólny dla benchmarku i skanu sektorów: oba otwierają ten
    /// sam `\\.\PhysicalDriveN`, więc lista jest jedna (`bench_drives`), a wybór
    /// trafia do `bench_drive` niezależnie od tego, która karta go pokazuje.
    /// `label_key` i `empty_key` pochodzą z rodziny i18n karty, która go rysuje.
    fn drive_combo(&mut self, ui: &mut egui::Ui, label_key: &str, empty_key: &str) {
        ui.label(self.translator.t(label_key));
        let drive_labels: Vec<(u32, String)> = self
            .bench_drives
            .iter()
            .map(|drive| (drive.index, drive.label()))
            .collect();
        let selected = self
            .bench_drive
            .and_then(|index| {
                drive_labels
                    .iter()
                    .find(|(candidate, _)| *candidate == index)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| self.translator.t(empty_key));
        let mut drive = self.bench_drive;
        egui::ComboBox::from_id_salt("bench-drive")
            .selected_text(selected)
            .width((ui.available_width() - 4.0).max(220.0))
            .show_ui(ui, |ui| {
                for (index, label) in &drive_labels {
                    ui.selectable_value(&mut drive, Some(*index), label);
                }
            });
        self.bench_drive = drive;
    }

    fn bench_controls(&mut self, ui: &mut egui::Ui, state: &SurfaceState) {
        ui.horizontal(|ui| self.drive_combo(ui, "ui.bench.drive", "ui.bench.no_drives"));

        ui.horizontal(|ui| {
            ui.label(self.translator.t("ui.bench.speed"));
            // Cztery przedziały czasu jako przełącznik, nie lista rozwijana:
            // wybór jest widoczny od razu i kosztuje jedno kliknięcie.
            for speed in BenchSpeed::ALL {
                ui.selectable_value(
                    &mut self.bench_speed,
                    speed,
                    self.translator.t(speed.label_key()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.bench_running {
                    if ui.button(self.translator.t("ui.bench.cancel")).clicked() {
                        self.cancel_bench();
                    }
                } else {
                    let start = egui::Button::new(
                        egui::RichText::new(self.translator.t("ui.bench.start")).strong(),
                    )
                    .min_size(egui::vec2(92.0, 26.0));
                    // Start nieaktywny w trakcie skanu sektorów — dwa testy
                    // naraz fałszują wynik.
                    if ui
                        .add_enabled(self.bench_drive.is_some() && !self.sector_running, start)
                        .clicked()
                    {
                        self.start_bench();
                    }
                    if matches!(state, SurfaceState::Done(_))
                        && ui
                            .button(self.translator.t("ui.bench.save_chart"))
                            .clicked()
                    {
                        self.chart_status = None;
                        self.chart_modal = true;
                    }
                }
                // W trakcie testu procent jest na pasku postępu — plakietka
                // powtarzałaby tę samą liczbę.
                if !matches!(state, SurfaceState::Running { .. }) {
                    ui.add_space(8.0);
                    let (text, color) = self.bench_state_label(state);
                    ui.label(egui::RichText::new(text).small().color(color));
                }
            });
        });

        if matches!(state, SurfaceState::Running { .. })
            && let Some(progress) = state.progress_percent()
        {
            ui.add_space(2.0);
            ui.add(
                egui::ProgressBar::new((progress / 100.0) as f32)
                    .desired_width(ui.available_width())
                    .text(format!("{progress:.0} %")),
            );
        }
    }

    /// Podgląd wykresu z dyskiem w nagłówku — to ten obszar trafia do PNG.
    ///
    /// Zrzut robimy z podglądu, a nie z widoku modułu, bo wykres może być
    /// przewinięty poza okno: modal jest zawsze w całości widoczny, więc kadr
    /// nie zależy od przewinięcia ani od rozmiaru okna.
    fn chart_modal(&mut self, ctx: &egui::Context) {
        if !self.chart_modal {
            return;
        }
        let state = self.surface.get();
        let SurfaceState::Done(scan) = &state else {
            self.chart_modal = false;
            return;
        };

        let disk_bytes = scan.disk_bytes.max(scan.params.span_bytes);
        let model = self.drive_model(scan.params.drive);
        let chart = sk_modules::bench::chart(&scan.samples, disk_bytes);
        let average = sk_modules::bench::stats(scan).map_or(0.0, |stats| stats.avg_mbps);
        let summary = format!(
            "{} · {} · {average:.0} MB/s",
            format_gib(disk_bytes),
            sk_modules::bench::format_duration(scan.duration_ms),
        );

        let translator = &self.translator;
        let failure = match &self.chart_status {
            Some(Err(message)) => Some(format!(
                "{}: {message}",
                self.translator.t("ui.bench.chart_failed")
            )),
            _ => None,
        };
        let mut capture = None;
        let mut save = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("bench-chart-preview")).show(ctx, |ui| {
            ui.set_width(900.0);
            ui.label(egui::RichText::new(translator.t("ui.bench.chart_preview")).strong());
            ui.add_space(6.0);
            let card = egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&model).strong());
                    ui.label(egui::RichText::new(&summary).small().weak());
                    ui.add_space(2.0);
                    render::chart_plot(
                        ui,
                        &chart,
                        320.0,
                        "bench-chart-preview-plot",
                        render::ChartMode::Still,
                    );
                });
            capture = Some(card.response.rect);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                save = ui
                    .button(egui::RichText::new(translator.t("ui.bench.save_chart")).strong())
                    .clicked();
                close = ui.button(translator.t("ui.bench.close")).clicked();
            });
            // Błąd zapisu pokazujemy w podglądzie, bo panel jest wtedy
            // zasłonięty modalem.
            if let Some(failure) = &failure {
                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), failure);
            }
        });

        if modal.should_close() {
            close = true;
        }
        if close {
            self.chart_modal = false;
        }
        if save && let Some(area) = capture {
            self.request_chart_shot(ctx, area, PathBuf::from(chart_file_name(&model)));
        }
    }

    /// Zrzut klatki i zapis obrazka. Callback integracji okna nie działa na
    /// wątku UI, więc plik powstaje tam, a wynik wraca kanałem.
    fn request_chart_shot(&mut self, ctx: &egui::Context, area: egui::Rect, path: PathBuf) {
        if self.chart_shot.is_some() {
            return;
        }
        let screen = ctx.content_rect();
        let area = area.intersect(screen);
        let outside = self.translator.t("ui.bench.chart_empty");
        let (sender, result) = channel();
        ctx.request_screenshot(move |image| {
            let outcome = match chart_image::crop(&image, screen, area) {
                Some(cropped) => {
                    chart_image::write_png(&path, &cropped).map(|()| path.display().to_string())
                }
                None => Err(outside),
            };
            let _ = sender.send(outcome);
        });
        self.chart_shot = Some(ChartShot {
            result,
            started: Instant::now(),
        });
        ctx.request_repaint();
    }

    fn poll_chart_shot(&mut self, ctx: &egui::Context) {
        let (received, timed_out) = match self.chart_shot.as_ref() {
            Some(shot) => (
                shot.result.try_recv(),
                shot.started.elapsed() > CHART_SHOT_TIMEOUT,
            ),
            None => return,
        };
        match received {
            Ok(result) => {
                if result.is_ok() {
                    // Zapisany obrazek jest tym, co pokazywał podgląd —
                    // zamykamy go, żeby nie zasłaniał wyniku.
                    self.chart_modal = false;
                }
                self.chart_shot = None;
                self.chart_status = Some(result);
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) if !timed_out => {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                self.chart_shot = None;
                self.chart_status = Some(Err(self.translator.t("ui.bench.chart_failed")));
                ctx.request_repaint();
            }
        }
    }

    /// Nazwa modelu dysku do panelu i do nazwy pliku z wykresem.
    fn drive_model(&self, index: u32) -> String {
        self.bench_drives
            .iter()
            .find(|drive| drive.index == index)
            .filter(|drive| !drive.model.is_empty())
            .map_or_else(
                || format!("PhysicalDrive{index}"),
                |drive| drive.model.clone(),
            )
    }

    fn export_html(&mut self) {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = PathBuf::from(format!("raport-serwiskit-{stamp}.html"));
        if let Some(jobs) = &self.jobs
            && jobs
                .send(Job::ExportHtml {
                    path,
                    locale: self.translator.locale(),
                })
                .is_ok()
        {
            self.status = None;
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut received = false;
        loop {
            match self.events.try_recv() {
                Ok(Event::Module { id, result }) => {
                    self.results.insert(id, result);
                    received = true;
                }
                Ok(Event::Sample { live, sensors }) => {
                    self.live_pending = false;
                    self.results.insert(LIVE_MODULE_ID, live);
                    self.results.insert(SENSORS_MODULE_ID, sensors);
                    received = true;
                }
                Ok(Event::Bench(result)) => {
                    self.bench_pending = false;
                    self.results.insert(BENCH_MODULE_ID, result);
                    received = true;
                }
                Ok(Event::Sectors(result)) => {
                    self.sector_pending = false;
                    self.results.insert(SECTOR_MODULE_ID, result);
                    received = true;
                }
                Ok(Event::Drives(drives)) => {
                    self.bench_drives = drives;
                    let known = self
                        .bench_drive
                        .is_some_and(|index| self.bench_drives.iter().any(|d| d.index == index));
                    if !known {
                        self.bench_drive = self.bench_drives.first().map(|drive| drive.index);
                    }
                    received = true;
                }
                Ok(Event::Done) => {
                    self.pending = false;
                    received = true;
                }
                Ok(Event::ExportDone(result)) => {
                    self.status = Some(result);
                    received = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.live_pending = false;
                    self.bench_pending = false;
                    self.sector_pending = false;
                    break;
                }
            }
        }
        received
    }

    fn selected_meta(&self) -> Option<ModuleMeta> {
        let id = self.selected?;
        self.metas.iter().find(|meta| meta.id == id).copied()
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(self.translator.t("ui.app.title"));
            ui.separator();

            let mut locale = self.translator.locale();
            ui.label(self.translator.t("ui.locale"));
            for candidate in Locale::ALL {
                if ui
                    .selectable_value(&mut locale, candidate, candidate.code().to_uppercase())
                    .clicked()
                {
                    self.translator.set_locale(candidate);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.pending {
                    ui.spinner();
                }
                if ui.button(self.translator.t("ui.refresh")).clicked() {
                    self.request_refresh();
                }
                if ui.button(self.translator.t("ui.export_html")).clicked() {
                    self.export_html();
                }
            });
        });

        if let Some(status) = &self.status {
            match status {
                Ok(path) => {
                    let text = format!("{}: {path}", self.translator.t("ui.export_saved"));
                    ui.label(egui::RichText::new(text).small().weak());
                }
                Err(message) => {
                    let text = format!("{}: {message}", self.translator.t("ui.export_failed"));
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), text);
                }
            }
        }
    }

    fn module_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(self.translator.t("ui.modules")).strong());
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.metas.is_empty() {
                    ui.label(self.translator.t("ui.no_modules"));
                    return;
                }

                let mut current_group = None;
                for meta in &self.metas {
                    if current_group != Some(meta.group) {
                        current_group = Some(meta.group);
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(self.translator.t(meta.group.label_key()))
                                .small()
                                .weak(),
                        );
                    }
                    let selected = self.selected == Some(meta.id);
                    let name = self.translator.t(meta.name_key);
                    if ui.selectable_label(selected, name).clicked() {
                        self.selected = Some(meta.id);
                        // Po wejściu w telemetrię pierwsza próbka idzie od razu.
                        self.next_live_at = Instant::now();
                        // Test prędkości mógł się zakończyć, gdy moduł był
                        // zamknięty — pokaż aktualny wynik od razu.
                        if meta.id == BENCH_MODULE_ID {
                            self.bench_refresh_once = true;
                        }
                        // Skan mógł się zakończyć, gdy moduł był zamknięty.
                        if meta.id == SECTOR_MODULE_ID {
                            self.sector_refresh_once = true;
                        }
                    }
                }
            });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.startup_error.clone() {
            ui.colored_label(
                egui::Color32::from_rgb(220, 100, 100),
                format!("{}: {error}", self.translator.t("ui.error")),
            );
            return;
        }

        let Some(meta) = self.selected_meta() else {
            ui.label(self.translator.t("ui.select_module"));
            return;
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(self.translator.t(meta.name_key));
                    ui.label(
                        egui::RichText::new(self.translator.t(meta.capability.label_key()))
                            .small()
                            .color(capability_color(meta.capability)),
                    );
                    if meta.experimental {
                        ui.label(
                            egui::RichText::new(self.translator.t("ui.experimental"))
                                .small()
                                .color(egui::Color32::from_rgb(220, 160, 60)),
                        );
                    }
                });
                ui.label(egui::RichText::new(self.translator.t(meta.desc_key)).weak());
                ui.separator();

                if meta.id == SECTOR_MODULE_ID {
                    self.sector_panel(ui);
                    ui.separator();
                }

                if meta.id == BENCH_MODULE_ID {
                    self.bench_panel(ui);
                    ui.separator();
                }

                match self.results.get(meta.id) {
                    Some(Ok(output)) => render::view(ui, &output.view, &self.translator),
                    Some(Err(message)) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 90, 90),
                            format!("{}: {message}", self.translator.t("ui.error")),
                        );
                    }
                    None => {
                        ui.add_space(8.0);
                        ui.spinner();
                    }
                }
            });
    }
}

impl eframe::App for SerwisApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.drain_events() {
            ctx.request_repaint();
        }

        if matches!(self.selected, Some(LIVE_MODULE_ID | SENSORS_MODULE_ID)) {
            self.request_live_sample();
            // Wybudź UI dokładnie na kolejną próbkę (min. 50 ms, żeby nie zapętlić).
            let wait = self
                .next_live_at
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(50));
            ctx.request_repaint_after(wait);
        }
        if self.pending {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.bench_refresh_once && self.selected == Some(BENCH_MODULE_ID) {
            self.bench_refresh_once = false;
            self.next_bench_at = Instant::now();
            self.request_bench_refresh();
        }
        if self.sector_refresh_once && self.selected == Some(SECTOR_MODULE_ID) {
            self.sector_refresh_once = false;
            self.next_sector_at = Instant::now();
            self.request_sector_refresh();
        }
        self.poll_bench(&ctx);
        self.poll_scan(&ctx);
        self.poll_chart_shot(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("modules")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| self.module_list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.content(ui));
        // Modal rysujemy po panelach, żeby był nad nimi. Zrzut klatki rusza
        // w tym samym kroku, w którym użytkownik kliknął „Zapisz PNG”.
        self.chart_modal(&ctx);
    }
}

fn worker_loop(
    registry: ModuleRegistry,
    jobs: Receiver<Job>,
    events: Sender<Event>,
    surface: Arc<SurfaceStore>,
    sectors: Arc<SectorStore>,
) {
    let wmi = WinWmi::new();
    let clock = SystemClock::new();
    let host = WinHostEnv::new();
    let machine = WinMachine::new();
    let storage = WinStorage::new();
    let mut history = LiveHistory::new(LIVE_HISTORY_CAPACITY);
    // Czujniki GPU/ACPI/baterii próbkujemy razem z telemetrią: mają tę samą
    // kadencję 1 s i zasilają wykres temperatur w `hw.sensors`.
    let sensor_probe = WinSensors::new();
    let mut sensors = SensorStore::new(LIVE_HISTORY_CAPACITY);
    sensors.set_probe(sensor_probe.probe());
    // Magazyn skanu sektorów przychodzi z zewnątrz (wspólny `SectorStore`),
    // żeby wątek skanu i moduł `storage.sectors` widziały ten sam stan.
    let started = Instant::now();
    let locale = Locale::Pl;

    while let Ok(job) = jobs.recv() {
        match job {
            Job::CollectAll => {
                wmi.begin_round();
                sensors.push(started.elapsed().as_secs_f64(), sensor_probe.readings());
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    sensors: &sensors,
                    sectors: sectors.as_ref(),
                    surface: surface.as_ref(),
                    locale,
                };
                if !collect_all(&registry, &ctx, &events) {
                    return;
                }
            }
            Job::SampleLive => {
                wmi.begin_round();
                let elapsed = started.elapsed().as_secs_f64();
                sample(&wmi, &mut history, elapsed);
                sensors.push(elapsed, sensor_probe.readings());

                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    sensors: &sensors,
                    sectors: sectors.as_ref(),
                    surface: surface.as_ref(),
                    locale,
                };
                let live = collect_module(&registry, LIVE_MODULE_ID, &ctx);
                let sensors = collect_module(&registry, SENSORS_MODULE_ID, &ctx);
                if events.send(Event::Sample { live, sensors }).is_err() {
                    return;
                }
            }
            Job::CollectBench => {
                wmi.begin_round();
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    sensors: &sensors,
                    sectors: sectors.as_ref(),
                    surface: surface.as_ref(),
                    locale,
                };
                let result = match registry.find(BENCH_MODULE_ID) {
                    Some(module) => module.collect(&ctx).map_err(|err| err.to_string()),
                    None => Err("brak modułu storage.bench".to_string()),
                };
                if events.send(Event::Bench(result)).is_err() {
                    return;
                }
            }
            Job::CollectSectors => {
                wmi.begin_round();
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    sensors: &sensors,
                    sectors: sectors.as_ref(),
                    surface: surface.as_ref(),
                    locale,
                };
                let result = collect_module(&registry, SECTOR_MODULE_ID, &ctx);
                if events.send(Event::Sectors(result)).is_err() {
                    return;
                }
            }
            Job::ListDrives => {
                wmi.begin_round();
                let drives = list_drives(&wmi);
                if events.send(Event::Drives(drives)).is_err() {
                    return;
                }
            }
            Job::ExportHtml { path, locale } => {
                wmi.begin_round();
                sensors.push(started.elapsed().as_secs_f64(), sensor_probe.readings());
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    sensors: &sensors,
                    sectors: sectors.as_ref(),
                    surface: surface.as_ref(),
                    locale,
                };
                let options = ReportOptions {
                    module_filter: None,
                    app_version: env!("CARGO_PKG_VERSION"),
                };
                let report = Report::collect(&registry, &ctx, &options)
                    .with_wmi_stats(wmi.cache_stats().into());
                let result = write_report_html(&report, locale, &path);
                if events.send(Event::ExportDone(result)).is_err() {
                    return;
                }
                if !collect_all(&registry, &ctx, &events) {
                    return;
                }
            }
        }
    }
}

/// Lista dysków dla panelu testu. Błąd WMI daje pustą listę — panel pokaże
/// „brak dysków”, a moduł `storage.bench` własną notkę o nieudanym odczycie.
fn list_drives(wmi: &WinWmi) -> Vec<DriveChoice> {
    let rows = match wmi.query(
        sk_core::providers::NS_CIMV2,
        sk_core::queries::WIN32_DISK_DRIVE,
    ) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::debug!(%error, "brak listy dysków z WMI");
            return Vec::new();
        }
    };
    let mut drives: Vec<DriveChoice> = rows
        .iter()
        .filter_map(|row| {
            let index = row.get("Index")?.as_u64()? as u32;
            let model = row
                .get("Model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let size_bytes = row
                .get("Size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            Some(DriveChoice {
                index,
                model,
                size_bytes,
            })
        })
        .collect();
    drives.sort_by_key(|drive| drive.index);
    drives
}

fn write_report_html(
    report: &Report,
    locale: Locale,
    path: &std::path::Path,
) -> Result<String, String> {
    let mut translator = Translator::load().map_err(|err| format!("tłumaczenia: {err}"))?;
    translator.set_locale(locale);
    let html = report.to_html(&translator);
    crate::report::write_html(path, &html)?;
    Ok(path.display().to_string())
}

/// Zbiera jeden moduł po `id`. Brak modułu to błąd widoku, nie panika —
/// rejestr jest jawny, więc brak wpisu oznacza błąd kompilacji wydania.
fn collect_module(
    registry: &ModuleRegistry,
    id: &str,
    ctx: &CollectCtx<'_>,
) -> Result<ModuleOutput, String> {
    registry
        .find(id)
        .ok_or_else(|| format!("brak modułu {id}"))?
        .collect(ctx)
        .map_err(|err| err.to_string())
}

fn collect_all(registry: &ModuleRegistry, ctx: &CollectCtx<'_>, events: &Sender<Event>) -> bool {
    for module in registry.modules() {
        let meta = module.meta();
        let result = module.collect(ctx).map_err(|err| err.to_string());
        if events
            .send(Event::Module {
                id: meta.id,
                result,
            })
            .is_err()
        {
            return false;
        }
    }
    events.send(Event::Done).is_ok()
}

fn capability_color(capability: Capability) -> egui::Color32 {
    match capability {
        Capability::Userland => egui::Color32::from_rgb(120, 170, 120),
        Capability::Elevated => egui::Color32::from_rgb(200, 170, 90),
        Capability::Destructive => egui::Color32::from_rgb(210, 100, 100),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disk_span_is_the_drive_size() {
        // Panel nie ma wyboru zakresu: test zawsze idzie przez cały dysk.
        assert_eq!(scan_span(512 << 30), 512 << 30);
        assert_eq!(scan_span(0), 1 << 30, "nieznany rozmiar nie daje zera");

        let disk_bytes = 512 << 30;
        let params = SurfaceParams::new(4, scan_span(disk_bytes)).clamped_to(disk_bytes);
        assert_eq!(params.span_bytes, disk_bytes);
        assert_eq!(params.time_limit_ms, DEFAULT_TIME_LIMIT_MS);
    }

    #[test]
    fn speed_choices_build_a_time_budget_with_a_minute_default() {
        assert_eq!(BenchSpeed::Fast.limit_ms(), 15_000);
        assert_eq!(BenchSpeed::Normal.limit_ms(), 30_000);
        assert_eq!(BenchSpeed::Precise.limit_ms(), 60_000);
        assert_eq!(BenchSpeed::Unlimited.limit_ms(), 0);
        // Domyślny wybór to limit z `sk-core`, czyli minuta.
        assert_eq!(BenchSpeed::Precise.limit_ms(), DEFAULT_TIME_LIMIT_MS);

        let mut keys: Vec<&str> = BenchSpeed::ALL
            .iter()
            .map(|speed| speed.label_key())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys.len(),
            BenchSpeed::ALL.len(),
            "etykiety muszą być różne"
        );
    }

    #[test]
    fn drive_label_names_index_model_and_size() {
        let drive = DriveChoice {
            index: 4,
            model: "WDC PC SN530".to_string(),
            size_bytes: 512 << 30,
        };
        assert_eq!(drive.label(), "#4 · WDC PC SN530 · 512.0 GB");

        let reader = DriveChoice {
            index: 6,
            model: String::new(),
            size_bytes: 0,
        };
        assert_eq!(reader.label(), "#6 · PhysicalDrive · -");
    }

    #[test]
    fn chart_file_name_carries_the_drive_model() {
        let name = chart_file_name("CT1000MX500SSD1");
        assert!(name.starts_with("wykres-CT1000MX500SSD1-"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        // Po modelu i myślniku jest jeszcze znacznik czasu YYYYMMDD-HHMMSS.
        assert_eq!(name.len(), "wykres-CT1000MX500SSD1-".len() + 15 + 4);
    }

    #[test]
    fn file_slug_replaces_everything_but_letters_and_digits() {
        assert_eq!(file_slug("CT1000MX500SSD1"), "CT1000MX500SSD1");
        assert_eq!(file_slug("WDC PC SN530 / 512GB"), "WDC-PC-SN530-512GB");
        // Ścieżka urządzenia nie może wnieść ukośników do nazwy pliku.
        assert_eq!(file_slug("\\\\.\\PhysicalDrive0"), "PhysicalDrive0");
        assert_eq!(file_slug("  "), "dysk");
        assert_eq!(file_slug(""), "dysk");
    }

    #[test]
    fn sector_params_round_down_to_whole_units() {
        use sk_core::sector::SECTOR_UNIT_BYTES;

        // 512 GB mieści się w całości: zakres jest wielokrotnością jednostki,
        // więc odczyt nie sięga poza koniec urządzenia.
        let params =
            sector_params(512 << 30, 1, SectorMode::Full).expect("512 GB to całe jednostki");
        assert_eq!(params.drive, 1);
        assert_eq!(params.mode, SectorMode::Full);
        assert_eq!(params.span_bytes, 512 << 30);
        assert_eq!(params.span_bytes % SECTOR_UNIT_BYTES, 0);

        // Tryb wybrany w karcie trafia do parametrów bez zmian.
        assert_eq!(
            sector_params(512 << 30, 0, SectorMode::Quick).map(|params| params.mode),
            Some(SectorMode::Quick)
        );

        // Dysk mniejszy niż jedna jednostka nie daje ani jednego kroku.
        assert_eq!(sector_params(1 << 20, 0, SectorMode::Full), None);

        // Nieznany rozmiar: zakres domyślny, przycięty dopiero przy otwarciu
        // urządzenia — inaczej brak danych z WMI blokowałby skan.
        let unknown = sector_params(0, 2, SectorMode::Quick).expect("zakres domyślny");
        assert_eq!(unknown.span_bytes, 1 << 30);
        assert!(unknown.span_bytes > 0);
    }

    #[test]
    fn sector_state_label_reads_states() {
        use sk_core::sector::SectorScan;

        let translator = Translator::load().expect("tłumaczenia się wczytują");
        let span = 4 << 20;
        let params = SectorParams::new(0, span, SectorMode::Full);
        let scan = SectorScan {
            params,
            disk_bytes: span,
            steps_done: 4,
            steps_total: 4,
            bytes_read: 4 * span,
            duration_ms: 2_000,
            errors: Vec::new(),
            cancelled: false,
            errors_truncated: false,
        };
        let states = [
            SectorState::Idle,
            SectorState::Running {
                params,
                disk_bytes: span,
                steps_done: 1,
                steps_total: 4,
                bytes_read: span,
                elapsed_ms: 1_000,
                errors: Vec::new(),
            },
            SectorState::Done(scan),
            SectorState::Failed {
                params: None,
                message: "brak urządzenia".to_string(),
            },
        ];
        let labels: Vec<String> = states
            .iter()
            .map(|state| sector_state_label(&translator, state).0)
            .collect();

        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            labels.len(),
            "stany dają różne etykiety: {labels:?}"
        );
        // W trakcie skanu etykieta to procent — resztę mówi karta stanu.
        assert!(labels[1].contains('%'), "postęp w etykiecie: {labels:?}");
    }
}
