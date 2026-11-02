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

    /// Passe-bande à gain unitaire au sommet.
    pub fn set_bandpass(&mut self, freq: f32, q: f32) {
        let (c, s) = Self::w0(freq);
        let alpha = Self::alpha(s, q);
        self.set(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
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
