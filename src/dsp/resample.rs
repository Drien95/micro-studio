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
