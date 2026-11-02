//! Interface graphique (egui).

use crate::calibrate::{self, CalibrationResult};
use crate::engine::{self, AudioBackend, CpalBackend, DeviceLists, Engine, INTERNAL_RATE, Shared};
use crate::monitor::{self, CpalMonitor, Monitor, MonitorBackend, MonitorShared};
use crate::params::{DenoiseEngine, EqKind, Params, builtin_presets};
use crate::settings::Settings;
use crate::vbcable::{self, Outcome, Step};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

enum Calib {
    Idle,
    Intro,
    Silence(Instant),
    Keyboard(Instant),
    Speech(Instant),
    Computing { rx: Receiver<CalibMsg>, progress: f32 },
    Done { result: Box<CalibrationResult>, previous: Box<Params>, listening_new: bool },
    Failed(String),
}

/// Installation de VB-Cable lancée depuis l'en-tête.
enum VbInstall {
    Idle,
    Running { rx: Receiver<VbMsg>, step: Option<Step> },
    Done(Outcome),
}

enum VbMsg {
    Step(Step),
    Done(Outcome),
}

/// Fonction qui installe VB-Cable ; remplaçable dans les tests.
pub type VbInstaller = Arc<dyn Fn(&mut dyn FnMut(Step)) -> Outcome + Send + Sync>;

enum CalibMsg {
    Progress(f32),
    Done(Box<CalibrationResult>),
}

/// Application egui : fenêtre principale et assistant de calibrage.
pub struct App {
    backend: Box<dyn AudioBackend>,
    /// `None` : ne rien écrire sur le disque (tests).
    settings_path: Option<PathBuf>,
    settings: Settings,
    shared: Arc<Shared>,
    engine: Option<Engine>,
    error: Option<String>,
    devices: DeviceLists,
    calib: Calib,
    held_in: f32,
    held_out: f32,
    last_save: Option<Instant>,
    /// Dernière lecture de la liste des périphériques.
    last_scan: Instant,
    /// Intervalle de relecture des périphériques tant que le son est arrêté.
    rescan_every: Duration,
    vb_install: VbInstall,
    vb_installer: VbInstaller,
    monitor_backend: Box<dyn MonitorBackend>,
    monitor_shared: Arc<MonitorShared>,
    monitor: Option<Monitor>,
    /// Sortie choisie pour le retour (`None` : sortie par défaut de Windows).
    monitor_sink: Option<String>,
    /// Volume du retour, en pourcentage.
    monitor_volume: f32,
    monitor_error: Option<String>,
}

/// Volume de départ du retour casque (pourcentage).
const MONITOR_DEFAULT_VOLUME: f32 = 80.0;

/// Intervalle par défaut de relecture des périphériques quand le moteur est
/// arrêté : un câble virtuel ou un micro branché après le lancement est vu seul.
pub const DEVICE_RESCAN: Duration = Duration::from_millis(2500);

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").field("engine", &self.engine).field("error", &self.error).finish_non_exhaustive()
    }
}

