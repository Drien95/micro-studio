//! Tous les réglages de la chaîne de traitement, sérialisables en JSON.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EqKind {
    LowShelf,
    Peak,
    HighShelf,
}

/// Moteur de suppression de bruit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenoiseEngine {
    /// RNNoise : léger, 10 ms de retard.
    #[default]
    RnNoise,
    /// DeepFilterNet3 : plus efficace (bruits brefs, aigus), 40 ms de retard.
    DeepFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub kind: EqKind,
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Nombre maximal de filtres coupe-bande (bourdonnement et ses harmoniques).
pub const MAX_NOTCHES: usize = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// Contourne toute la chaîne (comparaison avant/après).
    pub bypass: bool,

    pub input_gain_db: f32,

    pub hpf_enabled: bool,
    pub hpf_hz: f32,

    /// Filtres coupe-bande contre les bourdonnements (secteur, USB). Placés
    /// par le calibrage ; 0 Hz = filtre inutilisé.
    pub notch_enabled: bool,
    pub notch_freqs_hz: [f32; MAX_NOTCHES],
    /// Largeur de chaque coupe-bande (Hz) : étroite pour ne pas toucher la voix.
    pub notch_width_hz: f32,

    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    pub gate_attack_ms: f32,
    pub gate_hold_ms: f32,
    pub gate_release_ms: f32,
    /// Atténuation appliquée quand la porte est fermée (dB, négatif).
    pub gate_range_db: f32,

    pub denoise_enabled: bool,
    pub denoise_engine: DenoiseEngine,
    /// 0 = signal d'origine, 1 = débruitage complet.
    pub denoise_mix: f32,

    /// Porte vocale : coupe ce qui n'est pas de la voix d'après le réseau de
    /// neurones (touches de clavier, souris), même quand c'est fort.
    pub voice_gate_enabled: bool,
    /// Probabilité de voix (0..1) au-delà de laquelle la porte s'ouvre.
    pub voice_gate_threshold: f32,
    /// Durée de voix continue exigée avant d'ouvrir (ms) ; ajoute autant de
    /// retard (moins 10 ms) quand la porte vocale est active.
    pub voice_gate_confirm_ms: f32,
    pub voice_gate_hold_ms: f32,
    pub voice_gate_range_db: f32,

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
            notch_enabled: true,
            notch_freqs_hz: [0.0; MAX_NOTCHES],
            notch_width_hz: 4.0,
            gate_enabled: true,
            gate_threshold_db: -45.0,
            gate_attack_ms: 1.0,
            gate_hold_ms: 60.0,
            gate_release_ms: 150.0,
            gate_range_db: -40.0,
            denoise_enabled: true,
            denoise_engine: DenoiseEngine::RnNoise,
            denoise_mix: 1.0,
            voice_gate_enabled: false,
            voice_gate_threshold: 0.5,
            voice_gate_confirm_ms: 30.0,
            voice_gate_hold_ms: 300.0,
            voice_gate_range_db: -30.0,
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

    let stream = Params { gate_range_db: -60.0, comp_ratio: 3.5, comp_makeup_db: 5.0, ..Params::default() };

    let light = Params {
        deesser_enabled: false,
        denoise_mix: 0.7,
        eq_enabled: false,
        comp_ratio: 2.5,
        comp_makeup_db: 2.0,
        ..Params::default()
    };

    vec![("Voix naturelle", natural), ("Voix radio", radio), ("Stream / jeu", stream), ("Traitement léger", light)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_eq_is_ordered_by_frequency() {
        let eq = default_eq();
        assert!(eq.windows(2).all(|w| w[0].freq_hz < w[1].freq_hz));
        assert_eq!(eq[3].kind, EqKind::HighShelf);
    }

    #[test]
    fn presets_are_distinct() {
        let p = builtin_presets();
        assert_eq!(p.len(), 4);
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                assert_ne!(p[i].1, p[j].1, "« {} » et « {} » sont identiques", p[i].0, p[j].0);
            }
        }
        assert_eq!(p[0].1, Params::default(), "le premier préréglage est le réglage par défaut");
    }

    #[test]
    fn enum_names_are_stable_in_json() {
        // Renommer une variante casserait les fichiers de réglages existants.
        assert_eq!(serde_json::to_string(&EqKind::HighShelf).unwrap_or_default(), "\"HighShelf\"");
        assert_eq!(serde_json::to_string(&DenoiseEngine::DeepFilter).unwrap_or_default(), "\"DeepFilter\"");
        assert_eq!(serde_json::to_string(&DenoiseEngine::RnNoise).unwrap_or_default(), "\"RnNoise\"");
    }
}
