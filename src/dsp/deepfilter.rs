//! Suppression de bruit « premium » DeepFilterNet3 (Schröter et al., 2023) :
//! réseau de neurones pleine bande à 48 kHz, plus efficace que RNNoise sur
//! les bruits brefs (clavier, souris) et dans les aigus.
//!
//! Le moteur est le nôtre ([`super::dfn`]) : sans allocation après le
//! chargement, il tourne directement dans le callback audio.

use super::FRAME;
use super::dfn::{DfnEngine, LATENCY};

/// Retard propre au modèle (recouvrement de la STFT + 2 trames d'anticipation).
pub const MODEL_LATENCY_MS: f32 = LATENCY as f32 * 1000.0 / 48_000.0;

/// Atténuation maximale (dB) au-delà de laquelle le modèle n'est plus bridé.
const UNLIMITED_DB: f32 = 100.0;
/// Atténuation maximale (dB) quand l'intensité vaut un peu moins de 1.
const LIMITED_MAX_DB: f32 = 60.0;

/// Convertit l'intensité de l'interface (0..1) en atténuation maximale (dB) :
/// 1 = sans limite, 0,5 = 30 dB, 0 = aucun effet.
pub fn strength_to_atten_lim_db(strength: f32) -> f32 {
    let s = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 };
    if s >= 0.999 { UNLIMITED_DB } else { s * LIMITED_MAX_DB }
}

/// Estime une probabilité de voix (0..1) à partir du rapport signal/bruit
/// local (dB) que calcule le modèle : -5 dB ou moins = bruit, 15 dB ou plus =
/// voix nette. Heuristique, utilisée par la porte vocale.
pub fn lsnr_to_voice_probability(lsnr_db: f32) -> f32 {
    if lsnr_db.is_finite() { ((lsnr_db + 5.0) / 20.0).clamp(0.0, 1.0) } else { 0.0 }
}

/// DeepFilterNet3 sur une voie, trame par trame, sans allocation.
#[derive(Debug)]
pub struct DeepFilter {
    engine: Box<DfnEngine>,
    input: [f32; FRAME],
    atten_lim_db: f32,
}

impl DeepFilter {
    /// Charge les poids embarqués (allocation : hors du callback audio).
    ///
    /// # Errors
    /// Si les poids embarqués sont illisibles.
    pub fn new() -> Result<Self, super::dfn::LoadError> {
        Ok(Self { engine: Box::new(DfnEngine::new()?), input: [0.0; FRAME], atten_lim_db: UNLIMITED_DB })
    }

    /// Règle l'intensité (0..1), voir [`strength_to_atten_lim_db`].
    pub fn set_strength(&mut self, strength: f32) {
        let db = strength_to_atten_lim_db(strength);
        if (db - self.atten_lim_db).abs() > f32::EPSILON {
            self.atten_lim_db = db;
            self.engine.set_atten_lim(db);
        }
    }

    /// Débruite une trame en place ; renvoie le rapport signal/bruit local
    /// estimé (dB).
    pub fn process(&mut self, frame: &mut [f32; FRAME]) -> f32 {
        for (d, s) in self.input.iter_mut().zip(frame.iter()) {
            *d = if s.is_finite() { *s } else { 0.0 };
        }
        self.engine.process(&self.input, frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::SAMPLE_RATE;

    fn rms_db(x: &[f32]) -> f32 {
        let e = x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32;
        10.0 * e.max(1e-12).log10()
    }

    #[test]
    fn strength_maps_to_attenuation_limit() {
        assert!((strength_to_atten_lim_db(1.0) - UNLIMITED_DB).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(0.5) - 30.0).abs() < 1e-4);
        assert!(strength_to_atten_lim_db(0.0).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(f32::NAN) - UNLIMITED_DB).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(7.0) - UNLIMITED_DB).abs() < f32::EPSILON);
        assert!((MODEL_LATENCY_MS - 30.0).abs() < 1e-3);
    }

    #[test]
    fn lsnr_maps_to_a_probability() {
        assert!(lsnr_to_voice_probability(-15.0).abs() < f32::EPSILON);
        assert!((lsnr_to_voice_probability(5.0) - 0.5).abs() < 1e-6);
        assert!((lsnr_to_voice_probability(35.0) - 1.0).abs() < f32::EPSILON);
        assert!(lsnr_to_voice_probability(f32::NAN).abs() < f32::EPSILON);
    }

    /// Bruit blanc déterministe.
    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 99u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn run(df: &mut DeepFilter, x: &[f32]) -> (Vec<f32>, f32) {
        let mut out = Vec::with_capacity(x.len());
        let mut last = 0.0;
        for c in x.as_chunks::<FRAME>().0 {
            let mut f = *c;
            last = df.process(&mut f);
            out.extend_from_slice(&f);
        }
        (out, last)
    }

    #[test]
    fn steady_noise_is_strongly_reduced() {
        let x = noise(SAMPLE_RATE as usize * 2, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        let (out, last) = run(&mut df, &x);
        let half = out.len() / 2;
        assert!(rms_db(&out[half..]) < rms_db(&x[half..]) - 20.0, "{} / {}", rms_db(&out[half..]), rms_db(&x));
        assert!(lsnr_to_voice_probability(last) < 0.3, "bruit seul : pas de voix ({last} dB)");
    }

    #[test]
    fn strength_zero_leaves_the_signal_untouched_and_nan_is_silenced() {
        let mut x = noise(SAMPLE_RATE as usize, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        df.set_strength(0.0);
        let (out, _) = run(&mut df, &x);
        let half = out.len() / 2;
        assert!((rms_db(&out[half..]) - rms_db(&x[half..])).abs() < 1.0);
        x[10] = f32::NAN;
        let (out, _) = run(&mut df, &x);
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