impl App {
    /// L'application réelle : carte son via cpal, réglages dans `%APPDATA%`.
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_backend(Settings::load(), Box::new(CpalBackend), crate::settings::default_path())
    }

    /// Construit l'application avec un backend audio et un emplacement de
    /// sauvegarde au choix (les tests passent un faux backend et `None`).
    pub fn with_backend(settings: Settings, backend: Box<dyn AudioBackend>, settings_path: Option<PathBuf>) -> Self {
        let shared = Shared::new(settings.params.clone());
        let mut app = Self {
            devices: backend.list_devices(),
            backend,
            settings_path,
            settings,
            shared,
            engine: None,
            error: None,
            calib: Calib::Idle,
            held_in: -90.0,
            held_out: -90.0,
            last_save: None,
            last_scan: Instant::now(),
            rescan_every: DEVICE_RESCAN,
            vb_install: VbInstall::Idle,
            monitor_backend: Box::new(CpalMonitor),
            monitor_shared: MonitorShared::new(MONITOR_DEFAULT_VOLUME / 100.0),
            monitor: None,
            monitor_sink: None,
            monitor_volume: MONITOR_DEFAULT_VOLUME,
            monitor_error: None,
            vb_installer: Arc::new(|progress: &mut dyn FnMut(Step)| {
                vbcable::install(&vbcable::PowerShell, &vbcable::default_work_dir(), vbcable::Arch::current(), progress)
            }),
        };
        // Clé en main : on démarre tout de suite avec les derniers périphériques.
        app.start_engine();
        if !app.settings.calibrated && app.engine.is_some() {
            app.calib = Calib::Intro;
        }
        app
    }

    fn start_engine(&mut self) {
        self.start_engine_once();
        // Jamais la sortie du câble comme micro (boucle sans fin) : Windows en
        // fait souvent le micro par défaut juste après l'installation de
        // VB-Cable. On bascule alors sur le premier vrai micro.
        let looped = self.engine.as_ref().is_some_and(|e| vbcable::is_cable_loopback_input(&e.input_name))
            || self.settings.input_device.as_deref().is_some_and(vbcable::is_cable_loopback_input);
        if looped {
            self.engine = None;
            if let Some(mic) = vbcable::first_real_microphone(&self.devices.inputs) {
                self.settings.input_device = Some(mic);
                self.start_engine_once();
            } else {
                self.settings.input_device = None;
                self.error = Some("Aucun vrai micro trouvé : branche ton micro puis clique sur Rafraîchir.".into());
            }
        }
    }

    fn start_engine_once(&mut self) {
        self.engine = None;
        let out = self.settings.output_device.clone().or_else(|| engine::find_virtual_cable(&self.devices.outputs));
        match self.backend.start(&self.shared, self.settings.input_device.as_deref(), out.as_deref()) {
            Ok(e) => {
                self.settings.input_device = Some(e.input_name.clone());
                self.settings.output_device = Some(e.output_name.clone());
                self.engine = Some(e);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn apply_params(&mut self, p: Params) {
        self.settings.params = p;
        self.shared.set_params(&self.settings.params);
        self.last_save = Some(Instant::now());
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Micro Studio");
            ui.separator();
            match &self.engine {
                Some(e) => {
                    ui.label(RichText::new("• Actif").color(Color32::from_rgb(60, 180, 90)));
                    ui.label(format!("{} vers {}", e.input_name, e.output_name)).on_hover_text(format!(
                        "Micro à {} Hz, sortie à {} Hz, traitement interne à {} Hz",
                        e.input_rate, e.output_rate, INTERNAL_RATE
                    ));
                }
                None => {
                    ui.label(RichText::new("• Arrêté").color(Color32::from_rgb(200, 80, 60)));
                }
            }
        });

        let mut restart = false;
        ui.horizontal(|ui| {
            let mut input = self.settings.input_device.clone();
            // Libellé avant la liste : on lit « Micro : … » puis « Sortie : … ».
            let label = ui.label("Micro :");
            egui::ComboBox::from_id_salt("micro")
                .selected_text(input.clone().unwrap_or_else(|| "Par défaut".into()))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for n in self.devices.inputs.iter().filter(|n| !vbcable::is_cable_loopback_input(n)) {
                        ui.selectable_value(&mut input, Some(n.clone()), n);
                    }
                })
                .response
                .labelled_by(label.id);
            if input != self.settings.input_device {
                self.settings.input_device = input;
                restart = true;
            }

            let mut output = self.settings.output_device.clone();
            ui.add_space(8.0);
            let label = ui.label("Sortie (micro virtuel) :");
            egui::ComboBox::from_id_salt("sortie")
                .selected_text(output.clone().unwrap_or_else(|| "VB-Cable".into()))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for n in &self.devices.outputs {
                        ui.selectable_value(&mut output, Some(n.clone()), n);
                    }
                })
                .response
                .labelled_by(label.id);
            if output != self.settings.output_device {
                self.settings.output_device = output;
                restart = true;
            }

            if ui.button("Rafraîchir").on_hover_text("Rafraîchir la liste des périphériques").clicked() {
                self.devices = self.backend.list_devices();
            }
            if self.engine.is_some() {
                if ui.button("Arrêter").clicked() {
                    self.engine = None;
                }
            } else if ui.button("Démarrer").clicked() {
                restart = true;
            }
        });
        if restart {
            self.start_engine();
            self.last_save = Some(Instant::now());
        }

        if engine::find_virtual_cable(&self.devices.outputs).is_none() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("VB-Cable n'est pas installé : c'est lui qui crée le micro virtuel.")
                        .color(Color32::from_rgb(220, 160, 40)),
                );
            });
            self.vb_install_ui(ui);
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(Color32::from_rgb(220, 80, 60)));
        }
    }

    fn meters(&mut self, ui: &mut egui::Ui) {
        let m = &self.shared.meters;
        // Les crêtes redescendent doucement, comme sur une vraie console.
        self.held_in = m.in_peak_db.load().max(self.held_in - 1.2);
        self.held_out = m.out_peak_db.load().max(self.held_out - 1.2);

        egui::Grid::new("meters").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            meter_row(ui, "Entrée", self.held_in);
            ui.end_row();
            meter_row(ui, "Sortie", self.held_out);
            ui.end_row();
            ui.label("Activité");
            ui.horizontal(|ui| {
                let open = m.gate_open.load(std::sync::atomic::Ordering::Relaxed);
                ui.label(if open {
                    RichText::new("Porte ouverte").color(Color32::from_rgb(60, 180, 90))
                } else {
                    RichText::new("Porte fermée").weak()
                });
                ui.separator();
                ui.label(format!("Voix détectée : {:.0} %", m.vad.load() * 100.0));
                ui.separator();
                ui.label(format!("Compresseur : {:.1} dB", m.comp_gr_db.load()));
                ui.separator();
                ui.label(format!("De-esser : {:.1} dB", m.deess_gr_db.load()));
            });
            ui.end_row();
        });
    }

    fn quick_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let enabled = self.engine.is_some() && matches!(self.calib, Calib::Idle);
            if ui
                .add_enabled(enabled, egui::Button::new(RichText::new("Calibrage automatique").strong()))
                .on_hover_text(
                    "Mesure ton micro, ta pièce et ta voix, puis trouve les meilleurs réglages (environ 1 minute)",
                )
                .clicked()
            {
                self.calib = Calib::Intro;
            }

            let mut p = self.settings.params.clone();
            ui.toggle_value(&mut p.bypass, "Comparer : son brut")
                .on_hover_text("Désactive tous les effets pour entendre la différence");
            if p != self.settings.params {
                self.apply_params(p);
            }

            ui.separator();
            let mut chosen: Option<Params> = None;
            let label = ui.label("Préréglage :");
            egui::ComboBox::from_id_salt("presets")
                .selected_text("Choisir…")
                .show_ui(ui, |ui| {
                    for (name, p) in builtin_presets() {
                        if ui.selectable_label(false, name).clicked() {
                            chosen = Some(p);
                        }
                    }
                    for (name, p) in &self.settings.user_presets {
                        if ui.selectable_label(false, format!("Perso : {name}")).clicked() {
                            chosen = Some(p.clone());
                        }
                    }
                })
                .response
                .labelled_by(label.id);
            if let Some(mut p) = chosen {
                // Un préréglage garde le gain d'entrée mesuré pour ce micro.
                p.input_gain_db = self.settings.params.input_gain_db;
                self.apply_params(p);
            }
            if ui.button("Enregistrer mes réglages").clicked() {
                let n = self.settings.user_presets.len() + 1;
                self.settings.user_presets.push((format!("Mes réglages {n}"), self.settings.params.clone()));
                self.last_save = Some(Instant::now());
            }
        });
    }

    fn effects(&mut self, ui: &mut egui::Ui) {
        let mut p = self.settings.params.clone();

        section(ui, "Gain", None, |ui| {
            ui.add(egui::Slider::new(&mut p.input_gain_db, -12.0..=30.0).text("Gain d'entrée").suffix(" dB"))
                .on_hover_text("Volume du micro avant traitement. Le calibrage le règle pour toi.");
            ui.add(egui::Slider::new(&mut p.output_gain_db, -12.0..=12.0).text("Gain de sortie").suffix(" dB"));
        });

        section(ui, "Filtre passe-haut", Some(&mut p.hpf_enabled), |ui| {
            ui.label(
                RichText::new("Coupe les graves inutiles : vibrations du bureau, souffle, « boum » des p et des b.")
                    .weak(),
            );
            ui.add(egui::Slider::new(&mut p.hpf_hz, 40.0..=200.0).text("Fréquence").suffix(" Hz").logarithmic(true));
        });

        noise_sections(ui, &mut p);
        dereverb_section(ui, &mut p);

        section(ui, "De-esser", Some(&mut p.deesser_enabled), |ui| {
            ui.label(RichText::new("Adoucit les « s » et « ch » trop sifflants.").weak());
            ui.add(
                egui::Slider::new(&mut p.deesser_freq_hz, 3000.0..=10000.0)
                    .text("Fréquence")
                    .suffix(" Hz")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut p.deesser_threshold_db, -60.0..=0.0).text("Seuil").suffix(" dB"));
            ui.add(egui::Slider::new(&mut p.deesser_max_reduction_db, 0.0..=12.0).text("Réduction max").suffix(" dB"));
        });

        section(ui, "Égaliseur", Some(&mut p.eq_enabled), |ui| {
            ui.label(RichText::new("Sculpte le timbre : moins de son « carton », plus de présence et d'air.").weak());
            let names = ["Corps", "Carton", "Présence", "Air"];
            for (b, name) in p.eq_bands.iter_mut().zip(names) {
                ui.horizontal(|ui| {
                    ui.label(format!("{name:<9}"));
                    ui.add(egui::Slider::new(&mut b.gain_db, -12.0..=12.0).text("Gain").suffix(" dB"));
                    ui.add(
                        egui::Slider::new(&mut b.freq_hz, 60.0..=16000.0).text("Fréq.").suffix(" Hz").logarithmic(true),
                    );
                    if b.kind == EqKind::Peak {
                        ui.add(egui::Slider::new(&mut b.q, 0.3..=4.0).text("Q"));
                    }
                });
            }
        });

        dynamic_eq_section(ui, &mut p);
        multiband_section(ui, &mut p);

        section(ui, "Compresseur", Some(&mut p.comp_enabled), |ui| {
            ui.label(RichText::new("Rend le volume régulier : les passages faibles et forts se rapprochent.").weak());
            ui.add(egui::Slider::new(&mut p.comp_threshold_db, -50.0..=0.0).text("Seuil").suffix(" dB"));
            ui.add(egui::Slider::new(&mut p.comp_ratio, 1.0..=20.0).text("Ratio").suffix(":1").logarithmic(true));
            ui.add(
                egui::Slider::new(&mut p.comp_attack_ms, 0.1..=100.0).text("Attaque").suffix(" ms").logarithmic(true),
            );
            ui.add(
                egui::Slider::new(&mut p.comp_release_ms, 10.0..=1000.0)
                    .text("Relâchement")
                    .suffix(" ms")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut p.comp_knee_db, 0.0..=12.0).text("Genou").suffix(" dB"));
            ui.add(egui::Slider::new(&mut p.comp_makeup_db, 0.0..=24.0).text("Gain de rattrapage").suffix(" dB"));
        });

        section(ui, "Limiteur", Some(&mut p.limiter_enabled), |ui| {
            ui.label(RichText::new("Plafond de sécurité : même si tu cries, ça ne sature jamais.").weak());
            ui.add(egui::Slider::new(&mut p.limiter_ceiling_db, -12.0..=0.0).text("Plafond").suffix(" dB"));
        });

        if p != self.settings.params {
            self.apply_params(p);
        }
    }

    #[expect(clippy::too_many_lines, reason = "machine à états de l'assistant, à découper avec la refonte de l'UI")]
    fn calibration_window(&mut self, ctx: &egui::Context) {
        if matches!(self.calib, Calib::Idle) {
            return;
        }
        let mut next: Option<Calib> = None;
        let mut apply: Option<Params> = None;

        egui::Window::new("Calibrage automatique")
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| match &mut self.calib {
                Calib::Idle => {}
                Calib::Intro => {
                    ui.label("Le logiciel va écouter ton micro pendant environ une minute pour trouver les meilleurs réglages :");
                    ui.label(format!("1. {} secondes de silence total, pour mesurer le bruit de ta pièce et de ton micro.", calibrate::SILENCE_SECS as u32));
                    ui.label(format!("2. {} secondes à taper au clavier sans parler, pour apprendre à couper les touches.", calibrate::KEYBOARD_SECS as u32));
                    ui.label("3. La lecture d'un court texte, à ta voix normale et à ta distance habituelle du micro.");
                    ui.label("4. Un tournoi entre une cinquantaine de réglages possibles ; le meilleur gagne.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("C'est parti").strong()).clicked() {
                            self.shared.start_recording();
                            next = Some(Calib::Silence(Instant::now()));
                        }
                        if ui.button("Plus tard").clicked() {
                            next = Some(Calib::Idle);
                        }
                    });
                }
                Calib::Silence(start) => {
                    let t = start.elapsed().as_secs_f32();
                    ui.heading("Silence, s'il te plaît");
                    ui.label("Ne parle pas et ne touche pas au clavier.");
                    ui.add(egui::ProgressBar::new(t / calibrate::SILENCE_SECS).show_percentage());
                    if t >= calibrate::SILENCE_SECS {
                        next = Some(Calib::Keyboard(Instant::now()));
                    }
                }
                Calib::Keyboard(start) => {
                    let t = start.elapsed().as_secs_f32();
                    ui.heading("Tape au clavier, sans parler");
                    ui.label("Écris n'importe quoi, comme pendant une partie ou une discussion. Clique aussi avec la souris.");
                    ui.add(egui::ProgressBar::new(t / calibrate::KEYBOARD_SECS).show_percentage());
                    if t >= calibrate::KEYBOARD_SECS {
                        next = Some(Calib::Speech(Instant::now()));
                    }
                }
                Calib::Speech(start) => {
                    let t = start.elapsed().as_secs_f32();
                    ui.heading("Lis ce texte à voix haute");
                    ui.add_space(4.0);
                    ui.label(RichText::new(calibrate::READING_TEXT).size(16.0));
                    ui.add_space(8.0);
                    ui.add(egui::ProgressBar::new(t / calibrate::SPEECH_SECS).text(format!("{t:.0} s")));
                    let done = t >= calibrate::SPEECH_SECS || (t > 15.0 && ui.button("J'ai fini de lire").clicked());
                    if done {
                        let recording = calibrate::split_recording(&self.shared.stop_recording());
                        let (tx, rx) = channel();
                        std::thread::spawn(move || {
                            let tx2 = tx.clone();
                            let r = calibrate::run(&recording, move |p| {
                                let _ = tx2.send(CalibMsg::Progress(p));
                            });
                            let _ = tx.send(CalibMsg::Done(Box::new(r)));
                        });
                        next = Some(Calib::Computing { rx, progress: 0.0 });
                    }
                }
                Calib::Computing { rx, progress } => {
                    ui.heading("Recherche des meilleurs réglages…");
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            CalibMsg::Progress(p) => *progress = p,
                            CalibMsg::Done(r) => {
                                if let Err(msg) = calibrate::check(&r) {
                                    next = Some(Calib::Failed(msg.into()));
                                } else {
                                    let previous = Box::new(self.settings.params.clone());
                                    apply = Some(r.params.clone());
                                    next = Some(Calib::Done { result: r, previous, listening_new: true });
                                }
                            }
                        }
                    }
                    ui.add(egui::ProgressBar::new(*progress).show_percentage());
                }
                Calib::Done { result, previous, listening_new } => {
                    let a = &result.analysis;
                    ui.heading("Calibrage terminé");
                    ui.label(format!("Bruit de fond : {:.0} dBFS · Voix : {:.0} dBFS · Écart voix/bruit : {:.0} dB", a.noise_floor_db, a.speech_level_db, a.snr_db));
                    ui.label(format!("Sifflantes autour de {:.1} kHz · Variations de volume : {:.0} dB", a.sibilance_freq_hz / 1000.0, a.dynamic_range_db));
                    if a.clipped() {
                        ui.label(RichText::new("Ton micro a saturé pendant la lecture : baisse son volume dans Windows (Paramètres > Son) puis relance le calibrage.").color(Color32::from_rgb(220, 160, 40)));
                    }
                    if !a.tonal_noise.is_empty() {
                        let freqs: Vec<String> = a.tonal_noise.iter().map(|t| format!("{:.0} Hz", t.freq_hz)).collect();
                        ui.label(format!("Bourdonnement repéré et filtré : {}.", freqs.join(", ")));
                    }
                    ui.add_space(6.0);
                    ui.label(format!("{} réglages testés. Avant / après :", result.candidates_tested));
                    report_grid(ui, &result.baseline_score, &result.score);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Écouter :");
                        if ui.selectable_label(*listening_new, "Nouveaux réglages").clicked() {
                            *listening_new = true;
                            apply = Some(result.params.clone());
                        }
                        if ui.selectable_label(!*listening_new, "Anciens réglages").clicked() {
                            *listening_new = false;
                            apply = Some((**previous).clone());
                        }
                    });
                    ui.label(RichText::new("Astuce : dans Windows, coche « Écouter ce périphérique » sur CABLE Output pour t'entendre.").weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Garder").strong()).clicked() {
                            apply = Some(result.params.clone());
                            self.settings.calibrated = true;
                            next = Some(Calib::Idle);
                        }
                        if ui.button("Annuler").clicked() {
                            apply = Some((**previous).clone());
                            next = Some(Calib::Idle);
                        }
                    });
                }
                Calib::Failed(msg) => {
                    ui.label(msg.as_str());
                    if ui.button("Fermer").clicked() {
                        next = Some(Calib::Idle);
                    }
                }
            });

        if let Some(p) = apply {
            self.apply_params(p);
        }
        if let Some(n) = next {
            if matches!(n, Calib::Idle) {
                // Si on quitte en plein enregistrement, on l'arrête proprement.
                let _ = self.shared.stop_recording();
            }
            self.calib = n;
        }
    }
}

