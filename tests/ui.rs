//! Tests de l'interface (niveau système, ISTQB) : l'application complète est
//! pilotée comme un utilisateur le ferait (clics, lecture des textes affichés),
//! avec un faux périphérique audio à la place de la carte son.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use micro_studio::engine::{AudioBackend, DeviceLists, Engine, Shared};
use micro_studio::settings::Settings;
use micro_studio::ui::App;
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(Option<String>, Option<String>)>>>;

/// Faux périphérique : enregistre chaque démarrage, peut simuler une panne.
struct FakeBackend {
    devices: DeviceLists,
    fail: bool,
    calls: Calls,
}

impl AudioBackend for FakeBackend {
    fn list_devices(&self) -> DeviceLists {
        self.devices.clone()
    }

    fn start(&self, _: &Arc<Shared>, input: Option<&str>, output: Option<&str>) -> anyhow::Result<Engine> {
        self.calls.lock().expect("verrou de test").push((input.map(Into::into), output.map(Into::into)));
        if self.fail {
            anyhow::bail!("Aucun micro détecté");
        }
        Ok(Engine::detached(input.unwrap_or("Micro USB"), output.unwrap_or("Sortie"), 48_000, 48_000))
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
    let calls = Calls::default();
    let backend = FakeBackend { devices: devices(with_cable), fail, calls: Arc::clone(&calls) };
    // `None` : l'application n'écrit jamais sur le disque pendant les tests.
    let app = App::with_backend(settings, Box::new(backend), None);
    let mut harness =
        Harness::builder().with_size([900.0, 760.0]).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run_steps(2);
    (harness, calls)
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
    assert!(h.query_by_label_contains("VB-Cable n'est pas installé").is_none());
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
fn missing_vb_cable_shows_a_download_link() {
    let (h, _) = app(calibrated(), false, false);
    h.get_by_label_contains("VB-Cable n'est pas installé");
    h.get_by_label_contains("Télécharger VB-Cable");
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
