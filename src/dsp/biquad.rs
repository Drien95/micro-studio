//! Filtres biquad (formules RBJ « Audio EQ Cookbook »).

use super::util::SAMPLE_RATE;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    fn w0(freq: f32) -> (f32, f32) {
        let f = if freq.is_finite() { freq.clamp(10.0, SAMPLE_RATE * 0.49) } else { 1000.0 };
        let w0 = 2.0 * PI * f / SAMPLE_RATE;
        (w0.cos(), w0.sin())
    }

    /// `alpha` du cookbook RBJ ; Q borné pour éviter une division par zéro
    /// (réglage corrompu ou saisi à la main dans le fichier JSON).
    fn alpha(sin_w0: f32, q: f32) -> f32 {
        let q = if q.is_finite() { q.clamp(0.05, 50.0) } else { 0.707 };
        sin_w0 / (2.0 * q)
    }

    pub fn set_highpass(&mut self, freq: f32, q: f32) {
        let (c, s) = Self::w0(freq);
        let alpha = Self::alpha(s, q);
        self.set(f32::midpoint(1.0, c), -(1.0 + c), f32::midpoint(1.0, c), 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    pub fn set_lowpass(&mut self, freq: f32, q: f32) {
        let (c, s) = Self::w0(freq);
        let alpha = Self::alpha(s, q);
        self.set((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    /// Passe-bande à gain unitaire au sommet.
    pub fn set_bandpass(&mut self, freq: f32, q: f32) {
        let (c, s) = Self::w0(freq);
        let alpha = Self::alpha(s, q);
        self.set(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    /// Coupe-bande (« notch ») : supprime une fréquence précise, par exemple
    /// le bourdonnement du secteur à 50 Hz et ses harmoniques.
    pub fn set_notch(&mut self, freq: f32, q: f32) {
        let (c, s) = Self::w0(freq);
        let alpha = Self::alpha(s, q);
        self.set(1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    pub fn set_peak(&mut self, freq: f32, q: f32, gain_db: f32) {
        let (c, s) = Self::w0(freq);
        let a = 10f32.powf(gain_db / 40.0);
        let alpha = Self::alpha(s, q);
        self.set(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a);
    }

    pub fn set_low_shelf(&mut self, freq: f32, q: f32, gain_db: f32) {
        let (c, s) = Self::w0(freq);
        let a = 10f32.powf(gain_db / 40.0);
        let alpha = Self::alpha(s, q);
        let sa = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        );
    }

    pub fn set_high_shelf(&mut self, freq: f32, q: f32, gain_db: f32) {
        let (c, s) = Self::w0(freq);
        let a = 10f32.powf(gain_db / 40.0);
        let alpha = Self::alpha(s, q);
        let sa = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        );
    }

    /// Forme transposée directe II.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gain (dB) du filtre à `freq`, mesuré sur une sinusoïde en régime établi.
    fn gain_db(mut f: Biquad, freq: f32) -> f32 {
        let n = SAMPLE_RATE as usize / 2;
        let (mut e_in, mut e_out) = (0f32, 0f32);
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f32 / SAMPLE_RATE).sin();
            let y = f.process(x);
            if i > n / 2 {
                e_in += x * x;
                e_out += y * y;
            }
        }
        10.0 * (e_out / e_in).log10()
    }

    #[test]
    fn default_filter_is_transparent() {
        let mut f = Biquad::default();
        for x in [0.0, 0.5, -1.0, 0.25] {
            assert!((f.process(x) - x).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn notch_removes_only_its_frequency() {
        let mut f = Biquad::default();
        f.set_notch(150.0, 30.0);
        assert!(gain_db(f, 150.0) < -30.0, "{}", gain_db(f, 150.0));
        assert!(gain_db(f, 200.0).abs() < 0.5);
        assert!(gain_db(f, 1000.0).abs() < 0.1);
    }

    #[test]
    fn highpass_has_minus_3db_at_cutoff_with_butterworth_q() {
        let mut f = Biquad::default();
        f.set_highpass(100.0, std::f32::consts::FRAC_1_SQRT_2);
        assert!((gain_db(f, 100.0) + 3.0).abs() < 0.3);
        assert!(gain_db(f, 25.0) < -20.0, "deux octaves sous la coupure : -24 dB attendus");
        assert!(gain_db(f, 2000.0).abs() < 0.1);
    }

    #[test]
    fn bandpass_has_unity_gain_at_center() {
        let mut f = Biquad::default();
        f.set_bandpass(6000.0, 1.4);
        assert!(gain_db(f, 6000.0).abs() < 0.3);
        assert!(gain_db(f, 500.0) < -15.0);
    }

    #[test]
    fn peak_filter_reaches_its_gain_at_center() {
        for g in [-6.0, 3.0, 9.0] {
            let mut f = Biquad::default();
            f.set_peak(1000.0, 1.0, g);
            assert!((gain_db(f, 1000.0) - g).abs() < 0.2, "{g} dB");
            assert!(gain_db(f, 50.0).abs() < 0.3, "pas d'effet loin du centre");
        }
    }

    #[test]
    fn shelves_change_only_their_side() {
        let mut low = Biquad::default();
        low.set_low_shelf(200.0, 0.7, 6.0);
        assert!((gain_db(low, 30.0) - 6.0).abs() < 0.5);
        assert!(gain_db(low, 5000.0).abs() < 0.3);

        let mut high = Biquad::default();
        high.set_high_shelf(8000.0, 0.7, -4.0);
        assert!((gain_db(high, 20_000.0) + 4.0).abs() < 0.6);
        assert!(gain_db(high, 200.0).abs() < 0.3);
    }

    #[test]
    fn invalid_settings_still_give_a_stable_filter() {
        for (freq, q) in [(0.0, 0.0), (-50.0, -1.0), (f32::NAN, f32::NAN), (1e9, f32::INFINITY)] {
            let mut f = Biquad::default();
            f.set_peak(freq, q, 6.0);
            let g = gain_db(f, 1000.0);
            assert!(g.is_finite(), "freq {freq}, q {q}");
        }
    }
}
