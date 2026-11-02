//! La chaîne de traitement complète, dans l'ordre utilisé par les ingés son :
//! gain → passe-haut → noise gate → débruitage → de-esser → EQ → compresseur → limiteur.

pub mod biquad;
pub mod denoise;
pub mod dynamics;
pub mod resample;
pub mod util;

use crate::params::{EqKind, Params};
use biquad::Biquad;
use denoise::Denoiser;
pub use denoise::FRAME;
use dynamics::{Compressor, DeEsser, Gate, Limiter};
use util::{db_to_lin, lin_to_db};

/// Mesures d'une trame, pour les vumètres et le calibrage.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub in_peak_db: f32,
    pub out_peak_db: f32,
    pub gate_open: bool,
    pub gate_gain_db: f32,
    pub comp_gr_db: f32,
    pub deess_gr_db: f32,
    pub vad: f32,
}

pub struct Chain {
    params: Params,
    hpf: [Biquad; 2],
    gate: Gate,
    denoiser: Denoiser,
    deesser: DeEsser,
    eq: [Biquad; 4],
    comp: Compressor,
    limiter: Limiter,
}

impl Chain {
    pub fn new(params: &Params) -> Self {
        let mut c = Self {
            params: params.clone(),
            hpf: [Biquad::default(); 2],
            gate: Gate::default(),
            denoiser: Denoiser::default(),
            deesser: DeEsser::default(),
            eq: [Biquad::default(); 4],
            comp: Compressor::default(),
            limiter: Limiter::default(),
        };
        c.set_params(params);
        c
    }

    pub fn set_params(&mut self, p: &Params) {
        self.params = p.clone();
        // Deux biquads en série : pente de 24 dB/octave (Butterworth d'ordre 4).
        self.hpf[0].set_highpass(p.hpf_hz, 0.5412);
        self.hpf[1].set_highpass(p.hpf_hz, 1.3066);
        self.gate.configure(p.gate_threshold_db, p.gate_attack_ms, p.gate_hold_ms, p.gate_release_ms, p.gate_range_db);
        self.deesser.configure(p.deesser_freq_hz, p.deesser_threshold_db, p.deesser_max_reduction_db);
        for (f, b) in self.eq.iter_mut().zip(p.eq_bands.iter()) {
            match b.kind {
                EqKind::LowShelf => f.set_low_shelf(b.freq_hz, b.q, b.gain_db),
                EqKind::Peak => f.set_peak(b.freq_hz, b.q, b.gain_db),
                EqKind::HighShelf => f.set_high_shelf(b.freq_hz, b.q, b.gain_db),
            }
        }
        self.comp.configure(p.comp_threshold_db, p.comp_ratio, p.comp_attack_ms, p.comp_release_ms, p.comp_knee_db, p.comp_makeup_db);
        self.limiter.configure(p.limiter_ceiling_db);
    }

    /// Traite une trame de 10 ms (480 échantillons à 48 kHz) en place.
    pub fn process(&mut self, frame: &mut [f32; FRAME]) -> FrameStats {
        let p = &self.params;
        let mut st = FrameStats::default();
        let in_gain = db_to_lin(p.input_gain_db);
        let out_gain = db_to_lin(p.output_gain_db);

        let mut in_peak = 0f32;
        for x in frame.iter_mut() {
            *x *= in_gain;
            in_peak = in_peak.max(x.abs());
        }
        st.in_peak_db = lin_to_db(in_peak);

        if p.bypass {
            st.out_peak_db = st.in_peak_db;
            st.gate_open = true;
            return st;
        }

        let mut min_gate = 1f32;
        for x in frame.iter_mut() {
            let mut s = *x;
            if p.hpf_enabled {
                let h = self.hpf[0].process(s);
                s = self.hpf[1].process(h);
            }
            if p.gate_enabled {
                s = self.gate.process(s);
                min_gate = min_gate.min(self.gate.gain());
            }
            *x = s;
        }
        st.gate_open = !p.gate_enabled || self.gate.is_open();
        st.gate_gain_db = lin_to_db(min_gate);

        if p.denoise_enabled {
            self.denoiser.process(frame, p.denoise_mix);
            st.vad = self.denoiser.vad();
        }

        let mut max_gr = 0f32;
        let mut max_dgr = 0f32;
        let mut out_peak = 0f32;
        for x in frame.iter_mut() {
            let mut s = *x;
            if p.deesser_enabled {
                s = self.deesser.process(s);
                max_dgr = max_dgr.min(self.deesser.gain_reduction_db());
            }
            if p.eq_enabled {
                for f in self.eq.iter_mut() {
                    s = f.process(s);
                }
            }
            if p.comp_enabled {
                s = self.comp.process(s);
                max_gr = max_gr.min(self.comp.gain_reduction_db());
            }
            s *= out_gain;
            if p.limiter_enabled {
                s = self.limiter.process(s);
            }
            out_peak = out_peak.max(s.abs());
            *x = s;
        }
        st.comp_gr_db = max_gr;
        st.deess_gr_db = max_dgr;
        st.out_peak_db = lin_to_db(out_peak);
        st
    }