/// Sections contre les bruits : bourdonnement, porte, suppression par IA.
fn noise_sections(ui: &mut egui::Ui, p: &mut Params) {
    section(ui, "Anti-bourdonnement", Some(&mut p.notch_enabled), |ui| {
        ui.label(
            RichText::new("Retire les « zzzz » continus (secteur électrique, USB). Le calibrage les repère.").weak(),
        );
        let found: Vec<String> = p.notch_freqs_hz.iter().filter(|&&f| f > 0.0).map(|f| format!("{f:.0} Hz")).collect();
        if found.is_empty() {
            ui.label("Aucun bourdonnement repéré.");
        } else {
            ui.label(format!("Filtré : {}", found.join(", ")));
        }
    });

    section(ui, "Noise gate", Some(&mut p.gate_enabled), |ui| {
        ui.label(RichText::new("Coupe le micro quand tu ne parles pas.").weak());
        ui.add(egui::Slider::new(&mut p.gate_threshold_db, -80.0..=-10.0).text("Seuil").suffix(" dB"))
            .on_hover_text("En dessous de ce niveau, la porte se ferme. Trop haut : la fin des mots est coupée.");
        ui.add(egui::Slider::new(&mut p.gate_range_db, -80.0..=0.0).text("Atténuation").suffix(" dB"));
        ui.add(egui::Slider::new(&mut p.gate_hold_ms, 0.0..=500.0).text("Maintien").suffix(" ms"));
        ui.add(egui::Slider::new(&mut p.gate_release_ms, 10.0..=1000.0).text("Relâchement").suffix(" ms"));
    });

    section(ui, "Suppression de bruit (IA)", Some(&mut p.denoise_enabled), |ui| {
        ui.label(RichText::new("Un réseau de neurones retire ventilateur, clavier, souffle.").weak());
        ui.horizontal(|ui| {
            ui.label("Moteur :");
            ui.radio_value(&mut p.denoise_engine, DenoiseEngine::RnNoise, "RNNoise (léger)")
                .on_hover_text("Peu gourmand, 10 ms de retard.");
            ui.radio_value(&mut p.denoise_engine, DenoiseEngine::DeepFilter, "DeepFilterNet (premium)").on_hover_text(
                "Plus efficace sur le clavier et les bruits brefs, son plus naturel dans les aigus. 30 ms de retard.",
            );
        });
        ui.add(egui::Slider::new(&mut p.denoise_mix, 0.0..=1.0).text("Intensité"));
        ui.checkbox(&mut p.voice_gate_enabled, "Couper tout ce qui n'est pas de la voix (clavier, souris)")
            .on_hover_text(
                "Le réseau de neurones ferme le micro quand il n'entend pas de voix, même si le bruit est fort.",
            );
        ui.add_enabled(
            p.voice_gate_enabled,
            egui::Slider::new(&mut p.voice_gate_threshold, 0.1..=0.9).text("Exigence"),
        )
        .on_hover_text(
            "Plus c'est haut, plus il faut être sûr que c'est de la voix. Trop haut : des mots sont coupés.",
        );
    });
}

