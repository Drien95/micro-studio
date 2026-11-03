//! Anti-réverbération : retire la « queue » d'écho d'une pièce vide ou
//! carrelée, qui donne une voix lointaine, « dans une salle de bain ».
//!
//! Méthode de Lebart, Boucher et Denbigh (« A new method based on spectral
//! subtraction for speech dereverberation », Acta Acustica, 2001) : dans
//! chaque bande de fréquence, la réverbération tardive est la puissance
//! d'il y a `Td` (50 ms) qui s'est éteinte depuis selon la durée de
//! réverbération de la pièce (T60) :
//!
//! `R(t) = e^(-2·Δ·Td) · P(t - Td)`, avec `Δ = 3·ln(10) / T60`.
//!
//! On la retranche par un gain spectral (soustraction de Wiener), borné par
//! un plancher pour ne pas créer de « bruit musical ». Analyse et synthèse
//! par transformée de Fourier courte (fenêtres de 20 ms en racine de Hann,
//! recouvrement de moitié, reconstruction parfaite). Retard : 10 ms.
//!
//! Aucune allocation après la construction : utilisable dans le callback.

use super::FRAME;
use super::util::{SAMPLE_RATE, db_to_lin};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Taille des fenêtres d'analyse (2 trames, 20 ms).
const WIN: usize = 2 * FRAME;
/// Raies utiles du spectre (0 à Nyquist).
const BINS: usize = WIN / 2 + 1;
/// Délai `Td` (en trames de 10 ms) au-delà duquel le son est de la
/// réverbération tardive : 50 ms, la limite admise de l'« effet de précédence ».
const DELAY_FRAMES: usize = 5;
/// Lissage temporel de la puissance (constante de temps ≈ 40 ms).
const PSD_SMOOTHING: f32 = 0.78;
/// Retard ajouté par l'analyse-synthèse.
pub const LATENCY: usize = FRAME;

pub struct Dereverb {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: [f32; WIN],
    input: [f32; WIN],
    overlap: [f32; FRAME],
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Puissance lissée des `DELAY_FRAMES + 1` dernières trames (anneau).
    psd: [[f32; BINS]; DELAY_FRAMES + 1],
    pos: usize,
    /// Facteur d'extinction `e^(-2·Δ·Td)` × intensité.
    decay: f32,
    floor: f32,
    gains: [f32; BINS],
}

impl std::fmt::Debug for Dereverb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dereverb").field("decay", &self.decay).field("floor", &self.floor).finish_non_exhaustive()
    }
}

