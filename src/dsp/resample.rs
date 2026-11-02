//! Rééchantillonneur cubique (Catmull-Rom) en flux continu, pour passer de la
//! fréquence du périphérique à 48 kHz et inversement. Suffisant pour la voix.

#[derive(Debug, Clone)]
pub struct Resampler {
    step: f64,
    phase: f64,
    hist: [f32; 4],
}

impl Resampler {
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self { step: f64::from(from_hz) / f64::from(to_hz), phase: 0.0, hist: [0.0; 4] }
    }

    pub fn is_passthrough(&self) -> bool {
        (self.step - 1.0).abs() < 1e-9
    }

    #[inline]
    fn interp(&self) -> f32 {
        let t = self.phase as f32;
        let [y0, y1, y2, y3] = self.hist;
        let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c = -0.5 * y0 + 0.5 * y2;
        ((a * t + b) * t + c) * t + y1
    }

    #[inline]
    fn shift(&mut self, x: f32) {
        self.hist = [self.hist[1], self.hist[2], self.hist[3], x];
    }

    /// Mode « poussé » : un échantillon entre, zéro ou plusieurs sortent.
    pub fn push(&mut self, x: f32, mut out: impl FnMut(f32)) {
        if self.is_passthrough() {
            out(x);
            return;
        }
        self.shift(x);
        while self.phase < 1.0 {
            out(self.interp());
            self.phase += self.step;
        }
        self.phase -= 1.0;
    }

    /// Mode « tiré » : produit un échantillon en consommant ce qu'il faut de la source.
    pub fn pull(&mut self, mut src: impl FnMut() -> f32) -> f32 {
        if self.is_passthrough() {
            return src();
        }
        while self.phase >= 1.0 {
            self.shift(src());
            self.phase -= 1.0;
        }
        let y = self.interp();
        self.phase += self.step;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_passthrough() {
        let mut r = Resampler::new(48_000, 48_000);
        assert!(r.is_passthrough());
        let mut out = Vec::new();
        for x in [0.1, -0.2, 0.3] {
            r.push(x, |y| out.push(y));
        }
        assert_eq!(out, [0.1, -0.2, 0.3]);
        let mut src = [0.7f32, 0.8].into_iter();
        assert!((r.pull(|| src.next().unwrap_or(0.0)) - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn push_respects_the_rate_ratio() {
        for (from, to) in [(44_100, 48_000), (48_000, 44_100), (16_000, 48_000), (96_000, 48_000)] {
            let mut r = Resampler::new(from, to);
            let mut n = 0usize;
            for _ in 0..from {
                r.push(0.0, |_| n += 1);
            }
            assert!(n.abs_diff(to as usize) <= 2, "{from} → {to} : {n}");
        }
    }

    #[test]
    fn pull_consumes_at_the_rate_ratio() {
        let mut r = Resampler::new(48_000, 44_100);
        let mut consumed = 0usize;
        for _ in 0..44_100 {
            r.pull(|| {
                consumed += 1;
                0.0
            });
        }
        assert!(consumed.abs_diff(48_000) <= 4, "{consumed}");
    }

    #[test]
    fn constant_signal_stays_constant() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..1000 {
            r.push(0.5, |y| out.push(y));
        }
        assert!(out[10..].iter().all(|y| (y - 0.5).abs() < 1e-5));
    }

    #[test]
    fn sine_survives_resampling() {
        let (from, to, f) = (44_100.0f32, 48_000.0f32, 440.0f32);
        let mut r = Resampler::new(from as u32, to as u32);
        let mut out = Vec::new();
        for i in 0..44_100 {
            r.push((2.0 * std::f32::consts::PI * f * i as f32 / from).sin(), |y| out.push(y));
        }
        // Compare à la sinusoïde idéale à 48 kHz (avec le retard de 2 échantillons de l'interpolateur).
        let err = out[100..40_000]
            .iter()
            .enumerate()
            .map(|(i, y)| {
                let t = (i + 100) as f32 / to - 2.0 / from;
                (y - (2.0 * std::f32::consts::PI * f * t).sin()).abs()
            })
            .fold(0f32, f32::max);
        assert!(err < 0.01, "erreur max {err}");
    }
}
