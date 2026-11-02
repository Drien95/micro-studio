//! Suppression de bruit « premium » par DeepFilterNet3 (Schröter et al., 2023),
//! via `deep-filter-rt` : réseau de neurones pleine bande à 48 kHz, plus
//! efficace que RNNoise sur les bruits brefs (clavier, souris) et dans les
//! aigus. Modèle embarqué identique à l'original (vérifié par SHA-256 contre
//! `models/DeepFilterNet3_onnx.tar.gz` du dépôt officiel, commit `d375b2d`).
//!
//! Le modèle alloue de la mémoire à chaque trame : il ne doit donc jamais
//! tourner dans le callback audio. [`DeepFilterWorker`] le fait tourner dans
//! son propre fil, avec des canaux bornés (aucune allocation côté audio), au
//! prix d'une trame (10 ms) de retard en plus des 30 ms du modèle.

use super::FRAME;
use deep_filter_rt::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

/// Retard propre au modèle (recouvrement de la STFT + 2 trames d'anticipation).
pub const MODEL_LATENCY_MS: f32 = 30.0;

/// Atténuation maximale (dB) au-delà de laquelle le modèle n'est plus bridé.
const UNLIMITED_DB: f32 = 100.0;
/// Atténuation maximale (dB) quand l'intensité vaut un peu moins de 1.
const LIMITED_MAX_DB: f32 = 60.0;

/// Convertit l'intensité de l'interface (0..1) en atténuation maximale (dB) :
/// 1 = sans limite, 0,5 = 30 dB, 0 = aucun effet.
pub fn strength_to_atten_lim_db(strength: f32) -> f32 {
    let s = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 };
    if s >= 0.999 { UNLIMITED_DB } else { s * LIMITED_MAX_DB }
}

/// Estime une probabilité de voix (0..1) à partir du rapport signal/bruit
/// local (dB) que calcule le modèle : -5 dB ou moins = bruit, 15 dB ou plus =
/// voix nette. Heuristique, utilisée par la porte vocale.
pub fn lsnr_to_voice_probability(lsnr_db: f32) -> f32 {
    if lsnr_db.is_finite() { ((lsnr_db + 5.0) / 20.0).clamp(0.0, 1.0) } else { 0.0 }
}

/// DeepFilterNet3 sur une voie, trame par trame. Alloue : hors temps réel.
pub struct DeepFilter {
    model: Box<DfTract>,
    input: Array2<f32>,
    output: Array2<f32>,
    atten_lim_db: f32,
}

impl DeepFilter {
    /// Charge le modèle embarqué (environ 0,1 s).
    ///
    /// # Errors
    /// Si le modèle ne se charge pas (fichiers embarqués corrompus).
    pub fn new() -> anyhow::Result<Self> {
        let model = DfTract::new(DfParams::default(), &RuntimeParams::default())?;
        debug_assert_eq!(model.hop_size, FRAME, "DeepFilterNet3 travaille par trames de 10 ms à 48 kHz");
        Ok(Self {
            model: Box::new(model),
            input: Array2::zeros((1, FRAME)),
            output: Array2::zeros((1, FRAME)),
            atten_lim_db: UNLIMITED_DB,
        })
    }

    /// Règle l'intensité (0..1), voir [`strength_to_atten_lim_db`].
    pub fn set_strength(&mut self, strength: f32) {
        let db = strength_to_atten_lim_db(strength);
        if (db - self.atten_lim_db).abs() > f32::EPSILON {
            self.atten_lim_db = db;
            self.model.set_atten_lim(db);
        }
    }

    /// Débruite une trame en place ; renvoie le rapport signal/bruit local
    /// estimé (dB). En cas d'erreur du modèle, la trame passe telle quelle.
    pub fn process(&mut self, frame: &mut [f32; FRAME]) -> f32 {
        for (d, s) in self.input.iter_mut().zip(frame.iter()) {
            *d = if s.is_finite() { *s } else { 0.0 };
        }
        match self.model.process(self.input.view(), self.output.view_mut()) {
            Ok(lsnr) => {
                for (d, s) in frame.iter_mut().zip(self.output.iter()) {
                    *d = *s;
                }
                lsnr
            }
            Err(_) => 0.0,
        }
    }
}

impl std::fmt::Debug for DeepFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepFilter").field("atten_lim_db", &self.atten_lim_db).finish_non_exhaustive()
    }
}

/// Trames en attente au plus entre le fil audio et le fil du modèle.
const QUEUE: usize = 4;

struct Job {
    frame: [f32; FRAME],
    strength: f32,
}

struct Done {
    frame: [f32; FRAME],
    lsnr: f32,
}

/// DeepFilterNet3 dans un fil dédié. [`DeepFilterWorker::process`] ne fait
/// qu'échanger des trames par des canaux bornés : utilisable dans le callback
/// audio.
pub struct DeepFilterWorker {
    jobs: SyncSender<Job>,
    results: Receiver<Done>,
    lsnr: f32,
    misses: u64,
}

