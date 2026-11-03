//! Tests de l'interface (niveau système, ISTQB) : l'application complète est
//! pilotée comme un utilisateur le ferait (clics, lecture des textes affichés),
//! avec un faux périphérique audio à la place de la carte son.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use micro_studio::engine::{AudioBackend, DeviceLists, Engine, Shared};
use micro_studio::monitor::{Monitor, MonitorBackend, MonitorShared};
use micro_studio::params::Params;
use micro_studio::settings::Settings;
use micro_studio::ui::App;
use micro_studio::vbcable::{Outcome, Step};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Calls = Arc<Mutex<Vec<(Option<String>, Option<String>)>>>;

/// Faux périphérique : enregistre chaque démarrage, peut simuler une panne, et
/// sa liste de périphériques peut changer en cours de test (câble installé).
/// Comme le vrai, il refuse de démarrer sans sortie si VB-Cable est absent.
struct FakeBackend {
    devices: Arc<Mutex<DeviceLists>>,
    fail: bool,
    calls: Calls,
}

impl AudioBackend for FakeBackend {
    fn list_devices(&self) -> DeviceLists {
        self.devices.lock().expect("verrou de test").clone()
    }

    fn start(&self, _: &Arc<Shared>, input: Option<&str>, output: Option<&str>) -> anyhow::Result<Engine> {
        self.calls.lock().expect("verrou de test").push((input.map(Into::into), output.map(Into::into)));
        if self.fail {
            anyhow::bail!("Aucun micro détecté");
        }
        if output.is_none() {
            anyhow::bail!("VB-Cable n'est pas installé : choisis une sortie ou installe VB-Cable");
        }
        // Micro par défaut de Windows : le premier de la liste.
        let default_input = self.devices.lock().expect("verrou de test").inputs.first().cloned();
        let input = input.map(str::to_owned).or(default_input).unwrap_or_else(|| "Micro USB".to_owned());
        Ok(Engine::detached(&input, output.unwrap_or("Sortie"), 48_000, 48_000))
    }
}

fn devices(with_cable: bool) -> DeviceLists {
    let mut outputs = vec!["Haut-parleurs".to_owned()];
    if with_cable {
        outputs.push("CABLE Input (VB-Audio Virtual Cable)".to_owned());
    }
    DeviceLists { inputs: vec!["Micro USB".to_owned()], outputs }
}

fn app(settings: Settings, with_cable: bool, fail: bool) -> (Harness<'static, App>, Calls) {
    let (h, calls, _) = app_with_devices(settings, with_cable, fail);
    (h, calls)
}

fn app_with_devices(
    settings: Settings,
    with_cable: bool,
    fail: bool,
) -> (Harness<'static, App>, Calls, Arc<Mutex<DeviceLists>>) {
    let calls = Calls::default();
    let shared_devices = Arc::new(Mutex::new(devices(with_cable)));
    let backend = FakeBackend { devices: Arc::clone(&shared_devices), fail, calls: Arc::clone(&calls) };
    // `None` : l'application n'écrit jamais sur le disque pendant les tests.
    let app = App::with_backend(settings, Box::new(backend), None);
    let mut harness =
        Harness::builder().with_size([900.0, 760.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run_steps(2);
    (harness, calls, shared_devices)
}

fn calibrated() -> Settings {
    Settings { calibrated: true, ..Settings::default() }
}

#[test]
fn starts_automatically_on_the_virtual_cable() {
    let (h, calls) = app(calibrated(), true, false);
    assert!(h.state().is_running());
    let calls = calls.lock().expect("verrou");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.as_deref(), Some("CABLE Input (VB-Audio Virtual Cable)"));
    drop(calls);
    h.get_by_label_contains("Actif");
    assert!(h.query_by_label_contains("c'est lui qui crée le micro virtuel").is_none());
    assert_eq!(h.state().settings().output_device.as_deref(), Some("CABLE Input (VB-Audio Virtual Cable)"));
}

#[test]
fn first_launch_opens_the_calibration_assistant() {
    let (mut h, _) = app(Settings::default(), true, false);
    assert!(h.state().calibration_open());
    h.get_by_label("Plus tard").click();
    h.run_steps(2);
    assert!(!h.state().calibration_open());
}

#[test]
fn calibration_starts_with_the_silence_step() {
    let (mut h, _) = app(Settings::default(), true, false);
    h.get_by_label("C'est parti").click();
    h.run_steps(2);
    h.get_by_label_contains("Silence");
    assert!(h.state().calibration_open());
}

#[test]
fn calibration_button_reopens_the_assistant() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().calibration_open());
    h.get_by_label_contains("Calibrage automatique").click();
    h.run_steps(2);
    assert!(h.state().calibration_open());
}