impl Default for Dereverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Dereverb {
    /// Prépare les transformées (allocation : hors du callback audio).
    pub fn new() -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(WIN);
        let ifft = planner.plan_fft_inverse(WIN);
        let scratch_len = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        // Racine de Hann périodique : analyse × synthèse = Hann, dont les
        // copies décalées d'une demi-fenêtre s'additionnent à 1.
        let window = std::array::from_fn(|i| (std::f32::consts::PI * i as f32 / WIN as f32).sin());
        let mut d = Self {
            fft,
            ifft,
            window,
            input: [0.0; WIN],
            overlap: [0.0; FRAME],
            spec: vec![Complex32::default(); WIN],
            scratch: vec![Complex32::default(); scratch_len],
            psd: [[0.0; BINS]; DELAY_FRAMES + 1],
            pos: 0,
            decay: 0.0,
            floor: 0.0,
            gains: [1.0; BINS],
        };
        d.configure(0.5, 0.7);
        d
    }

    /// `t60_s` : durée de réverbération de la pièce (s) ; `strength` (0..1) :
    /// part de la réverbération estimée qui est retirée.
    pub fn configure(&mut self, t60_s: f32, strength: f32) {
        let t60 = if t60_s.is_finite() { t60_s.clamp(0.1, 3.0) } else { 0.5 };
        let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.0 };
        let delta = 3.0 * std::f32::consts::LN_10 / t60;
        let td = (DELAY_FRAMES * FRAME) as f32 / SAMPLE_RATE;
        self.decay = (-2.0 * delta * td).exp() * strength;
        // Jusqu'à 15 dB d'atténuation à pleine intensité.
        self.floor = db_to_lin(-15.0 * strength);
    }

    /// Traite une trame en place (sortie retardée de [`LATENCY`]).
    pub fn process(&mut self, frame: &mut [f32; FRAME]) {
        self.input.copy_within(FRAME.., 0);
        self.input[FRAME..].copy_from_slice(frame);
        for ((s, &x), &w) in self.spec.iter_mut().zip(&self.input).zip(&self.window) {
            *s = Complex32::new(x * w, 0.0);
        }
        self.fft.process_with_scratch(&mut self.spec, &mut self.scratch);

        // Anneau : `pos` reçoit la trame courante ; la plus ancienne (Td plus
        // tôt) est la suivante.
        let prev = (self.pos + DELAY_FRAMES) % (DELAY_FRAMES + 1);
        let old = (self.pos + 1) % (DELAY_FRAMES + 1);
        for k in 0..BINS {
            let p = self.spec[k].norm_sqr();
            let smoothed = PSD_SMOOTHING * self.psd[prev][k] + (1.0 - PSD_SMOOTHING) * p;
            self.psd[self.pos][k] = smoothed;
            let reverb = self.decay * self.psd[old][k];
            let g = if smoothed > 1e-20 { (1.0 - reverb / smoothed).max(self.floor) } else { 1.0 };
            self.gains[k] = g;
        }
        self.pos = old;
        for k in 0..BINS {
            let g = self.gains[k];
            self.spec[k] *= g;
            if k > 0 && k < WIN - k {
                self.spec[WIN - k] *= g;
            }
        }

        self.ifft.process_with_scratch(&mut self.spec, &mut self.scratch);
        let norm = 1.0 / WIN as f32;
        for (i, out) in frame.iter_mut().enumerate() {
            *out = self.overlap[i] + self.spec[i].re * self.window[i] * norm;
            self.overlap[i] = self.spec[FRAME + i].re * self.window[FRAME + i] * norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(d: &mut Dereverb, x: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        for c in x.as_chunks::<FRAME>().0 {
            let mut f = *c;
            d.process(&mut f);
            out.extend_from_slice(&f);
        }
        out
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn strength_zero_reconstructs_the_input_with_ten_ms_delay() {
        let mut d = Dereverb::new();
        d.configure(0.5, 0.0);
        let x = noise(48_000, 3);
        let y = run(&mut d, &x);
        let err = x[..x.len() - LATENCY]
            .iter()
            .zip(&y[LATENCY..])
            .skip(FRAME)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(err < 1e-4, "écart {err}");
    }

    /// Salves de bruit de 100 ms suivies de leur queue de réverbération
    /// (décroissance exponentielle, T60 = 0,8 s) puis d'un silence.
    fn reverberant_bursts() -> (Vec<f32>, Vec<bool>) {
        let n = 48_000 * 4;
        let src = noise(n, 9);
        let tail = noise(n, 11);
        let period = 48_000;
        let burst = 4_800;
        let decay_per_sample = (-3.0 * std::f32::consts::LN_10 / (0.8 * SAMPLE_RATE)).exp();
        let mut x = vec![0.0; n];
        let mut is_tail = vec![false; n];
        for i in 0..n {
            let k = i % period;
            if k < burst {
                x[i] = 0.3 * src[i];
            } else {
                x[i] = 0.3 * tail[i] * decay_per_sample.powf((k - burst) as f32);
                is_tail[i] = k > burst + 4_800;
            }
        }
        (x, is_tail)
    }

    #[test]
    fn reverb_tail_is_reduced_and_direct_sound_kept() {
        let (x, is_tail) = reverberant_bursts();
        let mut d = Dereverb::new();
        d.configure(0.8, 1.0);
        let y = run(&mut d, &x);
        let energy = |sig: &[f32], tail: bool, shift: usize| -> f32 {
            let (mut e, mut n) = (0f64, 0usize);
            for i in 48_000..sig.len() - shift {
                let k = i % 48_000;
                let direct = k < 4_800;
                if (tail && is_tail[i]) || (!tail && direct) {
                    e += f64::from(sig[i + shift].powi(2));
                    n += 1;
                }
            }
            (10.0 * (e / n.max(1) as f64).log10()) as f32
        };
        let tail_cut = energy(&x, true, 0) - energy(&y, true, LATENCY);
        let direct_cut = energy(&x, false, 0) - energy(&y, false, LATENCY);
        assert!(tail_cut > 6.0, "queue atténuée de {tail_cut:.1} dB seulement");
        assert!(direct_cut < 2.0, "son direct atténué de {direct_cut:.1} dB");
    }

    #[test]
    fn invalid_settings_are_safe() {
        let mut d = Dereverb::new();
        d.configure(f32::NAN, f32::INFINITY);
        let mut f = [0.1; FRAME];
        d.process(&mut f);
        assert!(f.iter().all(|v| v.is_finite()));
        assert!(format!("{d:?}").contains("Dereverb"));
    }
}
