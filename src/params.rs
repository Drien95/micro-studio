//! Tous les réglages de la chaîne de traitement, sérialisables en JSON.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EqKind {
    LowShelf,
    Peak,
    HighShelf,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub kind: EqKind,
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// Contourne toute la chaîne (comparaison avant/après).
    pub bypass: bool,

    pub input_gain_db: f32,

    pub hpf_enabled: bool,
    pub hpf_hz: f32,

    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    pub gate_attack_ms: f32,
    pub gate_hold_ms: f32,
    pub gate_release_ms: f32,
    /// Atténuation appliquée quand la porte est fermée (dB, négatif).
    pub gate_range_db: f32,

    pub denoise_enabled: bool,
    /// 0 = signal d'origine, 1 = débruitage complet.
    pub denoise_mix: f32,

    pub deesser_enabled: bool,
    pub deesser_freq_hz: f32,
    pub deesser_threshold_db: f32,
    pub deesser_max_reduction_db: f32,

    pub eq_enabled: bool,
    pub eq_bands: [EqBand; 4],

    pub comp_enabled: bool,
    pub comp_threshold_db: f32,
    pub comp_ratio: f32,
    pub comp_attack_ms: f32,
    pub comp_release_ms: f32,
    pub comp_knee_db: f32,
    pub comp_makeup_db: f32,

    pub limiter_enabled: bool,
    pub limiter_ceiling_db: f32,

    pub output_gain_db: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            bypass: false,
            input_gain_db: 0.0,
            hpf_enabled: true,
            hpf_hz: 90.0,
            gate_enabled: true,
            gate_threshold_db: -45.0,
            gate_attack_ms: 1.0,
            gate_hold_ms: 60.0,
            gate_release_ms: 150.0,
            gate_range_db: -40.0,
            denoise_enabled: true,
            denoise_mix: 1.0,
            deesser_enabled: true,
            deesser_freq_hz: 6500.0,
            deesser_threshold_db: -30.0,
            deesser_max_reduction_db: 6.0,
            eq_enabled: true,
            eq_bands: default_eq(),
            comp_enabled: true,
            comp_threshold_db: -20.0,
            comp_ratio: 4.0,
            comp_attack_ms: 5.0,
            comp_release_ms: 250.0,
            comp_knee_db: 6.0,
            comp_makeup_db: 4.0,
            limiter_enabled: true,
            limiter_ceiling_db: -1.0,
            output_gain_db: 0.0,
        }
    }
}

pub fn default_eq() -> [EqBand; 4] {
    [
        EqBand { kind: EqKind::Peak, freq_hz: 250.0, gain_db: 0.0, q: 1.0 },
        EqBand { kind: EqKind::Peak, freq_hz: 450.0, gain_db: -3.0, q: 1.0 },
        EqBand { kind: EqKind::Peak, freq_hz: 3200.0, gain_db: 3.0, q: 0.9 },
        EqBand { kind: EqKind::HighShelf, freq_hz: 10000.0, gain_db: 2.0, q: 0.7 },
    ]
}

/// Préréglages intégrés, pensés pour quelqu'un qui n'est pas ingé son.
pub fn builtin_presets() -> Vec<(&'static str, Params)> {
    let natural = Params::default();

    let mut radio = Params::default();
    radio.eq_bands[0].gain_db = 2.0;
    radio.eq_bands[1].gain_db = -4.0;
    radio.eq_bands[2].gain_db = 4.0;
    radio.eq_bands[3].gain_db = 3.0;
    radio.comp_ratio = 5.0;
    radio.comp_threshold_db = -24.0;
    radio.comp_makeup_db = 7.0;

    let mut stream = Params::default();
    stream.gate_range_db = -60.0;
    stream.comp_ratio = 3.5;
    stream.comp_makeup_db = 5.0;

    let mut light = Params::default();
    light.deesser_enabled = false;
    light.denoise_mix = 0.7;
    light.eq_enabled = false;
    light.comp_ratio = 2.5;
    light.comp_makeup_db = 2.0;

    vec![
        ("Voix naturelle", natural),
        ("Voix radio", radio),
        ("Stream / jeu", stream),
        ("Traitement léger", light),
    ]
}
