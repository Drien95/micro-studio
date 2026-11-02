//! Noise gate, compresseur, de-esser et limiteur.

use super::biquad::Biquad;
use super::util::{SAMPLE_RATE, db_to_lin, lin_to_db, time_coef};

// ---------------------------------------------------------------------------
// Noise gate avec hystérésis et temps de maintien.

#[derive(Debug, Default)]
pub struct Gate {
    env: f32,
    gain: f32,
    hold_left: usize,
    open: bool,
    threshold: f32,
    close_threshold: f32,
    floor: f32,
    hold_samples: usize,
    attack: f32,
    release: f32,
    env_release: f32,
}

impl Gate {
    pub fn configure(&mut self, threshold_db: f32, attack_ms: f32, hold_ms: f32, release_ms: f32, range_db: f32) {
        self.threshold = db_to_lin(threshold_db);
        // La porte se referme 4 dB plus bas qu'elle ne s'ouvre : évite le « bégaiement ».
        self.close_threshold = db_to_lin(threshold_db - 4.0);
        self.floor = db_to_lin(range_db.min(0.0));
        self.hold_samples = (hold_ms.max(0.0) * 0.001 * SAMPLE_RATE) as usize;
        self.attack = time_coef(attack_ms.max(0.1));
        self.release = time_coef(release_ms.max(1.0));
        self.env_release = time_coef(20.0);
        if self.gain == 0.0 {
            self.gain = self.floor;
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let a = x.abs();
        self.env = if a > self.env { a } else { self.env * self.env_release + a * (1.0 - self.env_release) };

        if self.env >= self.threshold {
            self.open = true;
            self.hold_left = self.hold_samples;
        } else if self.env < self.close_threshold {
            if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }
        }

        let target = if self.open { 1.0 } else { self.floor };
        let coef = if target > self.gain { self.attack } else { self.release };
        self.gain = target + (self.gain - target) * coef;
        x * self.gain
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

// ---------------------------------------------------------------------------
// Compresseur feed-forward à genou progressif.

#[derive(Debug, Default)]
pub struct Compressor {
    env_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    makeup: f32,
    attack: f32,
    release: f32,
    last_gr_db: f32,
}

impl Compressor {
    pub fn configure(
        &mut self,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        knee_db: f32,
        makeup_db: f32,
    ) {
        self.threshold_db = threshold_db;
        self.ratio = ratio.max(1.0);
        self.knee_db = knee_db.max(0.0);
        self.makeup = db_to_lin(makeup_db);
        self.attack = time_coef(attack_ms.max(0.1));
        self.release = time_coef(release_ms.max(1.0));
        if self.env_db == 0.0 {
            self.env_db = -120.0;
        }
    }

    /// Réduction de gain statique (dB, ≤ 0) pour un niveau d'entrée donné.
    pub fn static_gain_db(&self, level_db: f32) -> f32 {
        let over = level_db - self.threshold_db;
        let slope = 1.0 / self.ratio - 1.0;
        if self.knee_db > 0.0 && over.abs() <= self.knee_db / 2.0 {
            slope * (over + self.knee_db / 2.0).powi(2) / (2.0 * self.knee_db)
        } else if over > 0.0 {
            slope * over
        } else {
            0.0
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let level_db = lin_to_db(x.abs());
        let coef = if level_db > self.env_db { self.attack } else { self.release };
        self.env_db = level_db + (self.env_db - level_db) * coef;
        let gr = self.static_gain_db(self.env_db);
        self.last_gr_db = gr;
        x * db_to_lin(gr) * self.makeup
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr_db
    }
}

// ---------------------------------------------------------------------------
// De-esser à bande séparée : seuls les aigus au-dessus de la fréquence sont atténués.

#[derive(Debug, Default)]
pub struct DeEsser {
    detector: Biquad,
    splitter: Biquad,
    env: f32,
    threshold: f32,
    max_reduction: f32,
    attack: f32,
    release: f32,
    last_gr_db: f32,
}

impl DeEsser {
    pub fn configure(&mut self, freq_hz: f32, threshold_db: f32, max_reduction_db: f32) {
        self.detector.set_bandpass(freq_hz, 1.4);
        self.splitter.set_highpass(freq_hz * 0.75, 0.707);
        self.threshold = db_to_lin(threshold_db);
        self.max_reduction = db_to_lin(-max_reduction_db.abs());
        self.attack = time_coef(1.0);
        self.release = time_coef(60.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let s = self.detector.process(x).abs();
        let coef = if s > self.env { self.attack } else { self.release };
        self.env = s + (self.env - s) * coef;

        let gain = if self.env > self.threshold { (self.threshold / self.env).max(self.max_reduction) } else { 1.0 };
        self.last_gr_db = lin_to_db(gain);
        let high = self.splitter.process(x);
        // x = graves + aigus ; on n'atténue que la part aiguë.
        x - high * (1.0 - gain)
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr_db
    }
}

// ---------------------------------------------------------------------------
// Limiteur avec anticipation de 1,5 ms : aucun échantillon ne dépasse le plafond.

#[derive(Debug)]
pub struct Limiter {
    ceiling: f32,
    delay: Vec<f32>,
    pos: usize,
    gain: f32,
    release: f32,
    attack: f32,
}

pub const LIMITER_LOOKAHEAD: usize = 72; // 1,5 ms à 48 kHz

impl Default for Limiter {
    fn default() -> Self {
        Self {
            ceiling: db_to_lin(-1.0),
            delay: vec![0.0; LIMITER_LOOKAHEAD],
            pos: 0,
            gain: 1.0,
            release: time_coef(80.0),
            attack: time_coef(0.5),
        }
    }
}

impl Limiter {
    pub fn configure(&mut self, ceiling_db: f32) {
        self.ceiling = db_to_lin(ceiling_db.min(0.0));
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let delayed = self.delay[self.pos];
        self.delay[self.pos] = x;
        self.pos = (self.pos + 1) % self.delay.len();

        let peak = x.abs();
        let target = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
        let coef = if target < self.gain { self.attack } else { self.release };
        self.gain = target + (self.gain - target) * coef;
        (delayed * self.gain).clamp(-self.ceiling, self.ceiling)
    }
}