    /// Traite un enregistrement complet hors temps réel (utilisé par le calibrage).
    pub fn render(params: &Params, input: &[f32]) -> (Vec<f32>, Vec<FrameStats>) {
        let mut chain = Chain::new(params);
        let mut out = Vec::with_capacity(input.len());
        let mut stats = Vec::with_capacity(input.len() / FRAME + 1);
        for chunk in input.chunks(FRAME) {
            let mut f = [0f32; FRAME];
            f[..chunk.len()].copy_from_slice(chunk);
            stats.push(chain.process(&mut f));
            out.extend_from_slice(&f[..chunk.len()]);
        }
        (out, stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::SAMPLE_RATE;

    fn sine(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE).sin())
            .collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        let e = x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32;
        10.0 * e.max(1e-12).log10()
    }

    fn only(f: impl Fn(&mut Params)) -> Params {
        let mut p = Params {
            hpf_enabled: false,
            gate_enabled: false,
            denoise_enabled: false,
            deesser_enabled: false,
            eq_enabled: false,
            comp_enabled: false,
            limiter_enabled: false,
            ..Params::default()
        };
        f(&mut p);
        p
    }

    #[test]
    fn highpass_removes_rumble_keeps_voice() {
        let p = only(|p| {
            p.hpf_enabled = true;
            p.hpf_hz = 90.0;
        });
        let low = Chain::render(&p, &sine(30.0, 0.5, 1.0)).0;
        let mid = Chain::render(&p, &sine(1000.0, 0.5, 1.0)).0;
        assert!(rms_db(&low[24000..]) < rms_db(&sine(30.0, 0.5, 1.0)) - 30.0);
        assert!((rms_db(&mid[24000..]) - rms_db(&sine(1000.0, 0.5, 1.0))).abs() < 0.5);
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let p = only(|p| {
            p.limiter_enabled = true;
            p.limiter_ceiling_db = -1.0;
            p.input_gain_db = 12.0;
        });
        let out = Chain::render(&p, &sine(200.0, 0.9, 1.0)).0;
        let ceiling = util::db_to_lin(-1.0);
        assert!(out.iter().all(|s| s.abs() <= ceiling + 1e-6));
    }

    #[test]
    fn compressor_reduces_loud_parts() {
        let p = only(|p| {
            p.comp_enabled = true;
            p.comp_threshold_db = -20.0;
            p.comp_ratio = 4.0;
            p.comp_makeup_db = 0.0;
        });
        let loud = Chain::render(&p, &sine(500.0, 0.5, 1.0)).0; // ≈ -9 dB RMS
        let quiet = Chain::render(&p, &sine(500.0, 0.02, 1.0)).0; // ≈ -37 dB RMS
        assert!(rms_db(&loud[24000..]) < -14.0);
        assert!((rms_db(&quiet[24000..]) - rms_db(&sine(500.0, 0.02, 1.0))).abs() < 0.5);
    }

    #[test]
    fn gate_mutes_noise_floor() {
        let p = only(|p| {
            p.gate_enabled = true;
            p.gate_threshold_db = -40.0;
            p.gate_range_db = -60.0;
        });
        let out = Chain::render(&p, &sine(300.0, 0.001, 1.0)).0; // -60 dBFS
        assert!(rms_db(&out[24000..]) < -110.0);
    }

    #[test]
    fn denoiser_dry_path_is_aligned_with_wet() {
        // Vérifie l'hypothèse « une trame de retard » : un mélange 50/50 d'une
        // sinusoïde propre ne doit pas créer d'annulation (filtre en peigne).
        let p = only(|p| {
            p.denoise_enabled = true;
            p.denoise_mix = 0.5;
        });
        let x = sine(440.0, 0.3, 1.0);
        let out = Chain::render(&p, &x).0;
        let wet = Chain::render(&only(|p| { p.denoise_enabled = true; p.denoise_mix = 1.0; }), &x).0;
        let dry = Chain::render(&only(|p| { p.denoise_enabled = true; p.denoise_mix = 0.0; }), &x).0;
        let mut corr = 0f32;
        let (mut ew, mut ed) = (0f32, 0f32);
        for i in 24000..48000 {
            corr += wet[i] * dry[i];
            ew += wet[i] * wet[i];
            ed += dry[i] * dry[i];
        }
        let c = corr / (ew.sqrt() * ed.sqrt()).max(1e-9);
        assert!(c > 0.8, "corrélation sec/traité = {c}");
        assert!(rms_db(&out[24000..]) > rms_db(&x) - 6.0);
    }

    #[test]
    fn resampler_keeps_duration() {
        let mut r = resample::Resampler::new(44_100, 48_000);
        let mut n = 0;
        for _ in 0..44_100 {
            r.push(0.1, |_| n += 1);
        }
        assert!((n as i32 - 48_000).abs() <= 2, "{n}");
    }
}
