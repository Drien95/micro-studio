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
// De-esser à bande dynamique : seule la bande des sifflantes est atténuée.
//
// On retire au signal une part (1 - gain) de sa bande des sifflantes, isolée par
// un passe-bande RBJ dont la phase est nulle au centre. La réduction y est donc
// exacte. L'ancienne version retranchait un passe-haut, dont le déphasage
// (~67° à la fréquence visée) limitait la réduction réelle à ~0,7 dB au lieu des
// 6 dB demandés.

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
        self.splitter.set_bandpass(freq_hz, 0.8);
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
        let band = self.splitter.process(x);
        x - band * (1.0 - gain)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mut f: impl FnMut(f32) -> f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| f(amp * (i as f32 * 0.05).sin())).collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn gate_opens_above_threshold_and_closes_after_hold() {
        let mut g = Gate::default();
        g.configure(-30.0, 1.0, 50.0, 20.0, -60.0);
        assert!(!g.is_open());
        run(|x| g.process(x), 0.5, 4800);
        assert!(g.is_open());
        assert!(g.gain() > 0.99);
        // Silence : la porte reste ouverte pendant le maintien (50 ms = 2400 échantillons)…
        run(|x| g.process(x), 0.0, 2000);
        assert!(g.is_open(), "toujours ouverte pendant le maintien");
        // … puis se ferme et descend vers -60 dB (décroissance exponentielle).
        run(|x| g.process(x), 0.0, 9600);
        assert!(!g.is_open());
        assert!(lin_to_db(g.gain()) < -40.0, "{} dB", lin_to_db(g.gain()));
        run(|x| g.process(x), 0.0, 48_000);
        assert!((lin_to_db(g.gain()) + 60.0).abs() < 1.0);
    }

    #[test]
    fn gate_hysteresis_avoids_chatter() {
        let mut g = Gate::default();
        g.configure(-30.0, 1.0, 0.0, 20.0, -40.0);
        run(|x| g.process(x), db_to_lin(-28.0), 4800);
        assert!(g.is_open());
        // 2 dB sous le seuil d'ouverture, mais au-dessus du seuil de fermeture (-34 dB).
        run(|x| g.process(x), db_to_lin(-29.0) * 1.2, 9600);
        assert!(g.is_open(), "l'hystérésis de 4 dB garde la porte ouverte");
    }

    #[test]
    fn compressor_static_curve() {
        let mut c = Compressor::default();
        c.configure(-20.0, 4.0, 5.0, 100.0, 0.0, 0.0);
        assert!(c.static_gain_db(-40.0).abs() < f32::EPSILON, "sous le seuil : aucun effet");
        assert!((c.static_gain_db(-10.0) + 7.5).abs() < 1e-4, "10 dB au-dessus, ratio 4 : -7,5 dB");
        c.configure(-20.0, 4.0, 5.0, 100.0, 6.0, 0.0);
        // Genou progressif : continu aux bornes et doux au milieu.
        assert!(c.static_gain_db(-23.0).abs() < 1e-4);
        assert!((c.static_gain_db(-17.0) + 2.25).abs() < 1e-3);
        assert!(c.static_gain_db(-20.0) < 0.0 && c.static_gain_db(-20.0) > -2.25);
    }

    #[test]
    fn compressor_clamps_ratio_and_applies_makeup() {
        let mut c = Compressor::default();
        c.configure(-20.0, 0.2, 5.0, 100.0, 0.0, 6.0);
        assert!(c.static_gain_db(0.0).abs() < f32::EPSILON, "ratio < 1 ramené à 1");
        let out = run(|x| c.process(x), 0.01, 4800);
        assert!((lin_to_db(peak(&out[2400..])) - (lin_to_db(0.01) + 6.0)).abs() < 0.2);
        assert!(c.gain_reduction_db().abs() < 1e-3);
    }

    #[test]
    fn deesser_only_touches_high_frequencies() {
        let tone = |freq: f32, amp: f32, n: usize| -> Vec<f32> {
            (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE).sin()).collect()
        };
        let mut d = DeEsser::default();
        d.configure(6500.0, -30.0, 6.0);
        let low: Vec<f32> = tone(300.0, 0.5, 9600).into_iter().map(|x| d.process(x)).collect();
        assert!((peak(&low[4800..]) - 0.5).abs() < 0.02, "les graves passent intacts");

        let mut d = DeEsser::default();
        d.configure(6500.0, -30.0, 6.0);
        let high: Vec<f32> = tone(6500.0, 0.5, 9600).into_iter().map(|x| d.process(x)).collect();
        let reduction = lin_to_db(peak(&high[4800..]) / 0.5);
        assert!((reduction + 6.0).abs() < 0.5, "sifflante atténuée de {reduction:.1} dB (6 dB demandés)");
        assert!(d.gain_reduction_db() < -3.0);
    }

    #[test]
    fn limiter_delays_by_lookahead_and_respects_ceiling() {
        let mut l = Limiter::default();
        l.configure(-6.0);
        let mut out = Vec::new();
        out.push(l.process(0.1));
        for _ in 0..LIMITER_LOOKAHEAD {
            out.push(l.process(0.0));
        }
        assert!((out[LIMITER_LOOKAHEAD] - 0.1).abs() < 1e-6, "retard de {LIMITER_LOOKAHEAD} échantillons");
        let loud = run(|x| l.process(x), 2.0, 9600);
        assert!(peak(&loud) <= db_to_lin(-6.0) + 1e-6);
    }

    #[test]
    fn limiter_ceiling_cannot_exceed_full_scale() {
        let mut l = Limiter::default();
        l.configure(6.0);
        let out = run(|x| l.process(x), 3.0, 4800);
        assert!(peak(&out) <= 1.0 + 1e-6);
    }
}