#[test]
fn audio_failure_is_shown_and_does_not_open_the_assistant() {
    let (h, _) = app(Settings::default(), true, true);
    assert!(!h.state().is_running());
    assert!(h.state().error().is_some_and(|e| e.contains("Aucun micro")));
    h.get_by_label_contains("Aucun micro détecté");
    h.get_by_label_contains("Arrêté");
    assert!(!h.state().calibration_open());
}

#[test]
fn missing_vb_cable_offers_a_one_click_install() {
    let (h, _) = app(calibrated(), false, false);
    h.get_by_label_contains("c'est lui qui crée le micro virtuel");
    h.get_by_label("Installer le micro virtuel (VB-Cable)");
    // Mention exigée par VB-Audio pour proposer VB-Cable depuis une application.
    h.get_by_label_contains("donationware de VB-Audio");
    h.get_by_label("Soutenir VB-Audio");
}

/// Attend la fin du fil d'installation (simulé, donc quasi immédiat).
fn wait_for(h: &mut Harness<'static, App>, label: &str) {
    for _ in 0..200 {
        h.run_steps(1);
        if h.query_by_label_contains(label).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.query_by_label_contains(label).is_some(), "texte jamais affiché : {label}");
}

#[test]
fn one_click_install_starts_the_sound_when_the_cable_appears() {
    let (mut h, calls, shared_devices) = app_with_devices(calibrated(), false, false);
    assert!(!h.state().is_running());
    let installed = Arc::clone(&shared_devices);
    h.state_mut().set_vb_installer(Arc::new(move |progress: &mut dyn FnMut(Step)| {
        progress(Step::Downloading);
        progress(Step::WaitingForSetup);
        *installed.lock().expect("verrou") = devices(true);
        Outcome::Finished
    }));
    h.get_by_label("Installer le micro virtuel (VB-Cable)").click();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state().is_running(), "le son démarre seul après l'installation");
    assert!(h.query_by_label("Installer le micro virtuel (VB-Cable)").is_none());
    assert_eq!(
        calls.lock().expect("verrou").last().and_then(|c| c.1.clone()).as_deref(),
        Some("CABLE Input (VB-Audio Virtual Cable)")
    );
}

#[test]
fn install_cancelled_or_failed_is_explained_and_can_be_retried() {
    let (mut h, _) = app(calibrated(), false, false);
    h.state_mut().set_vb_installer(Arc::new(|_: &mut dyn FnMut(Step)| Outcome::Cancelled));
    h.get_by_label("Installer le micro virtuel (VB-Cable)").click();
    wait_for(&mut h, "Installation annulée");
    h.state_mut().set_vb_installer(Arc::new(|_: &mut dyn FnMut(Step)| Outcome::Failed("réseau coupé".into())));
    h.get_by_label("Installer le micro virtuel (VB-Cable)").click();
    wait_for(&mut h, "réseau coupé");
    assert!(!h.state().is_running());
}

#[test]
fn install_finished_but_cable_missing_suggests_a_reboot() {
    let (mut h, _) = app(calibrated(), false, false);
    h.state_mut().set_vb_installer(Arc::new(|progress: &mut dyn FnMut(Step)| {
        progress(Step::Verifying);
        Outcome::Finished
    }));
    h.get_by_label("Installer le micro virtuel (VB-Cable)").click();
    wait_for(&mut h, "redémarre le PC");
    assert!(!h.state().is_running());
}

#[test]
fn install_progress_is_shown_while_running() {
    let (mut h, _) = app(calibrated(), false, false);
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    h.state_mut().set_vb_installer(Arc::new(move |progress: &mut dyn FnMut(Step)| {
        progress(Step::WaitingForSetup);
        let _ = wait.lock().expect("verrou").recv();
        Outcome::Finished
    }));
    h.get_by_label("Installer le micro virtuel (VB-Cable)").click();
    wait_for(&mut h, "Install Driver");
    assert!(h.query_by_label("Installer le micro virtuel (VB-Cable)").is_none(), "pas de double lancement");
    release.send(()).expect("fil d'installation");
    wait_for(&mut h, "Installation de VB-Cable terminée");
}