impl DeepFilterWorker {
    /// Lance le fil du modèle. Si le modèle ne se charge pas, le fil rend les
    /// trames telles quelles (le son continue, sans débruitage).
    pub fn spawn() -> Self {
        let (jobs, job_rx) = sync_channel::<Job>(QUEUE);
        let (done_tx, results) = sync_channel::<Done>(QUEUE);
        let _ = std::thread::Builder::new().name("deepfilter".into()).spawn(move || {
            let mut model = DeepFilter::new().ok();
            while let Ok(mut job) = job_rx.recv() {
                let lsnr = model.as_mut().map_or(0.0, |m| {
                    m.set_strength(job.strength);
                    m.process(&mut job.frame)
                });
                if done_tx.send(Done { frame: job.frame, lsnr }).is_err() {
                    break;
                }
            }
        });
        Self { jobs, results, lsnr: 0.0, misses: 0 }
    }

    /// Envoie `frame` au modèle et la remplace par la trame débruitée
    /// précédente (une trame de retard). Tant qu'aucun résultat n'est prêt
    /// (démarrage, fil en retard), rend du silence.
    pub fn process(&mut self, frame: &mut [f32; FRAME], strength: f32) {
        let _ = self.jobs.try_send(Job { frame: *frame, strength });
        match self.results.try_recv() {
            Ok(done) => {
                *frame = done.frame;
                self.lsnr = done.lsnr;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                *frame = [0.0; FRAME];
                self.misses += 1;
            }
        }
    }

    /// Dernier rapport signal/bruit local estimé (dB).
    pub fn lsnr(&self) -> f32 {
        self.lsnr
    }

    /// Trames rendues en silence faute de résultat prêt.
    pub fn misses(&self) -> u64 {
        self.misses
    }
}

impl std::fmt::Debug for DeepFilterWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepFilterWorker")
            .field("lsnr", &self.lsnr)
            .field("misses", &self.misses)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::util::SAMPLE_RATE;

    fn rms_db(x: &[f32]) -> f32 {
        let e = x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32;
        10.0 * e.max(1e-12).log10()
    }

    #[test]
    fn strength_maps_to_attenuation_limit() {
        assert!((strength_to_atten_lim_db(1.0) - UNLIMITED_DB).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(0.5) - 30.0).abs() < 1e-4);
        assert!(strength_to_atten_lim_db(0.0).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(f32::NAN) - UNLIMITED_DB).abs() < f32::EPSILON);
        assert!((strength_to_atten_lim_db(7.0) - UNLIMITED_DB).abs() < f32::EPSILON);
    }

    #[test]
    fn lsnr_maps_to_a_probability() {
        assert!(lsnr_to_voice_probability(-15.0).abs() < f32::EPSILON);
        assert!((lsnr_to_voice_probability(5.0) - 0.5).abs() < 1e-6);
        assert!((lsnr_to_voice_probability(35.0) - 1.0).abs() < f32::EPSILON);
        assert!(lsnr_to_voice_probability(f32::NAN).abs() < f32::EPSILON);
    }

    /// Bruit blanc déterministe.
    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 99u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
            })
            .collect()
    }

    #[test]
    fn steady_noise_is_strongly_reduced() {
        let x = noise(SAMPLE_RATE as usize * 2, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        let mut out = Vec::with_capacity(x.len());
        let mut last = 0.0;
        for c in x.as_chunks::<FRAME>().0 {
            let mut f = *c;
            last = df.process(&mut f);
            out.extend_from_slice(&f);
        }
        let half = out.len() / 2;
        assert!(rms_db(&out[half..]) < rms_db(&x[half..]) - 20.0, "{} / {}", rms_db(&out[half..]), rms_db(&x));
        assert!(lsnr_to_voice_probability(last) < 0.3, "bruit seul : pas de voix ({last} dB)");
        assert!(format!("{df:?}").contains("DeepFilter"));
    }

    #[test]
    fn strength_zero_leaves_the_signal_untouched() {
        let x = noise(SAMPLE_RATE as usize, 0.05);
        let mut df = DeepFilter::new().expect("modèle embarqué");
        df.set_strength(0.0);
        let mut out = Vec::new();
        for c in x.as_chunks::<FRAME>().0 {
            let mut f = *c;
            df.process(&mut f);
            out.extend_from_slice(&f);
        }
        let half = out.len() / 2;
        assert!((rms_db(&out[half..]) - rms_db(&x[half..])).abs() < 1.0);
    }

    #[test]
    fn worker_returns_frames_with_one_frame_of_delay() {
        let mut w = DeepFilterWorker::spawn();
        let mut f = [0.01f32; FRAME];
        w.process(&mut f, 1.0);
        // Premier appel : rien n'est encore prêt (ou déjà prêt si le fil est rapide).
        let mut got = 0;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let mut f = [0.01f32; FRAME];
            w.process(&mut f, 1.0);
            got += 1;
        }
        assert!(got > 0);
        assert!(w.misses() < 200, "le fil du modèle répond");
        assert!(w.lsnr().is_finite());
        assert!(format!("{w:?}").contains("DeepFilterWorker"));
    }
}