/// Réglages de l'anti-réverbération.
fn dereverb_section(ui: &mut egui::Ui, p: &mut Params) {
    section(ui, "Anti-réverbération", Some(&mut p.dereverb_enabled), |ui| {
        ui.label(
            RichText::new("Retire l'écho de la pièce (voix lointaine, « salle de bain »). Ajoute 10 ms de retard.")
                .weak(),
        );
        ui.add(egui::Slider::new(&mut p.dereverb_strength, 0.0..=1.0).text("Intensité"));
        ui.add(egui::Slider::new(&mut p.dereverb_t60_s, 0.2..=1.5).text("Écho de la pièce").suffix(" s"))
            .on_hover_text("Durée de l'écho : 0,3 s pour une chambre meublée, 1 s pour une pièce vide et carrelée.");
    });
}

/// Réglages de l'égaliseur dynamique.
fn dynamic_eq_section(ui: &mut egui::Ui, p: &mut Params) {
    section(ui, "Égaliseur dynamique", Some(&mut p.deq_enabled), |ui| {
        ui.label(
            RichText::new("Adoucit le son « carton » et la dureté seulement quand ils débordent, pas en permanence.")
                .weak(),
        );
        let names = ["Carton", "Dureté"];
        for (b, name) in p.deq_bands.iter_mut().zip(names) {
            ui.horizontal(|ui| {
                ui.label(format!("{name:<7}"));
                ui.add(egui::Slider::new(&mut b.threshold_db, -60.0..=0.0).text("Seuil").suffix(" dB"));
                ui.add(egui::Slider::new(&mut b.max_cut_db, 0.0..=12.0).text("Max").suffix(" dB"));
                ui.add(egui::Slider::new(&mut b.freq_hz, 150.0..=8000.0).text("Fréq.").suffix(" Hz").logarithmic(true));
            });
        }
    });
}

