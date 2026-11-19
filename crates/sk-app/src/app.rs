//! Okno aplikacji: lista modułów, generyczny renderer widoków, wątek roboczy.
//!
//! Zbieranie danych nigdy nie dzieje się na wątku UI — moduły odpyłują WMI
//! setki milisekund, a telemetria próbkuje co sekundę. Wątek roboczy trzyma
//! też historię próbek (`LiveHistory`), więc `system.live` jest bezstanowe.

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
use sk_core::surface::{DEFAULT_TIME_LIMIT_MS, SurfaceParams, SurfaceState, SurfaceStore};
use sk_win::{SystemClock, WinHostEnv, WinMachine, WinStorage, WinWmi};

use crate::live::sample;
use crate::render;
use crate::report::{Report, ReportOptions};

const LIVE_MODULE_ID: &str = "system.live";
const LIVE_INTERVAL: Duration = Duration::from_secs(1);
/// 600 próbek = 10 minut historii przy próbkowaniu co sekundę.
const LIVE_HISTORY_CAPACITY: usize = 600;
const BENCH_MODULE_ID: &str = "storage.bench";
/// Odświeżanie widoku w trakcie testu — 4 razy na sekundę wystarcza, żeby
/// wykres rósł płynnie, a wątek roboczy nie był zajęty ciągłym zbieraniem.
const BENCH_REFRESH_INTERVAL: Duration = Duration::from_millis(400);

enum Job {
    CollectAll,
    SampleLive,
    CollectBench,
    ListDrives,
    ExportHtml { path: PathBuf, locale: Locale },
}

enum Event {
    Module {
        id: &'static str,
        result: Result<ModuleOutput, String>,
    },
    /// Aktualizacja `system.live` — nie zwalnia blokady `CollectAll`.
    LiveSample(Result<ModuleOutput, String>),
    /// Aktualizacja `storage.bench` w trakcie lub po teście.
    Bench(Result<ModuleOutput, String>),
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
    status: Option<Result<String, String>>,
    startup_error: Option<String>,
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

