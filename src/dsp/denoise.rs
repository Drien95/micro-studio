//! Suppression de bruit par réseau de neurones (`RNNoise`, via nnnoiseless).

use nnnoiseless::DenoiseState;

pub const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 échantillons = 10 ms à 48 kHz

/// Limite d'atténuation à partir de laquelle le débruiteur n'est plus bridé.
pub const UNLIMITED_DB: f32 = 100.0;

/// Part de signal d'origine à garder pour qu'un débruiteur n'atténue jamais le
/// bruit de plus de `limit_db` (même règle que `atten_lim` de `DeepFilterNet`) :
/// `sortie = débruité × (1 - part) + original × part`, `part = 10^(-limite/20)`.
/// Brider la réduction évite le son « métallique » d'un débruitage trop poussé.
pub fn dry_part(limit_db: f32) -> f32 {
    let lim = if limit_db.is_finite() { limit_db.abs() } else { UNLIMITED_DB };
    if lim >= UNLIMITED_DB { 0.0 } else { 10f32.powf(-lim / 20.0) }
}

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    scratch_in: [f32; FRAME],
    scratch_out: [f32; FRAME],
    /// `RNNoise` rend le signal avec une trame de retard : on retarde le signal sec
    /// d'autant pour pouvoir mélanger sans effet de filtre en peigne.
    dry_delay: [f32; FRAME],
    first: bool,
    last_vad: f32,
}

impl Default for Denoiser {
    fn default() -> Self {
        Self {
            state: DenoiseState::new(),
            scratch_in: [0.0; FRAME],
            scratch_out: [0.0; FRAME],
            dry_delay: [0.0; FRAME],
            first: true,
            last_vad: 0.0,
        }
    }
}

impl Denoiser {
    /// Traite une trame de 480 échantillons en place, sans atténuer le bruit
    /// de plus de `limit_db` (voir [`dry_part`]).
    pub fn process(&mut self, frame: &mut [f32; FRAME], limit_db: f32) {
        let mix = 1.0 - dry_part(limit_db);
        for (d, s) in self.scratch_in.iter_mut().zip(frame.iter()) {
            *d = s * 32768.0;
        }
        self.last_vad = self.state.process_frame(&mut self.scratch_out, &self.scratch_in);
        for ((x, delayed), out) in frame.iter_mut().zip(self.dry_delay.iter_mut()).zip(self.scratch_out.iter()) {
            let wet = if self.first { 0.0 } else { out / 32768.0 };
            let dry = std::mem::replace(delayed, *x);
            *x = wet * mix + dry * (1.0 - mix);
        }
        self.first = false;
    }

    /// Probabilité (0..1) que la trame contienne de la voix.
    pub fn vad(&self) -> f32 {
        self.last_vad
    }
}

impl std::fmt::Debug for Denoiser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Denoiser").field("first", &self.first).field("last_vad", &self.last_vad).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_signal_is_delayed_by_exactly_one_frame() {
        let mut d = Denoiser::default();
        let mut a = [0f32; FRAME];
        for (i, x) in a.iter_mut().enumerate() {
            *x = i as f32 / FRAME as f32;
        }
        let original = a;
        d.process(&mut a, 0.0);
        assert!(a.iter().all(|&x| x == 0.0), "première trame : rien encore");
        let mut b = [0f32; FRAME];
        d.process(&mut b, 0.0);
        assert!(
            b.iter().zip(&original).all(|(x, y)| x.to_bits() == y.to_bits()),
            "la trame précédente ressort intacte"
        );
    }

    #[test]
    fn vad_is_a_probability() {
        let mut d = Denoiser::default();
        let mut seed = 1u32;
        for _ in 0..20 {
            let mut f = [0f32; FRAME];
            for x in &mut f {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *x = (seed >> 9) as f32 / (1u32 << 23) as f32 * 0.2 - 0.1;
            }
            d.process(&mut f, UNLIMITED_DB);
            assert!((0.0..=1.0).contains(&d.vad()));
            assert!(f.iter().all(|x| x.is_finite()));
        }
    }

    #[test]
    fn steady_noise_is_attenuated_not_amplified() {
        let mut d = Denoiser::default();
        let mut seed = 7u32;
        let (mut e_in, mut e_out) = (0f32, 0f32);
        for k in 0..500 {
            let mut f = [0f32; FRAME];
            for x in &mut f {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *x = (seed >> 9) as f32 / (1u32 << 23) as f32 * 0.02 - 0.01;
            }
            let e: f32 = f.iter().map(|x| x * x).sum();
            d.process(&mut f, UNLIMITED_DB);
            if k > 300 {
                e_in += e;
                e_out += f.iter().map(|x| x * x).sum::<f32>();
            }
        }
        let reduction = 10.0 * (e_in / e_out.max(1e-20)).log10();
        // RNNoise est entraîné sur des bruits réels : un bruit blanc pur n'est que
        // légèrement atténué, mais il ne doit jamais être amplifié.
        assert!(reduction > 0.5, "réduction du bruit blanc : {reduction:.1} dB");
    }

    #[test]
    fn dry_part_follows_the_attenuation_limit() {
        assert!(dry_part(UNLIMITED_DB).abs() < f32::EPSILON, "sans limite : tout débruité");
        assert!(dry_part(250.0).abs() < f32::EPSILON);
        assert!((dry_part(0.0) - 1.0).abs() < f32::EPSILON, "0 dB : signal d'origine");
        assert!((dry_part(20.0) - 0.1).abs() < 1e-6, "20 dB : 10 % d'origine");
        assert!(dry_part(f32::NAN).abs() < f32::EPSILON, "réglage corrompu : sans limite");
    }
}