/// Réglages du compresseur multibande.
fn multiband_section(ui: &mut egui::Ui, p: &mut Params) {
    section(ui, "Compresseur multibande", Some(&mut p.mb_enabled), |ui| {
        ui.label(
            RichText::new(
                "Tient séparément les graves, les médiums et les aigus : une voix régulière sans l'étouffer.",
            )
            .weak(),
        );
        let names = ["Graves", "Médiums", "Aigus"];
        for (i, name) in names.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{name:<8}"));
                ui.add(egui::Slider::new(&mut p.mb_threshold_db[i], -50.0..=0.0).text("Seuil").suffix(" dB"));
                ui.add(egui::Slider::new(&mut p.mb_ratio[i], 1.0..=8.0).text("Ratio").suffix(":1").logarithmic(true));
            });
        }
        ui.add(
            egui::Slider::new(&mut p.mb_crossovers_hz[0], 100.0..=600.0)
                .text("Séparation graves / médiums")
                .suffix(" Hz")
                .logarithmic(true),
        );
        ui.add(
            egui::Slider::new(&mut p.mb_crossovers_hz[1], 2000.0..=8000.0)
                .text("Séparation médiums / aigus")
                .suffix(" Hz")
                .logarithmic(true),
        );
    });
}

/// Tableau « sans traitement / avec tes réglages » du rapport de calibrage.
fn report_grid(ui: &mut egui::Ui, before: &calibrate::Score, after: &calibrate::Score) {
    egui::Grid::new("rapport_calibrage").striped(true).num_columns(3).show(ui, |ui| {
        ui.label("");
        ui.label(RichText::new("Sans traitement").strong());
        ui.label(RichText::new("Avec tes réglages").strong());
        ui.end_row();
        for (label, f) in report_rows() {
            ui.label(label);
            ui.label(f(before));
            ui.label(f(after));
            ui.end_row();
        }
    });
}