        let worker = {
            let surface = Arc::clone(&surface);
            thread::Builder::new()
                .name("serwiskit-worker".to_string())
                .spawn(move || worker_loop(registry, job_rx, event_tx, surface))
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

    /// Jedna próbka telemetrii na sekundę, tylko gdy moduł live jest otwarty.
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

    fn start_bench(&mut self) {
        if self.bench_running {
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

    /// Panel sterowania testem. To jedyny fragment GUI wiedzący o konkretnym
    /// module — reszta interfejsu renderuje wyłącznie model widoku.
    fn bench_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(self.translator.t("ui.bench.title")).strong());
        ui.horizontal(|ui| {
            ui.label(self.translator.t("ui.bench.drive"));
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
                .unwrap_or_else(|| self.translator.t("ui.bench.no_drives"));
            let mut drive = self.bench_drive;
            egui::ComboBox::from_id_salt("bench-drive")
                .selected_text(selected)
                .width(320.0)
                .show_ui(ui, |ui| {
                    for (index, label) in &drive_labels {
                        ui.selectable_value(&mut drive, Some(*index), label);
                    }
                });
            self.bench_drive = drive;

            ui.label(self.translator.t("ui.bench.speed"));
            let mut speed = self.bench_speed;
            egui::ComboBox::from_id_salt("bench-speed")
                .selected_text(self.translator.t(speed.label_key()))
                .show_ui(ui, |ui| {
                    for candidate in BenchSpeed::ALL {
                        ui.selectable_value(
                            &mut speed,
                            candidate,
                            self.translator.t(candidate.label_key()),
                        );
                    }
                });
            self.bench_speed = speed;

            if self.bench_running {
                if ui.button(self.translator.t("ui.bench.cancel")).clicked() {
                    self.cancel_bench();
                }
            } else if ui
                .add_enabled(
                    !self.bench_drives.is_empty(),
                    egui::Button::new(self.translator.t("ui.bench.start")),
                )
                .clicked()
            {
                self.start_bench();
            }
        });
        ui.label(
            egui::RichText::new(self.translator.t("ui.bench.speed_hint"))
                .small()
                .weak(),
        );

        let state = self.surface.get();
        // Postęp liczy się z zegara, gdy ustawiono limit czasu — ten sam
        // rachunek co w module (`SurfaceState::progress_percent`).
        let progress = state.progress_percent().unwrap_or(0.0);
        match state {
            SurfaceState::Running { samples, .. } => {
                ui.add(
                    egui::ProgressBar::new((progress / 100.0) as f32)
                        .text(format!("{progress:.0} %"))
                        .desired_width(420.0),
                );
                if let Some(sample) = samples.last() {
                    ui.label(format!(
                        "{}: {:.1} MB/s",
                        self.translator.t("ui.bench.current"),
                        sample.speed_mbps
                    ));
                }
            }
            SurfaceState::Done(scan) => {
                let text = if self.bench_running {
                    self.translator.t("ui.bench.running")
                } else {
                    self.translator.t("ui.bench.finished")
                };
                let avg = average_speed(&scan.samples);
                ui.label(
                    egui::RichText::new(format!("{text} · {avg:.1} MB/s"))
                        .small()
                        .weak(),
                );
            }
            SurfaceState::Failed { message, .. } => {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 90, 90),
                    format!("{}: {message}", self.translator.t("ui.bench.failed")),
                );
            }
            SurfaceState::Idle => {
                ui.label(
                    egui::RichText::new(self.translator.t("ui.bench.waiting"))
                        .small()
                        .weak(),
                );
            }
        }
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
                Ok(Event::LiveSample(result)) => {
                    self.live_pending = false;
                    self.results.insert(LIVE_MODULE_ID, result);
                    received = true;
                }
                Ok(Event::Bench(result)) => {
                    self.bench_pending = false;
                    self.results.insert(BENCH_MODULE_ID, result);
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

        if self.selected == Some(LIVE_MODULE_ID) {
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
        self.poll_bench(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("modules")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| self.module_list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.content(ui));
    }
}

fn worker_loop(
    registry: ModuleRegistry,
    jobs: Receiver<Job>,
    events: Sender<Event>,
    surface: Arc<SurfaceStore>,
) {
    let wmi = WinWmi::new();
    let clock = SystemClock::new();
    let host = WinHostEnv::new();
    let machine = WinMachine::new();
    let storage = WinStorage::new();
    let mut history = LiveHistory::new(LIVE_HISTORY_CAPACITY);
    let started = Instant::now();
    let locale = Locale::Pl;

    while let Ok(job) = jobs.recv() {
        match job {
            Job::CollectAll => {
                wmi.begin_round();
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
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

                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
                    surface: surface.as_ref(),
                    locale,
                };
                let Some(module) = registry.find(LIVE_MODULE_ID) else {
                    continue;
                };
                let result = module.collect(&ctx).map_err(|err| err.to_string());
                if events.send(Event::LiveSample(result)).is_err() {
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
            Job::ListDrives => {
                wmi.begin_round();
                let drives = list_drives(&wmi);
                if events.send(Event::Drives(drives)).is_err() {
                    return;
                }
            }
            Job::ExportHtml { path, locale } => {
                wmi.begin_round();
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    machine: &machine,
                    storage: &storage,
                    live: &history,
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

fn average_speed(samples: &[sk_core::surface::SurfaceSample]) -> f64 {
    let speeds: Vec<f64> = samples
        .iter()
        .map(|sample| sample.speed_mbps)
        .filter(|speed| speed.is_finite() && *speed > 0.0)
        .collect();
    if speeds.is_empty() {
        return 0.0;
    }
    speeds.iter().sum::<f64>() / speeds.len() as f64
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
}