#[test]
fn stop_and_start_buttons() {
    let (mut h, calls) = app(calibrated(), true, false);
    h.get_by_label("Arrêter").click();
    h.run_steps(2);
    assert!(!h.state().is_running());
    h.get_by_label("Démarrer").click();
    h.run_steps(2);
    assert!(h.state().is_running());
    assert_eq!(calls.lock().expect("verrou").len(), 2);
}

#[test]
fn bypass_toggle_updates_the_settings() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().settings().params.bypass);
    h.get_by_label("Comparer : son brut").click();
    h.run_steps(2);
    assert!(h.state().settings().params.bypass);
}

#[test]
fn saving_a_user_preset() {
    let (mut h, _) = app(calibrated(), true, false);
    h.get_by_label("Enregistrer mes réglages").click();
    h.run_steps(2);
    let presets = &h.state().settings().user_presets;
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].0, "Mes réglages 1");
}

#[test]
fn installing_vb_cable_is_detected_without_restarting() {
    let (mut h, calls, shared_devices) = app_with_devices(Settings::default(), false, false);
    assert!(!h.state().is_running(), "sans VB-Cable, le son ne peut pas démarrer");
    h.get_by_label_contains("c'est lui qui crée le micro virtuel");
    h.state_mut().set_device_rescan(Duration::ZERO);

    // L'utilisateur installe VB-Cable pendant que le logiciel tourne.
    *shared_devices.lock().expect("verrou") = devices(true);
    h.run_steps(2);
    assert!(h.state().is_running(), "démarrage automatique dès que le câble apparaît");
    assert!(h.query_by_label_contains("c'est lui qui crée le micro virtuel").is_none());
    assert_eq!(
        calls.lock().expect("verrou").last().and_then(|c| c.1.clone()).as_deref(),
        Some("CABLE Input (VB-Audio Virtual Cable)")
    );
    assert!(h.state().calibration_open(), "premier lancement : l'assistant s'ouvre ensuite");
}

#[test]
fn a_manual_stop_is_not_undone_by_the_rescan() {
    let (mut h, calls) = app(calibrated(), true, false);
    h.state_mut().set_device_rescan(Duration::ZERO);
    h.get_by_label("Arrêter").click();
    h.run_steps(3);
    assert!(!h.state().is_running(), "l'arrêt volontaire est respecté");
    assert_eq!(calls.lock().expect("verrou").len(), 1);
}

#[test]
fn device_lists_are_labelled_before_the_list() {
    let (h, _) = app(calibrated(), true, false);
    // Les listes portent le nom de leur libellé (accessibilité) ; le libellé
    // « Micro : » est placé avant la liste, pas après.
    let micro = h.get_by_label("Micro :").rect();
    let sortie = h.get_by_label("Sortie (micro virtuel) :").rect();
    assert!(micro.left() < sortie.left());
}