type ReportRow = (&'static str, fn(&calibrate::Score) -> String);

/// Lignes du rapport : libellé et mise en forme de la mesure.
fn report_rows() -> [ReportRow; 9] {
    [
        ("Qualité perçue, note globale (DNSMOS)", |s| mos_label(s.mos.map(|m| m.ovrl))),
        ("Clarté de la voix (DNSMOS)", |s| mos_label(s.mos.map(|m| m.sig))),
        ("Discrétion du bruit (DNSMOS)", |s| mos_label(s.mos.map(|m| m.bak))),
        ("Voix au-dessus du bruit de fond", |s| format!("{:.0} dB", s.noise_reduction_db)),
        ("Voix au-dessus du clavier", |s| format!("{:.0} dB", s.keyboard_reduction_db)),
        ("Volume (cible -18 LUFS)", |s| format!("{:.1} LUFS", s.loudness_lufs)),
        ("Régularité du volume", |s| format!("±{:.0} dB", s.level_spread_db / 2.0)),
        ("Mots coupés", |s| format!("{:.0} %", s.words_cut_ratio * 100.0)),
        ("Équilibre de la voix (écart)", |s| format!("{:.1} dB", s.tonal_error_db)),
    ]
}

/// Note DNSMOS « 3,8 / 5 », ou un tiret si elle n'a pas été calculée.
fn mos_label(v: Option<f32>) -> String {
    v.map_or_else(|| "—".to_string(), |v| format!("{v:.1} / 5").replace('.', ","))
}

fn meter_row(ui: &mut egui::Ui, label: &str, db: f32) {
    ui.label(label);
    let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let color = if db > -3.0 {
        Color32::from_rgb(220, 70, 60)
    } else if db > -12.0 {
        Color32::from_rgb(230, 180, 50)
    } else {
        Color32::from_rgb(60, 180, 90)
    };
    ui.add(egui::ProgressBar::new(frac).fill(color).desired_width(420.0).text(if db < -59.0 {
        "—".to_string()
    } else {
        format!("{db:.0} dB")
    }));
}

fn section(ui: &mut egui::Ui, title: &str, enabled: Option<&mut bool>, body: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        let on = if let Some(e) = enabled {
            ui.checkbox(e, RichText::new(title).strong());
            *e
        } else {
            ui.label(RichText::new(title).strong());
            true
        };
        ui.add_enabled_ui(on, body);
    });
}

