//! Limiteur « true peak » (UIT-R BS.1770-4, annexe 2) : aucune crête, même
//! entre deux échantillons, ne dépasse le plafond (en dBTP).
//!
//! Un limiteur ordinaire ne regarde que les échantillons. Or le signal
//! analogique reconstruit (et surtout le décodage Opus de Discord) peut
//! monter plus haut entre deux échantillons : jusqu'à 3 dB pour une
//! sinusoïde à fs/4 déphasée. On mesure donc la crête sur le signal
//! suréchantillonné 4 fois (filtre d'interpolation à 48 coefficients, comme
//! la norme), et le gain est calculé pour que cette crête-là reste sous le
//! plafond. EBU R128 et l'AES (TD1004) recommandent -1 dBTP ; -2 dBTP laisse
//! de la marge à un codec avec pertes comme Opus.
//!
//! Gain : minimum glissant du gain requis sur la fenêtre d'anticipation, puis
//! moyenne glissante sur la même durée. La moyenne d'une suite qui reste sous
//! le gain requis pendant toute la fenêtre est elle aussi en dessous : le gain
//! atteint sa valeur à temps, sans saut brusque (pas de distorsion).
//! Relâchement exponentiel ensuite.

use super::util::{SAMPLE_RATE, db_to_lin, time_coef};

/// Facteur de suréchantillonnage de la mesure de crête (BS.1770 : 4).
const OVERSAMPLING: usize = 4;
/// Coefficients par phase du filtre d'interpolation (48 au total).
const TAPS_PER_PHASE: usize = 12;
/// Retard du détecteur : centre du filtre, arrondi à l'échantillon.
const DETECTOR_DELAY: usize = TAPS_PER_PHASE / 2;
/// Durée d'anticipation : 1,5 ms.
const WINDOW: usize = 72;
/// Le minimum glissant couvre un peu plus que la moyenne : les crêtes situées
/// entre deux échantillons dépendent des gains des deux voisins.
const HOLD: usize = WINDOW + 2;
/// Retard total ajouté par le limiteur (échantillons).
pub const LATENCY: usize = WINDOW - 1 + DETECTOR_DELAY;
/// Marge du détecteur : -0,2 dB.
const DETECTOR_MARGIN: f32 = 0.977_237;
/// Relâchement du gain (ms).
const RELEASE_MS: f32 = 80.0;

/// Filtre d'interpolation : sinus cardinal fenêtré (Kaiser β = 6), coupure
/// à 0,45 × fréquence d'échantillonnage, chaque phase normalisée à 1.
fn interpolation_filter() -> [[f32; TAPS_PER_PHASE]; OVERSAMPLING] {
    let n = OVERSAMPLING * TAPS_PER_PHASE;
    let center = (n - 1) as f64 / 2.0;
    let fc = 0.45 / OVERSAMPLING as f64;
    let beta = 6.0;
    let i0 = |x: f64| {
        let (mut sum, mut term) = (1.0, 1.0);
        for k in 1..40 {
            term *= (x / (2.0 * f64::from(k))).powi(2);
            sum += term;
        }
        sum
    };
    let h: Vec<f64> = (0..n)
        .map(|j| {
            let t = j as f64 - center;
            let sinc = if t == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t)
            };
            let r = t / center;
            sinc * i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0(beta)
        })
        .collect();
    let mut phases = [[0f32; TAPS_PER_PHASE]; OVERSAMPLING];
    for (q, phase) in phases.iter_mut().enumerate() {
        let sum: f64 = (0..TAPS_PER_PHASE).map(|k| h[OVERSAMPLING * k + q]).sum();
        for (k, c) in phase.iter_mut().enumerate() {
            *c = (h[OVERSAMPLING * k + q] / sum) as f32;
        }
    }
    phases
}