/// Cas réel : après l'installation de VB-Cable, Windows prend *CABLE Output*
/// comme micro par défaut ; Micro Studio s'écoutait alors lui-même en boucle.
fn app_with_cable_as_default_mic(settings: Settings) -> (Harness<'static, App>, Calls) {
    let calls = Calls::default();
    let mut lists = devices(true);
    lists.inputs.insert(0, "CABLE Output (VB-Audio Virtual Cable)".to_owned());
    let backend = FakeBackend { devices: Arc::new(Mutex::new(lists)), fail: false, calls: Arc::clone(&calls) };
    let app = App::with_backend(settings, Box::new(backend), None);
    let mut harness =
        Harness::builder().with_size([900.0, 760.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run_steps(2);
    (harness, calls)
}

#[test]
fn the_cable_output_is_never_used_as_the_microphone() {
    let (h, calls) = app_with_cable_as_default_mic(calibrated());
    assert!(h.state().is_running());
    assert_eq!(h.state().settings().input_device.as_deref(), Some("Micro USB"));
    assert_eq!(calls.lock().expect("verrou").last().and_then(|c| c.0.clone()).as_deref(), Some("Micro USB"));

    // Même réglage enregistré par une version précédente : corrigé au démarrage.
    let saved = Settings { input_device: Some("CABLE Output (VB-Audio Virtual Cable)".to_owned()), ..calibrated() };
    let (h, _) = app_with_cable_as_default_mic(saved);
    assert_eq!(h.state().settings().input_device.as_deref(), Some("Micro USB"));
}

/// Cas réel (PC d'Adrien) : Voicemod est le micro par défaut de Windows.
fn app_with_voicemod(settings: Settings) -> (Harness<'static, App>, Calls) {
    let calls = Calls::default();
    let mut lists = devices(true);
    lists.inputs.insert(0, "Microphone (Voicemod Virtual Audio Device (WDM))".to_owned());
    let backend = FakeBackend { devices: Arc::new(Mutex::new(lists)), fail: false, calls: Arc::clone(&calls) };
    let app = App::with_backend(settings, Box::new(backend), None);
    let mut harness =
        Harness::builder().with_size([900.0, 760.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run_steps(2);
    (harness, calls)
}

#[test]
fn a_virtual_default_microphone_is_replaced_by_the_real_one() {
    let (h, _) = app_with_voicemod(calibrated());
    assert!(h.state().is_running());
    assert_eq!(h.state().settings().input_device.as_deref(), Some("Micro USB"));
}

#[test]
fn a_saved_virtual_microphone_is_kept_but_flagged() {
    let saved =
        Settings { input_device: Some("Microphone (Voicemod Virtual Audio Device (WDM))".to_owned()), ..calibrated() };
    let (mut h, _) = app_with_voicemod(saved);
    // Choix explicite : respecté, mais signalé avec un bouton de correction.
    assert_eq!(h.state().settings().input_device.as_deref(), Some("Microphone (Voicemod Virtual Audio Device (WDM))"));
    h.get_by_label_contains("est un micro virtuel");
    h.get_by_label("Utiliser « Micro USB »").click();
    h.run_steps(2);
    assert_eq!(h.state().settings().input_device.as_deref(), Some("Micro USB"));
    assert!(h.query_by_label_contains("est un micro virtuel").is_none());
}

#[test]
fn without_a_real_microphone_the_loop_is_refused() {
    let calls = Calls::default();
    let lists = DeviceLists {
        inputs: vec!["CABLE Output (VB-Audio Virtual Cable)".to_owned()],
        outputs: vec!["CABLE Input (VB-Audio Virtual Cable)".to_owned()],
    };
    let backend = FakeBackend { devices: Arc::new(Mutex::new(lists)), fail: false, calls };
    let app = App::with_backend(calibrated(), Box::new(backend), None);
    assert!(!app.is_running());
    assert!(app.error().is_some_and(|e| e.contains("Aucun vrai micro")));
}

type MonitorCalls = Arc<Mutex<Vec<(String, String)>>>;

/// Faux retour casque : note chaque démarrage (source, sortie).
struct FakeMonitor {
    default_output: Option<String>,
    calls: MonitorCalls,
}

impl MonitorBackend for FakeMonitor {
    fn default_output(&self) -> Option<String> {
        self.default_output.clone()
    }

    fn start(&self, _: &Arc<MonitorShared>, source: &str, sink: &str) -> anyhow::Result<Monitor> {
        self.calls.lock().expect("verrou de test").push((source.to_owned(), sink.to_owned()));
        Ok(Monitor::detached(source, sink))
    }
}

fn with_fake_monitor(h: &mut Harness<'static, App>, default_output: Option<&str>) -> MonitorCalls {
    let calls = MonitorCalls::default();
    h.state_mut().set_monitor_backend(Box::new(FakeMonitor {
        default_output: default_output.map(str::to_owned),
        calls: Arc::clone(&calls),
    }));
    calls
}

#[test]
fn listen_to_myself_plays_the_cable_in_the_headphones() {
    let (mut h, _) = app_with_cable_as_default_mic(calibrated());
    // Windows a pris le câble comme sortie par défaut : il ne doit pas servir au retour.
    let calls = with_fake_monitor(&mut h, Some("CABLE Input (VB-Audio Virtual Cable)"));
    // Libellé + contrôle qu'il nomme (lecteurs d'écran).
    assert!(h.query_all_by_label("Casque :").count() >= 1);
    assert!(h.query_all_by_label("Volume :").count() >= 2, "le curseur de volume porte un nom accessible");
    h.get_by_label("M'écouter dans le casque").click();
    h.run_steps(2);
    assert!(h.state().is_monitoring());
    assert_eq!(
        calls.lock().expect("verrou").last().cloned(),
        Some(("CABLE Output (VB-Audio Virtual Cable)".to_owned(), "Haut-parleurs".to_owned()))
    );

    h.get_by_label("M'écouter dans le casque").click();
    h.run_steps(2);
    assert!(!h.state().is_monitoring());
}

#[test]
fn listen_to_myself_without_the_cable_explains_why() {
    let (mut h, _) = app(calibrated(), true, false);
    let calls = with_fake_monitor(&mut h, Some("Haut-parleurs"));
    h.get_by_label("M'écouter dans le casque").click();
    h.run_steps(2);
    assert!(!h.state().is_monitoring());
    assert!(calls.lock().expect("verrou").is_empty());
    h.get_by_label_contains("CABLE Output) est introuvable");
}

#[test]
fn hum_filters_found_by_the_calibration_are_listed() {
    let (h, _) = app(calibrated(), true, false);
    h.get_by_label_contains("Aucun bourdonnement repéré.");
    let mut settings = calibrated();
    settings.params.notch_freqs_hz[0] = 50.0;
    settings.params.notch_freqs_hz[1] = 150.0;
    let (h, _) = app(settings, true, false);
    h.get_by_label_contains("Filtré : 50 Hz, 150 Hz");
}

#[test]
fn the_voice_gate_can_be_switched_on_by_hand() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().settings().params.voice_gate_enabled);
    h.get_by_label_contains("Couper tout ce qui n'est pas de la voix").click();
    h.run_steps(2);
    assert!(h.state().settings().params.voice_gate_enabled);
}

#[test]
fn the_multiband_compressor_can_be_switched_on_by_hand() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().settings().params.mb_enabled);
    // La section est plus bas dans la page, hors de l'écran : clic par
    // l'accessibilité (comme un lecteur d'écran), sans position à l'écran.
    h.get_by_label_contains("Compresseur multibande").click_accesskit();
    h.run_steps(2);
    assert!(h.state().settings().params.mb_enabled);
}

