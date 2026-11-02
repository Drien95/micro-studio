//! Suppression de bruit par réseau de neurones (`RNNoise`, via nnnoiseless).

use nnnoiseless::DenoiseState;

pub const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 échantillons = 10 ms à 48 kHz

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
    /// Traite une trame de 480 échantillons en place. `mix` dans [0, 1].
    pub fn process(&mut self, frame: &mut [f32; FRAME], mix: f32) {
        for (d, s) in self.scratch_in.iter_mut().zip(frame.iter()) {
            *d = s * 32768.0;
        }
        self.last_vad = self.state.process_frame(&mut self.scratch_out, &self.scratch_in);
        let mix = mix.clamp(0.0, 1.0);
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