#[derive(Debug)]
pub struct Limiter {
    ceiling: f32,
    /// Plafond visé par le détecteur (avec marge).
    target: f32,
    filter: [[f32; TAPS_PER_PHASE]; OVERSAMPLING],
    /// Dernières entrées pour l'interpolation (anneau, doublé pour lire sans modulo).
    hist: [f32; 2 * TAPS_PER_PHASE],
    hist_pos: usize,
    /// Ligne à retard du signal (anneau).
    delay: [f32; LATENCY],
    delay_pos: usize,
    /// Gains requis des `HOLD` dernières mesures (anneau).
    required: [f32; HOLD],
    req_pos: usize,
    /// Enveloppe après relâchement, et ses `WINDOW` dernières valeurs (anneau).
    env: f32,
    smooth: [f32; WINDOW],
    smooth_pos: usize,
    smooth_sum: f64,
    release: f32,
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            ceiling: db_to_lin(-1.0),
            target: db_to_lin(-1.0) * DETECTOR_MARGIN,
            filter: interpolation_filter(),
            hist: [0.0; 2 * TAPS_PER_PHASE],
            hist_pos: 0,
            delay: [0.0; LATENCY],
            delay_pos: 0,
            required: [1.0; HOLD],
            req_pos: 0,
            env: 1.0,
            smooth: [1.0; WINDOW],
            smooth_pos: 0,
            smooth_sum: WINDOW as f64,
            release: time_coef(RELEASE_MS),
            gain: 1.0,
        }
    }
}

impl Limiter {
    /// Plafond en dBTP (au plus 0).
    pub fn configure(&mut self, ceiling_db: f32) {
        let db = if ceiling_db.is_finite() { ceiling_db.min(0.0) } else { -1.0 };
        self.ceiling = db_to_lin(db);
        // Les 4 points interpolés tombent à 1/8, 3/8, 5/8 et 7/8 d'échantillon :
        // une crête exactement entre deux peut être lue jusqu'à 0,2 dB trop
        // bas à fs/4 (cos(π/16)). Le détecteur vise donc 0,2 dB plus bas.
        self.target = self.ceiling * DETECTOR_MARGIN;
    }

    /// Crête « vraie » autour de l'échantillon d'il y a `DETECTOR_DELAY` :
    /// l'échantillon lui-même et les 4 points interpolés voisins.
    fn true_peak(&self) -> f32 {
        // Entrées de la plus récente à la plus ancienne.
        let recent = &self.hist[self.hist_pos..self.hist_pos + TAPS_PER_PHASE];
        let mut peak = recent[TAPS_PER_PHASE - 1 - DETECTOR_DELAY].abs();
        for phase in &self.filter {
            let y: f32 = phase.iter().zip(recent.iter().rev()).map(|(c, x)| c * x).sum();
            peak = peak.max(y.abs());
        }
        peak
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        // Historique doublé : la tranche [pos, pos + TAPS) est toujours contiguë,
        // de la plus ancienne (pos) à la plus récente (pos + TAPS - 1).
        self.hist[self.hist_pos] = x;
        self.hist[self.hist_pos + TAPS_PER_PHASE] = x;
        self.hist_pos = (self.hist_pos + 1) % TAPS_PER_PHASE;

        let tp = self.true_peak();
        let need = if tp > self.target { self.target / tp } else { 1.0 };
        self.required[self.req_pos] = need;
        self.req_pos = (self.req_pos + 1) % HOLD;
        let held = self.required.iter().copied().fold(1.0f32, f32::min);

        // Relâchement : l'enveloppe remonte lentement, mais jamais au-dessus
        // du minimum requis.
        self.env = held.min(self.env + (1.0 - self.env) * (1.0 - self.release));
        self.smooth_sum += f64::from(self.env) - f64::from(self.smooth[self.smooth_pos]);
        self.smooth[self.smooth_pos] = self.env;
        self.smooth_pos = (self.smooth_pos + 1) % WINDOW;
        self.gain = (self.smooth_sum / WINDOW as f64) as f32;

        let delayed = self.delay[self.delay_pos];
        self.delay[self.delay_pos] = x;
        self.delay_pos = (self.delay_pos + 1) % self.delay.len();
        (delayed * self.gain).clamp(-self.ceiling, self.ceiling)
    }

    /// Réduction de gain actuelle (dB, ≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        super::util::lin_to_db(self.gain)
    }
}

