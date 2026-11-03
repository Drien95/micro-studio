//! Maintenabilité et compatibilité : les fichiers de réglages doivent survivre
//! aux mises à jour du logiciel (champs ajoutés ou supprimés).

use micro_studio::params::{Params, builtin_presets};
use micro_studio::settings::Settings;

#[test]
fn params_round_trip_through_json() {
    for (_, p) in builtin_presets() {
        let json = serde_json::to_string(&p).expect("sérialisation");
        let back: Params = serde_json::from_str(&json).expect("désérialisation");
        assert_eq!(p, back);
    }
}

#[test]
fn old_settings_file_with_missing_fields_still_loads() {
    let json = r#"{ "params": { "input_gain_db": 6.0 }, "calibrated": true }"#;
    let s: Settings = serde_json::from_str(json).expect("ancien format accepté");
    assert!((s.params.input_gain_db - 6.0).abs() < f32::EPSILON);
    assert!((s.params.comp_ratio - Params::default().comp_ratio).abs() < f32::EPSILON);
    assert!(s.calibrated);
    assert_eq!(s.user_presets.len(), 0);
}

#[test]
fn unknown_fields_from_a_newer_version_are_ignored() {
    let json = r#"{ "params": { "futur_reglage": 1 }, "nouvelle_option": "x" }"#;
    let s: Settings = serde_json::from_str(json).expect("champs inconnus tolérés");
    assert_eq!(s.params, Params::default());
}

#[test]
fn builtin_presets_have_unique_names_and_sane_values() {
    let presets = builtin_presets();
    let mut names: Vec<_> = presets.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), presets.len(), "noms de préréglages en double");
    for (name, p) in &presets {
        assert!(p.limiter_ceiling_db <= 0.0, "{name}");
        assert!(p.comp_ratio >= 1.0, "{name}");
        assert!((0.0..=100.0).contains(&p.denoise_limit_db), "{name}");
        assert!(p.gate_range_db <= 0.0, "{name}");
    }
}
