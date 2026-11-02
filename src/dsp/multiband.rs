//! Compresseur multibande à 3 bandes (graves, médiums, aigus).
//!
//! Un compresseur classique réagit au niveau global : un grave fort (voix
//! grave, « p » plosif) fait baisser aussi les aigus, et inversement. Ici,
//! chaque bande a son propre compresseur, ce qui tient la voix régulière
//! sans l'étouffer, comme les traitements de voix des studios radio.
//!
//! Filtres de séparation : Linkwitz-Riley d'ordre 4 (deux Butterworth
//! d'ordre 2 en série). Les sorties passe-bas et passe-haut sont en phase
//! et leur somme est un passe-tout : bandes réglées à 0 dB, le signal
//! ressort avec la même courbe d'amplitude (seule la phase tourne). La bande
//! grave passe en plus par un passe-tout à la seconde fréquence de
//! coupure, pour rester en phase avec les deux autres.

use super::biquad::Biquad;
use super::dynamics::Compressor;

/// Q d'un Butterworth d'ordre 2 (deux en série = Linkwitz-Riley d'ordre 4).
const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Attaque et relâchement (ms) par bande : plus lents dans les graves, dont
/// les périodes sont longues (sinon le compresseur déformerait l'onde).
const TIMES_MS: [(f32, f32); 3] = [(20.0, 200.0), (8.0, 120.0), (2.0, 60.0)];
/// Genou progressif (dB) commun aux trois bandes.
const KNEE_DB: f32 = 6.0;

/// Filtre Linkwitz-Riley d'ordre 4.
#[derive(Clone, Copy, Debug, Default)]
struct Lr4([Biquad; 2]);

impl Lr4 {
    fn lowpass(hz: f32) -> Self {
        let mut b = Biquad::default();
        b.set_lowpass(hz, BUTTERWORTH_Q);
        Self([b; 2])
    }

    fn highpass(hz: f32) -> Self {
        let mut b = Biquad::default();
        b.set_highpass(hz, BUTTERWORTH_Q);
        Self([b; 2])
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.0[0].process(x);
        self.0[1].process(y)
    }
}

/// Réglages du compresseur multibande.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultibandSettings {
    /// Fréquences de séparation graves/médiums et médiums/aigus (Hz).
    pub crossovers_hz: [f32; 2],
    /// Seuil par bande (dBFS).
    pub threshold_db: [f32; 3],
    /// Taux de compression par bande.
    pub ratio: [f32; 3],
}

#[derive(Debug, Default)]
pub struct Multiband {
    low_lp: Lr4,
    low_hp: Lr4,
    /// Passe-tout de la bande grave : passe-bas + passe-haut à la 2e coupure.
    low_ap: (Lr4, Lr4),
    mid_lp: Lr4,
    mid_hp: Lr4,
    comps: [Compressor; 3],
    crossovers_hz: [f32; 2],
}

impl Multiband {
    pub fn configure(&mut self, s: &MultibandSettings) {
        let lo = if s.crossovers_hz[0].is_finite() { s.crossovers_hz[0].clamp(60.0, 1_000.0) } else { 250.0 };
        let hi = if s.crossovers_hz[1].is_finite() { s.crossovers_hz[1].clamp(lo * 2.0, 12_000.0) } else { 4_000.0 };
        // Recalculer les filtres remettrait leur état à zéro (petit clic) :
        // seulement si les fréquences changent.
        let same = |a: f32, b: f32| (a - b).abs() < 1e-3;
        if !(same(self.crossovers_hz[0], lo) && same(self.crossovers_hz[1], hi)) {
            self.crossovers_hz = [lo, hi];
            self.low_lp = Lr4::lowpass(lo);
            self.low_hp = Lr4::highpass(lo);
            self.low_ap = (Lr4::lowpass(hi), Lr4::highpass(hi));
            self.mid_lp = Lr4::lowpass(hi);
            self.mid_hp = Lr4::highpass(hi);
        }
        for (i, c) in self.comps.iter_mut().enumerate() {
            let (attack, release) = TIMES_MS[i];
            c.configure(s.threshold_db[i], s.ratio[i], attack, release, KNEE_DB, 0.0);
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let low = self.low_lp.process(x);
        let low = self.low_ap.0.process(low) + self.low_ap.1.process(low);
        let rest = self.low_hp.process(x);
        let mid = self.mid_lp.process(rest);
        let high = self.mid_hp.process(rest);
        self.comps[0].process(low) + self.comps[1].process(mid) + self.comps[2].process(high)
    }

    /// Réduction de gain de chaque bande (dB, ≤ 0), pour l'affichage.
    pub fn gain_reduction_db(&self) -> [f32; 3] {
        [self.comps[0].gain_reduction_db(), self.comps[1].gain_reduction_db(), self.comps[2].gain_reduction_db()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::SAMPLE_RATE;

    fn settings(threshold_db: f32, ratio: f32) -> MultibandSettings {
        MultibandSettings { crossovers_hz: [250.0, 4_000.0], threshold_db: [threshold_db; 3], ratio: [ratio; 3] }
    }

    /// Gain (dB, en valeur efficace) en régime établi pour une sinusoïde
    /// d'amplitude `amp`.
    fn gain_at(m: &mut Multiband, hz: f32, amp: f32) -> f32 {
        let n = SAMPLE_RATE as usize;
        let (mut e_in, mut e_out) = (0f64, 0f64);
        for i in 0..n {
            let x = amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SAMPLE_RATE).sin();
            let y = m.process(x);
            if i >= n / 2 {
                e_in += f64::from(x * x);
                e_out += f64::from(y * y);
            }
        }
        (10.0 * (e_out / e_in).log10()) as f32
    }

    #[test]
    fn neutral_settings_keep_the_spectrum_flat() {
        for hz in [50.0, 150.0, 250.0, 600.0, 2_000.0, 4_000.0, 8_000.0, 15_000.0] {
            let mut m = Multiband::default();
            m.configure(&settings(0.0, 1.0));
            let g = gain_at(&mut m, hz, 0.1);
            assert!(g.abs() < 0.1, "{hz} Hz : {g:.2} dB");
        }
    }

    #[test]
    fn each_band_is_compressed_on_its_own() {
        // Médiums seuls compressés : un 1 kHz fort baisse, un 100 Hz fort non.
        let s = MultibandSettings { threshold_db: [0.0, -30.0, 0.0], ratio: [1.0, 4.0, 1.0], ..settings(0.0, 1.0) };
        let mut m = Multiband::default();
        m.configure(&s);
        let mid = gain_at(&mut m, 1_000.0, 0.3);
        let mut m = Multiband::default();
        m.configure(&s);
        let low = gain_at(&mut m, 100.0, 0.3);
        assert!(mid < -10.0, "1 kHz : {mid:.1} dB");
        assert!(low.abs() < 0.5, "100 Hz : {low:.1} dB");
        assert!(m.gain_reduction_db()[1] > -0.5, "un 100 Hz ne déclenche pas les médiums");
        assert!(m.gain_reduction_db()[0].abs() < 0.01);
    }

    #[test]
    fn invalid_crossovers_fall_back_to_safe_values() {
        let mut m = Multiband::default();
        m.configure(&MultibandSettings { crossovers_hz: [f32::NAN, 10.0], ..settings(0.0, 1.0) });
        assert_eq!(m.crossovers_hz, [250.0, 500.0]);
        let g = gain_at(&mut m, 1_000.0, 0.1);
        assert!(g.abs() < 0.1, "{g}");
    }
}