impl App {
    /// Réglages courants.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Vrai si le moteur audio tourne.
    pub fn is_running(&self) -> bool {
        self.engine.is_some()
    }

    /// Dernier message d'erreur affiché.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Vrai si l'assistant de calibrage est ouvert.
    pub fn calibration_open(&self) -> bool {
        !matches!(self.calib, Calib::Idle)
    }

    /// Remplace l'installation de VB-Cable (les tests évitent ainsi le réseau
    /// et PowerShell).
    pub fn set_vb_installer(&mut self, installer: VbInstaller) {
        self.vb_installer = installer;
    }

    /// Bouton d'installation de VB-Cable et suivi de son déroulement.
    fn vb_install_ui(&mut self, ui: &mut egui::Ui) {
        match &self.vb_install {
            VbInstall::Running { step, .. } => {
                ui.label(step.map_or("Préparation de l'installation…", Step::label));
            }
            VbInstall::Idle | VbInstall::Done(_) => {
                if let VbInstall::Done(outcome) = &self.vb_install {
                    let text = match outcome {
                        Outcome::Finished => {
                            format!("{outcome} Le son démarre dès que CABLE Input apparaît ; sinon, redémarre le PC.")
                        }
                        _ => outcome.to_string(),
                    };
                    ui.label(text);
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("Installer le micro virtuel (VB-Cable)")
                        .on_hover_text(
                            "Télécharge VB-Cable depuis le site officiel de VB-Audio et lance son installation",
                        )
                        .clicked()
                    {
                        self.start_vb_install();
                    }
                    ui.hyperlink_to("Soutenir VB-Audio", vbcable::ORIGIN_URL);
                });
            }
        }
        ui.label(RichText::new(vbcable::CREDIT).weak().small());
    }

    fn start_vb_install(&mut self) {
        let (tx, rx) = channel();
        let installer = Arc::clone(&self.vb_installer);
        std::thread::spawn(move || {
            let outcome = installer(&mut |step| {
                let _ = tx.send(VbMsg::Step(step));
            });
            let _ = tx.send(VbMsg::Done(outcome));
        });
        self.vb_install = VbInstall::Running { rx, step: None };
    }

    /// Relève les messages du fil d'installation de VB-Cable.
    fn poll_vb_install(&mut self) {
        let VbInstall::Running { rx, step } = &mut self.vb_install else {
            return;
        };
        let mut finished = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                VbMsg::Step(s) => *step = Some(s),
                VbMsg::Done(outcome) => finished = Some(outcome),
            }
        }
        if let Some(outcome) = finished {
            if outcome == Outcome::Finished {
                // Relecture immédiate des périphériques : démarrage automatique
                // si le câble est déjà là (voir `rescan_devices`).
                self.last_scan = Instant::now().checked_sub(self.rescan_every).unwrap_or_else(Instant::now);
            }
            self.vb_install = VbInstall::Done(outcome);
        }
    }

    /// Remplace l'accès matériel du retour casque (tests).
    pub fn set_monitor_backend(&mut self, backend: Box<dyn MonitorBackend>) {
        self.monitor = None;
        self.monitor_backend = backend;
    }

    /// Vrai si le retour casque est actif.
    pub fn is_monitoring(&self) -> bool {
        self.monitor.is_some()
    }

    /// Démarre (ou redémarre) le retour casque.
    fn start_monitor(&mut self) {
        self.monitor = None;
        let Some(source) = monitor::find_source(&self.devices.inputs) else {
            self.monitor_error =
                Some("Le micro virtuel (CABLE Output) est introuvable : installe d'abord VB-Cable.".into());
            return;
        };
        let default = self.monitor_backend.default_output();
        let Some(sink) = monitor::pick_sink(self.monitor_sink.as_deref(), default.as_deref(), &self.devices.outputs)
        else {
            self.monitor_error = Some("Aucun casque ni haut-parleur trouvé pour le retour.".into());
            return;
        };
        match self.monitor_backend.start(&self.monitor_shared, &source, &sink) {
            Ok(m) => {
                self.monitor = Some(m);
                self.monitor_error = None;
            }
            Err(e) => self.monitor_error = Some(format!("Retour casque impossible : {e:#}")),
        }
    }

    /// Ligne « M'écouter » : case à cocher, choix du casque et volume.
    fn monitor_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut on = self.monitor.is_some();
            ui.checkbox(&mut on, "M'écouter dans le casque").on_hover_text(
                "Rejoue dans ton casque ce que Discord ou OBS reçoivent, avec un léger décalage. \
                 À couper une fois les réglages faits.",
            );
            if on != self.monitor.is_some() {
                if on {
                    self.start_monitor();
                } else {
                    self.monitor = None;
                    self.monitor_error = None;
                }
            }

            let label = ui.label("Casque :");
            let current = self.monitor.as_ref().map(|m| m.sink.clone()).or_else(|| self.monitor_sink.clone());
            let mut wanted = self.monitor_sink.clone();
            egui::ComboBox::from_id_salt("casque")
                .selected_text(current.unwrap_or_else(|| "Par défaut".into()))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for n in monitor::sink_choices(&self.devices.outputs) {
                        ui.selectable_value(&mut wanted, Some(n.clone()), n);
                    }
                })
                .response
                .labelled_by(label.id);
            if wanted != self.monitor_sink {
                self.monitor_sink = wanted;
                if self.monitor.is_some() {
                    self.start_monitor();
                }
            }

            let label = ui.label("Volume :");
            let changed = ui
                .add(egui::Slider::new(&mut self.monitor_volume, 0.0..=100.0).suffix(" %").integer())
                .labelled_by(label.id)
                .changed();
            if changed {
                self.monitor_shared.volume.store(monitor::clamp_volume(self.monitor_volume / 100.0));
            }
        });
        if let Some(err) = &self.monitor_error {
            ui.label(RichText::new(err).color(Color32::from_rgb(220, 80, 60)));
        }
    }

    /// Change l'intervalle de relecture des périphériques (tests).
    pub fn set_device_rescan(&mut self, every: Duration) {
        self.rescan_every = every;
    }

    /// Tant que le son est arrêté, relit régulièrement les périphériques. Si
    /// aucune sortie n'a encore été choisie et que VB-Cable vient d'apparaître
    /// (installé pendant que le logiciel tournait), démarre tout seul.
    fn rescan_devices(&mut self) {
        if self.engine.is_some() || self.last_scan.elapsed() < self.rescan_every {
            return;
        }
        self.last_scan = Instant::now();
        self.devices = self.backend.list_devices();
        if self.settings.output_device.is_none() && engine::find_virtual_cable(&self.devices.outputs).is_some() {
            self.start_engine();
            if self.engine.is_some() && !self.settings.calibrated {
                self.calib = Calib::Intro;
            }
        }
    }

    /// Dessine toute l'interface (appelé à chaque image).
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll_vb_install();
        self.rescan_devices();
        let ctx = ui.ctx().clone();
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            self.header(ui);
            ui.add_space(4.0);
            self.meters(ui);
            ui.add_space(4.0);
            self.quick_actions(ui);
            self.monitor_row(ui);
            ui.add_space(6.0);
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.effects(ui);
            });
        });
        self.calibration_window(&ctx);

        if let Some(t) = self.last_save
            && t.elapsed() > Duration::from_millis(800)
        {
            if let Some(path) = &self.settings_path
                && let Err(e) = self.settings.save_to(path)
            {
                self.error = Some(format!("Impossible d'enregistrer les réglages : {e}"));
            }
            self.last_save = None;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}