/// Crête vraie (dBTP) d'un signal, mesurée avec un suréchantillonnage de 8
/// et un long filtre : sert de référence aux tests. Les 64 premiers et
/// derniers échantillons ne servent que de contexte (un signal coupé net
/// donnerait sinon de faux dépassements, effet de Gibbs).
pub fn true_peak_dbtp(x: &[f32]) -> f32 {
    const UP: usize = 8;
    const HALF: isize = 64;
    let edge = (HALF as usize).min(x.len() / 2);
    let inner = &x[edge..x.len() - edge];
    let mut peak = inner.iter().fold(0f32, |m, s| m.max(s.abs()));
    for n in edge..x.len() - edge {
        for q in 1..UP {
            let t = n as f64 + q as f64 / UP as f64;
            let mut y = 0f64;
            for k in -HALF..=HALF {
                let i = n.cast_signed() + k;
                if i < 0 || i as usize >= x.len() {
                    continue;
                }
                let d = t - i as f64;
                let w = 0.5 + 0.5 * (std::f64::consts::PI * d / (HALF as f64 + 1.0)).cos();
                let sinc = (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d);
                y += f64::from(x[i as usize]) * sinc * w;
            }
            peak = peak.max(y.abs() as f32);
        }
    }
    super::util::lin_to_db(peak)
}

/// Retard du limiteur en millisecondes (affichage).
pub const LATENCY_MS: f32 = LATENCY as f32 * 1000.0 / SAMPLE_RATE;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::lin_to_db;

    fn sine(freq: f32, phase: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE + phase).sin()).collect()
    }

    #[test]
    fn inter_sample_peaks_are_caught() {
        // Sinusoïde à fs/4 déphasée de 45° : les échantillons valent ±0,707 de
        // la crête réelle. Échantillons à -1 dBFS, crête réelle à +2 dBTP.
        let amp = db_to_lin(-1.0) * std::f32::consts::SQRT_2;
        let x = sine(12_000.0, std::f32::consts::FRAC_PI_4, amp, 4_800);
        assert!(true_peak_dbtp(&x) > 1.5, "signal d'essai : {} dBTP", true_peak_dbtp(&x));
        let mut l = Limiter::default();
        l.configure(-1.0);
        let y: Vec<f32> = x.iter().map(|&s| l.process(s)).collect();
        let tp = true_peak_dbtp(&y[LATENCY + 200..]);
        assert!(tp <= -1.0 + 0.1, "sortie : {tp:.2} dBTP");
    }

    #[test]
    fn transients_never_exceed_the_ceiling() {
        // Silence puis coup brutal (voix qui crie) : l'anticipation suffit.
        let mut x = vec![0.0; 2_000];
        x.extend(sine(1_000.0, 0.0, 1.5, 4_000));
        x.extend(sine(7_000.0, 0.3, 0.9, 4_000));
        let mut l = Limiter::default();
        l.configure(-2.0);
        let y: Vec<f32> = x.iter().map(|&s| l.process(s)).collect();
        assert!(true_peak_dbtp(&y) <= -2.0 + 0.15, "{} dBTP", true_peak_dbtp(&y));
        // Fin : sinusoïde à 0,9 (-0,9 dBFS) sous un plafond de -2 dBTP.
        assert!(l.gain_reduction_db() < -1.0, "{}", l.gain_reduction_db());
    }

    #[test]
    fn quiet_signals_pass_unchanged_after_the_delay() {
        let x = sine(440.0, 0.0, 0.3, 4_800);
        let mut l = Limiter::default();
        let y: Vec<f32> = x.iter().map(|&s| l.process(s)).collect();
        let err = x.iter().zip(&y[LATENCY..]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err < 1e-6, "{err}");
        assert!(l.gain_reduction_db().abs() < 1e-3);
        assert!((LATENCY_MS - 1.604).abs() < 0.01);
    }

    #[test]
    fn invalid_values_are_safe() {
        let mut l = Limiter::default();
        l.configure(f32::NAN);
        assert!((lin_to_db(l.ceiling) + 1.0).abs() < 1e-4);
        l.configure(6.0);
        assert!((l.ceiling - 1.0).abs() < 1e-6);
        for s in [f32::NAN, f32::INFINITY, 3.0, -3.0] {
            assert!(l.process(s).abs() <= 1.0);
        }
    }

    #[test]
    fn reference_meter_reads_full_scale_sines() {
        let x = sine(997.0, 0.0, 1.0, 4_800);
        assert!(true_peak_dbtp(&x[..2_000]).abs() < 0.05);
        let amp = db_to_lin(-1.0) * std::f32::consts::SQRT_2;
        let x = sine(12_000.0, std::f32::consts::FRAC_PI_4, amp, 2_000);
        assert!((true_peak_dbtp(&x) - 2.01).abs() < 0.05, "{}", true_peak_dbtp(&x));
    }
}
