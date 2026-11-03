//! Égaliseur dynamique : des bandes qui ne baissent que quand il le faut.
//!
//! Un égaliseur classique corrige toujours, même quand la voix n'en a pas
//! besoin. Ici, chaque bande surveille son propre niveau et ne coupe que
//! l'excès au-dessus de son seuil : le son « carton » d'une voix qui force
//! dans les bas-médiums, ou la dureté d'un éclat de voix vers 3 kHz, sont
//! adoucis sans creuser la voix le reste du temps.
//!
//! Même principe que le de-esser : on retranche au signal une part de sa
//! bande, isolée par un passe-bande RBJ dont la phase est nulle au centre,
//! ce qui rend la réduction exacte à la fréquence visée.

use super::biquad::Biquad;
use super::util::{db_to_lin, lin_to_db, time_coef};

/// Nombre de bandes dynamiques.
pub const BANDS: usize = 2;

/// Réglages d'une bande dynamique.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DynamicBand {
    pub freq_hz: f32,
    /// Niveau de la bande (dBFS) au-delà duquel elle est réduite.
    pub threshold_db: f32,
    /// Taux de réduction de l'excès (2 = la moitié de l'excès est retirée).
    pub ratio: f32,
    /// Réduction maximale (dB, positive).
    pub max_cut_db: f32,
}

/// Bandes par défaut : bas-médiums « carton » et dureté des aigus.
pub fn default_bands() -> [DynamicBand; BANDS] {
    [
        DynamicBand { freq_hz: 400.0, threshold_db: -32.0, ratio: 3.0, max_cut_db: 6.0 },
        DynamicBand { freq_hz: 3_000.0, threshold_db: -34.0, ratio: 3.0, max_cut_db: 6.0 },
    ]
}

/// Largeur des bandes (Q du passe-bande).
const Q: f32 = 1.2;
const ATTACK_MS: f32 = 3.0;
const RELEASE_MS: f32 = 80.0;

#[derive(Debug, Default)]
struct Band {
    detector: Biquad,
    splitter: Biquad,
    env: f32,
    threshold_db: f32,
    slope: f32,
    max_cut_db: f32,
    attack: f32,
    release: f32,
    last_cut_db: f32,
}

impl Band {
    fn configure(&mut self, b: &DynamicBand) {
        let f = if b.freq_hz.is_finite() { b.freq_hz.clamp(80.0, 12_000.0) } else { 1_000.0 };
        self.detector.set_bandpass(f, Q);
        self.splitter.set_bandpass(f, Q);
        self.threshold_db = b.threshold_db;
        self.slope = 1.0 - 1.0 / b.ratio.max(1.0);
        self.max_cut_db = b.max_cut_db.abs().min(24.0);
        self.attack = time_coef(ATTACK_MS);
        self.release = time_coef(RELEASE_MS);
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let d = self.detector.process(x).abs();
        let coef = if d > self.env { self.attack } else { self.release };
        self.env = d + (self.env - d) * coef;
        // Niveau efficace d'une sinusoïde ≈ crête / √2 : le seuil se lit
        // comme un niveau en dBFS.
        let level_db = lin_to_db(self.env * std::f32::consts::FRAC_1_SQRT_2);
        let cut_db = ((level_db - self.threshold_db).max(0.0) * self.slope).min(self.max_cut_db);
        self.last_cut_db = -cut_db;
        let band = self.splitter.process(x);
        x - band * (1.0 - db_to_lin(-cut_db))
    }
}

#[derive(Debug, Default)]
pub struct DynamicEq {
    bands: [Band; BANDS],
}

impl DynamicEq {
    pub fn configure(&mut self, bands: &[DynamicBand; BANDS]) {
        for (b, s) in self.bands.iter_mut().zip(bands) {
            b.configure(s);
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.bands.iter_mut().fold(x, |s, b| b.process(s))
    }

    /// Réduction actuelle de chaque bande (dB, ≤ 0).
    pub fn cut_db(&self) -> [f32; BANDS] {
        [self.bands[0].last_cut_db, self.bands[1].last_cut_db]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::SAMPLE_RATE;

    /// Gain efficace (dB) en régime établi d'une sinusoïde.
    fn gain_at(eq: &mut DynamicEq, hz: f32, amp: f32) -> f32 {
        let n = SAMPLE_RATE as usize;
        let (mut e_in, mut e_out) = (0f64, 0f64);
        for i in 0..n {
            let x = amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SAMPLE_RATE).sin();
            let y = eq.process(x);
            if i >= n / 2 {
                e_in += f64::from(x * x);
                e_out += f64::from(y * y);
            }
        }
        (10.0 * (e_out / e_in).log10()) as f32
    }

    fn eq() -> DynamicEq {
        let mut e = DynamicEq::default();
        e.configure(&default_bands());
        e
    }

    #[test]
    fn quiet_signals_pass_untouched() {
        for hz in [400.0, 3_000.0, 1_000.0] {
            let g = gain_at(&mut eq(), hz, 0.005);
            assert!(g.abs() < 0.05, "{hz} Hz : {g:.2} dB");
        }
    }

    #[test]
    fn loud_band_is_reduced_up_to_the_limit() {
        // 400 Hz à -10 dBFS efficaces : 22 dB d'excès, réduction plafonnée à 6 dB.
        let mut e = eq();
        let g = gain_at(&mut e, 400.0, 0.447);
        assert!((g + 6.0).abs() < 0.5, "400 Hz fort : {g:.2} dB");
        assert!((e.cut_db()[0] + 6.0).abs() < 0.1);
        // Hors des bandes, rien ne bouge même fort.
        let g = gain_at(&mut eq(), 100.0, 0.447);
        assert!(g.abs() < 1.0, "100 Hz fort : {g:.2} dB");
    }

    #[test]
    fn reduction_follows_the_ratio() {
        // 4 dB au-dessus du seuil, ratio 3 : environ 2,7 dB de réduction.
        let mut e = eq();
        let amp = db_to_lin(-28.0) * std::f32::consts::SQRT_2;
        let g = gain_at(&mut e, 400.0, amp);
        assert!((-3.5..=-1.8).contains(&g), "{g:.2} dB");
    }
}