#[test]
fn the_dynamic_eq_can_be_switched_on_by_hand() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().settings().params.deq_enabled);
    h.get_by_label_contains("Égaliseur dynamique").click_accesskit();
    h.run_steps(2);
    assert!(h.state().settings().params.deq_enabled);
}

#[test]
fn the_dereverb_can_be_switched_on_by_hand() {
    let (mut h, _) = app(calibrated(), true, false);
    assert!(!h.state().settings().params.dereverb_enabled);
    h.get_by_label_contains("Anti-réverbération").click_accesskit();
    h.run_steps(2);
    assert!(h.state().settings().params.dereverb_enabled);
}

#[test]
fn blind_listening_keeps_the_chosen_finalist() {
    use micro_studio::calibrate::{Analysis, CalibrationResult, Score};
    let (mut h, _) = app(calibrated(), true, false);
    let finalists: Vec<(Params, Score)> = [-4.0, -8.0, -12.0]
        .iter()
        .map(|&g| (Params { gate_threshold_db: g * 5.0, ..Params::default() }, Score::default()))
        .collect();
    let r = CalibrationResult {
        analysis: Analysis::default(),
        params: finalists[0].0.clone(),
        score: Score::default(),
        baseline_score: Score::default(),
        candidates_tested: 3,
        finalists: finalists.clone(),
    };
    h.state_mut().show_calibration_result(r);
    h.run_steps(2);
    // Le meilleur est appliqué d'emblée, sous une lettre inconnue.
    assert_eq!(h.state().settings().params, finalists[0].0);
    h.get_by_label("Réglage B").click();
    h.run_steps(2);
    let listened = h.state().settings().params.clone();
    assert!(finalists.iter().any(|f| f.0 == listened));
    h.get_by_label("Anciens réglages").click();
    h.run_steps(2);
    assert_eq!(h.state().settings().params, Params::default());
    h.get_by_label("Garder le réglage B").click();
    h.run_steps(2);
    assert_eq!(h.state().settings().params, listened);
    assert!(!h.state().calibration_open());
}
