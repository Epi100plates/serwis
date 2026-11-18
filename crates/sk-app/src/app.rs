//! Okno aplikacji: lista modułów, generyczny renderer widoków, wątek roboczy.
//!
//! Zbieranie danych nigdy nie dzieje się na wątku UI — moduły mogą odpytywać
//! WMI setki milisekund, a w kolejnych fazach dojdą benchmarki dysków.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::Duration;

use eframe::egui;
use sk_core::i18n::{Locale, Translator};
use sk_core::module::{Capability, CollectCtx, ModuleMeta, ModuleOutput};
use sk_core::registry::ModuleRegistry;
use sk_win::{SystemClock, WinHostEnv, WinWmi};

use crate::render;

enum Job {
    CollectAll,
}

enum Event {
    Module {
        id: &'static str,
        result: Result<ModuleOutput, String>,
    },
    Done,
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
    pending: bool,
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

        let worker = thread::Builder::new()
            .name("serwiskit-worker".to_string())
            .spawn(move || worker_loop(registry, job_rx, event_tx));

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
            pending: false,
            startup_error,
        };
        app.request_refresh();
        app
    }

    fn request_refresh(&mut self) {
        self.pending = true;
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::CollectAll);
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
                Ok(Event::Done) => {
                    self.pending = false;
                    received = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
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
                if ui.button(self.translator.t("ui.refresh")).clicked() {
                    self.request_refresh();
                }
                if self.pending {
                    ui.spinner();
                }
            });
        });
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
        if self.pending {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("modules")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| self.module_list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.content(ui));
    }
}

fn worker_loop(registry: ModuleRegistry, jobs: Receiver<Job>, events: Sender<Event>) {
    let wmi = WinWmi::new();
    let clock = SystemClock::new();
    let host = WinHostEnv::new();

    while let Ok(job) = jobs.recv() {
        match job {
            Job::CollectAll => {
                let ctx = CollectCtx {
                    wmi: &wmi,
                    clock: &clock,
                    host: &host,
                    locale: Locale::Pl,
                };
                for module in registry.modules() {
                    let meta = module.meta();
                    let result = module.collect(&ctx).map_err(|err| err.to_string());
                    if events
                        .send(Event::Module {
                            id: meta.id,
                            result,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                if events.send(Event::Done).is_err() {
                    return;
                }
            }
        }
    }
}

fn capability_color(capability: Capability) -> egui::Color32 {
    match capability {
        Capability::Userland => egui::Color32::from_rgb(120, 170, 120),
        Capability::Elevated => egui::Color32::from_rgb(200, 170, 90),
        Capability::Destructive => egui::Color32::from_rgb(210, 100, 100),
    }
}
