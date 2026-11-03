//! Suppression de bruit « premium » DeepFilterNet3 (Schröter et al., 2023) :
//! réseau de neurones pleine bande à 48 kHz, plus efficace que RNNoise sur
//! les bruits brefs (clavier, souris) et dans les aigus.
//!
//! Le moteur est le nôtre ([`super::dfn`]) : sans allocation après le
//! chargement, il tourne directement dans le callback audio.

use super::FRAME;
use super::denoise::UNLIMITED_DB;
use super::dfn::{DfnEngine, LATENCY};

/// Retard propre au modèle (recouvrement de la STFT + 2 trames d'anticipation).
pub const MODEL_LATENCY_MS: f32 = LATENCY as f32 * 1000.0 / 48_000.0;

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

    /// Atténuation maximale du bruit (dB) ; [`super::denoise::UNLIMITED_DB`]
    /// ou plus : sans limite.
    pub fn set_atten_lim_db(&mut self, db: f32) {
        let db = if db.is_finite() { db.abs().min(UNLIMITED_DB) } else { UNLIMITED_DB };
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
    fn model_latency_is_30_ms() {
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
    fn limit_caps_the_reduction() {
        let x = noise(SAMPLE_RATE as usize * 2, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        df.set_atten_lim_db(12.0);
        let (out, _) = run(&mut df, &x);
        let half = out.len() / 2;
        let reduction = rms_db(&x[half..]) - rms_db(&out[half..]);
        assert!(reduction <= 12.5 && reduction > 6.0, "réduction {reduction:.1} dB pour 12 dB permis");
    }

    #[test]
    fn zero_limit_leaves_the_signal_untouched_and_nan_is_silenced() {
        let mut x = noise(SAMPLE_RATE as usize, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        df.set_atten_lim_db(0.0);
        let (out, _) = run(&mut df, &x);
        let half = out.len() / 2;
        assert!((rms_db(&out[half..]) - rms_db(&x[half..])).abs() < 1.0);
        x[10] = f32::NAN;
        let (out, _) = run(&mut df, &x);
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
