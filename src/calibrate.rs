//! Calibrage automatique (« benchmark ») : à partir de quelques secondes de
//! silence et d'une lecture à voix haute, mesure le micro, la pièce et la voix,
//! propose des réglages, puis organise un tournoi entre variantes pour garder
//! celle qui obtient le meilleur score.

use crate::dsp::util::{SAMPLE_RATE, db_to_lin, lin_to_db};
use crate::dsp::{Chain, FRAME};
use crate::params::Params;
use rustfft::{FftPlanner, num_complex::Complex};

pub const SILENCE_SECS: f32 = 8.0;
pub const SPEECH_SECS: f32 = 40.0;

/// Texte à lire pendant la mesure : il contient des « s », des « ch », des
/// plosives (« p », « b ») et des voyelles variées.
pub const READING_TEXT: &str = "Bonjour, je teste mon micro avec Micro Studio. \
Ce matin, six chasseurs sachant chasser sans leur chien sont passés près du pont. \
Le petit bateau bleu part vers la plage, puis il revient doucement au port. \
Je parle normalement, ni trop fort ni trop bas, comme pendant une partie entre amis. \
Parfois je ris, parfois je chuchote, et parfois je m'emporte un peu : c'est la vie ! \
Pour finir, je compte jusqu'à dix : un, deux, trois, quatre, cinq, six, sept, huit, neuf, dix.";

/// Niveau moyen de voix visé en sortie (dBFS RMS), proche des pratiques de stream.
pub const TARGET_SPEECH_DB: f32 = -18.0;
/// Niveau visé avant compression, après le gain d'entrée.
const PRE_COMP_SPEECH_DB: f32 = -22.0;

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub noise_floor_db: f32,
    pub speech_level_db: f32,
    pub speech_peak_db: f32,
    pub snr_db: f32,
    pub dynamic_range_db: f32,
    pub low_noise_excess_db: f32,
    pub band_levels: BandLevels,
    pub sibilance_freq_hz: f32,
    pub sibilance_p90_db: f32,
    pub clipped_ratio: f32,
}

/// Densité spectrale moyenne (dB) de la voix dans des zones utiles à l'ingé son.
#[derive(Clone, Copy, Debug, Default)]
pub struct BandLevels {
    pub body: f32,     // 200–500 Hz : corps
    pub boxy: f32,     // 500–1500 Hz : son « carton »
    pub presence: f32, // 2–5 kHz : présence, intelligibilité
    pub air: f32,      // 9–14 kHz : air
}

/// Cible relative au « corps » de la voix, dérivée du spectre moyen de la
/// parole (ILTASS, Byrne et al. 1994) converti en densité par Hz.
/// Heuristique : à affiner avec des mesures réelles.
const TARGET_BOXY: f32 = -9.0;
const TARGET_PRESENCE: f32 = -22.0;
const TARGET_AIR: f32 = -36.0;

fn frame_rms_db(x: &[f32]) -> Vec<f32> {
    x.chunks(FRAME)
        .filter(|c| c.len() == FRAME)
        .map(|c| {
            let e = c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32;
            10.0 * e.max(1e-12).log10()
        })
        .collect()
}

fn percentile(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return -120.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let idx = ((s.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    s[idx]
}

/// Spectre de puissance moyen (fenêtre de Hann, 2048 points) des trames choisies.
fn average_spectrum(x: &[f32], keep: impl Fn(usize) -> bool) -> Vec<f32> {
    const N: usize = 2048;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let window: Vec<f32> =
        (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
    let mut acc = vec![0f32; N / 2];
    let mut count = 0;
    let mut start = 0;
    let mut buf = vec![Complex::new(0f32, 0f32); N];
    while start + N <= x.len() {
        if keep(start / FRAME) {
            for i in 0..N {
                buf[i] = Complex::new(x[start + i] * window[i], 0.0);
            }
            fft.process(&mut buf);
            for (a, c) in acc.iter_mut().zip(buf.iter()) {
                *a += c.norm_sqr();
            }
            count += 1;
        }
        start += N / 2;
    }
    if count > 0 {
        for a in &mut acc {
            *a /= count as f32;
        }
    }
    acc
}

fn band_db(spec: &[f32], lo: f32, hi: f32) -> f32 {
    let bin_hz = SAMPLE_RATE / (2.0 * spec.len() as f32);
    let (a, b) = ((lo / bin_hz) as usize, ((hi / bin_hz) as usize).min(spec.len() - 1));
    let n = (b - a).max(1) as f32;
    let mean = spec[a..b].iter().sum::<f32>() / n;
    10.0 * mean.max(1e-20).log10()
}

fn peak_freq(spec: &[f32], lo: f32, hi: f32) -> f32 {
    let bin_hz = SAMPLE_RATE / (2.0 * spec.len() as f32);
    let (a, b) = ((lo / bin_hz) as usize, (hi / bin_hz) as usize);
    let mut best = a;
    for i in a..b.min(spec.len()) {
        if spec[i] > spec[best] {
            best = i;
        }
    }
    best as f32 * bin_hz
}

pub fn analyze(silence: &[f32], speech: &[f32]) -> Analysis {
    let sil = frame_rms_db(silence);
    let sp = frame_rms_db(speech);
    let noise_floor = percentile(&sil, 0.5);

    // Trames « parlées » : au moins 12 dB au-dessus du bruit de fond.
    let active: Vec<bool> = sp.iter().map(|&l| l > noise_floor + 12.0).collect();
    let active_levels: Vec<f32> = sp.iter().zip(&active).filter(|(_, a)| **a).map(|(l, _)| *l).collect();
    let speech_level = percentile(&active_levels, 0.5);
    let peak = speech.iter().fold(0f32, |m, s| m.max(s.abs()));
    let clipped = speech.iter().filter(|s| s.abs() > 0.98).count() as f32 / speech.len().max(1) as f32;

    let sp_spec = average_spectrum(speech, |i| active.get(i).copied().unwrap_or(false));
    let noise_spec = average_spectrum(silence, |_| true);

    let bands = BandLevels {
        body: band_db(&sp_spec, 200.0, 500.0),
        boxy: band_db(&sp_spec, 500.0, 1500.0),
        presence: band_db(&sp_spec, 2000.0, 5000.0),
        air: band_db(&sp_spec, 9000.0, 14000.0),
    };
    let low_noise_excess = band_db(&noise_spec, 30.0, 120.0) - band_db(&noise_spec, 300.0, 3000.0);

    // Niveau de sifflante trame par trame (filtre passe-bande 5–9 kHz).
    let mut bp = crate::dsp::biquad::Biquad::default();
    let sib_freq = peak_freq(&sp_spec, 4500.0, 9000.0).clamp(4500.0, 9000.0);
    bp.set_bandpass(sib_freq, 1.4);
    let filtered: Vec<f32> = speech.iter().map(|&s| bp.process(s)).collect();
    let sib_levels: Vec<f32> =
        frame_rms_db(&filtered).into_iter().zip(&active).filter(|(_, a)| **a).map(|(l, _)| l).collect();

    Analysis {
        noise_floor_db: noise_floor,
        speech_level_db: speech_level,
        speech_peak_db: lin_to_db(peak),
        snr_db: speech_level - noise_floor,
        dynamic_range_db: percentile(&active_levels, 0.9) - percentile(&active_levels, 0.1),
        low_noise_excess_db: low_noise_excess,
        band_levels: bands,
        sibilance_freq_hz: sib_freq,
        sibilance_p90_db: percentile(&sib_levels, 0.9),
        clipped_ratio: clipped,
    }
}

/// Réglages de départ déduits des mesures.
pub fn propose(a: &Analysis) -> Params {
    let mut p = Params::default();

    // Gain : amener la voix vers -22 dBFS avant compression, sans faire saturer les pointes.
    let mut gain = PRE_COMP_SPEECH_DB - a.speech_level_db;
    gain = gain.min(-3.0 - a.speech_peak_db + 6.0); // le limiteur gère les quelques dB restants
    p.input_gain_db = gain.clamp(-12.0, 30.0);
    let g = p.input_gain_db;

    // Passe-haut : plus haut si la pièce a beaucoup de grondement grave.
    p.hpf_hz = if a.low_noise_excess_db > 10.0 { 120.0 } else { 90.0 };

    // Noise gate : entre le bruit de fond et la voix, au moins 6 dB au-dessus du bruit.
    let noise = a.noise_floor_db + g;
    let speech = a.speech_level_db + g;
    let margin = (0.35 * (speech - noise)).max(6.0);
    p.gate_threshold_db = (noise + margin).min(speech - 12.0).clamp(-70.0, -20.0);

    // Débruitage : toujours actif ; un peu de signal d'origine si le micro est déjà propre.
    p.denoise_mix = if a.snr_db > 45.0 { 0.8 } else { 1.0 };

    // De-esser : n'agit que sur les 10 % de sifflantes les plus fortes.
    p.deesser_freq_hz = a.sibilance_freq_hz;
    p.deesser_threshold_db = (a.sibilance_p90_db + g - 2.0).clamp(-60.0, -6.0);

    // EQ : on rapproche le spectre de la cible, avec prudence (70 % de la correction, ±6 dB).
    let b = a.band_levels;
    let corr = |target: f32, measured: f32, lo: f32, hi: f32| ((target - (measured - b.body)) * 0.7).clamp(lo, hi);
    p.eq_bands[0].gain_db = 0.0;
    p.eq_bands[1].gain_db = corr(TARGET_BOXY, b.boxy, -6.0, 2.0);
    p.eq_bands[2].gain_db = corr(TARGET_PRESENCE, b.presence, -4.0, 5.0);
    p.eq_bands[3].gain_db = corr(TARGET_AIR, b.air, -3.0, 4.0);

    // Compresseur : plus la voix varie, plus le ratio est élevé.
    p.comp_ratio = if a.dynamic_range_db < 8.0 {
        2.5
    } else if a.dynamic_range_db < 14.0 {
        3.5
    } else {
        5.0
    };
    p.comp_threshold_db = PRE_COMP_SPEECH_DB - 4.0;
    let expected_gr = 4.0 * (1.0 - 1.0 / p.comp_ratio);
    p.comp_makeup_db = (TARGET_SPEECH_DB - PRE_COMP_SPEECH_DB + expected_gr).clamp(0.0, 12.0);
    p
}

#[allow(dead_code)] // détails affichables dans un futur rapport
#[derive(Clone, Debug, Default)]
pub struct Score {
    pub total: f32,
    pub noise_reduction_db: f32,
    pub level_spread_db: f32,
    pub loudness_error_db: f32,
    pub words_cut_ratio: f32,
    pub clipped_ratio: f32,
    pub tonal_error_db: f32,
}

/// Note un rendu : plus c'est haut, mieux c'est.
/// `speech_mask` indique, trame par trame, où il y avait de la voix dans l'enregistrement d'origine.
pub fn score(output: &[f32], speech_mask: &[bool]) -> Score {
    let levels = frame_rms_db(output);
    let n = levels.len().min(speech_mask.len());
    let sp: Vec<f32> = (0..n).filter(|&i| speech_mask[i]).map(|i| levels[i]).collect();
    let quiet: Vec<f32> = (0..n).filter(|&i| !speech_mask[i]).map(|i| levels[i]).collect();
    let speech_level = percentile(&sp, 0.5);
    let noise_level = percentile(&quiet, 0.5).max(-100.0);

    // Mots coupés : trames de voix rendues plus de 15 dB sous le niveau de voix médian.
    let cut = sp.iter().filter(|&&l| l < speech_level - 15.0).count() as f32 / sp.len().max(1) as f32;
    let spread = percentile(&sp, 0.9) - percentile(&sp, 0.1);
    let loud_err = (speech_level - TARGET_SPEECH_DB).abs();
    let clipped = output.iter().filter(|s| s.abs() > 0.995).count() as f32 / output.len().max(1) as f32;

    let spec = average_spectrum(output, |i| speech_mask.get(i).copied().unwrap_or(false));
    let body = band_db(&spec, 200.0, 500.0);
    let tonal = ((band_db(&spec, 500.0, 1500.0) - body - TARGET_BOXY).abs()
        + (band_db(&spec, 2000.0, 5000.0) - body - TARGET_PRESENCE).abs()
        + (band_db(&spec, 9000.0, 14000.0) - body - TARGET_AIR).abs())
        / 3.0;

    let nr = (speech_level - noise_level).min(60.0);
    let total = nr * 1.0 - spread * 1.5 - loud_err * 1.0 - cut * 200.0 - clipped * 5000.0 - tonal * 0.5;
    Score {
        total,
        noise_reduction_db: nr,
        level_spread_db: spread,
        loudness_error_db: loud_err,
        words_cut_ratio: cut,
        clipped_ratio: clipped,
        tonal_error_db: tonal,
    }
}

/// Résultat complet d'un calibrage.
#[derive(Clone, Debug)]
pub struct CalibrationResult {
    pub analysis: Analysis,
    pub params: Params,
    pub score: Score,
    pub baseline_score: Score,
    pub candidates_tested: usize,
}

/// Lance tout le calibrage : analyse, proposition, puis tournoi des variantes.
/// `progress` reçoit une valeur entre 0 et 1.
pub fn run(silence: &[f32], speech: &[f32], mut progress: impl FnMut(f32)) -> CalibrationResult {
    let analysis = analyze(silence, speech);
    let base = propose(&analysis);

    // Enregistrement complet : silence puis voix, avec le masque « voix » d'origine.
    let mut full = Vec::with_capacity(silence.len() + speech.len());
    full.extend_from_slice(silence);
    full.extend_from_slice(speech);
    let gate_level = analysis.noise_floor_db + 12.0;
    let mask: Vec<bool> = frame_rms_db(&full).iter().map(|&l| l > gate_level).collect();

    let baseline = Params { bypass: true, input_gain_db: base.input_gain_db, ..Params::default() };
    let baseline_score = score(&Chain::render(&baseline, &full).0, &mask);

    let mut candidates = Vec::new();
    for gate_off in [-4.0, 0.0, 3.0] {
        for ratio_mul in [0.75, 1.0, 1.3] {
            for eq_scale in [0.5, 1.0] {
                for mix in [0.85, 1.0] {
                    let mut p = base.clone();
                    p.gate_threshold_db += gate_off;
                    p.comp_ratio = (p.comp_ratio * ratio_mul).max(1.5);
                    for b in &mut p.eq_bands {
                        b.gain_db *= eq_scale;
                    }
                    p.denoise_mix = mix;
                    candidates.push(p);
                }
            }
        }
    }

    // Le tournoi est réparti sur tous les cœurs du processeur.
    let total = candidates.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, std::num::NonZero::get).min(total);
    let per = total.div_ceil(workers);
    let mut results: Vec<(Params, Score)> = Vec::with_capacity(total);
    std::thread::scope(|scope| {
        let handles: Vec<_> = candidates
            .chunks(per)
            .map(|chunk| {
                let (full, mask, done) = (&full, &mask, &done);
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|p| {
                            let r = evaluate(p.clone(), full, mask);
                            done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            r
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        while handles.iter().any(|h| !h.is_finished()) {
            progress(done.load(std::sync::atomic::Ordering::Relaxed) as f32 / total as f32);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for h in handles {
            results.extend(h.join().expect("calcul du tournoi"));
        }
    });
    progress(1.0);
    let best = results.into_iter().max_by(|a, b| a.1.total.total_cmp(&b.1.total));
    let (params, score) = best.expect("au moins un candidat");
    CalibrationResult { analysis, params, score, baseline_score, candidates_tested: total }
}

/// Ajuste le gain de sortie pour viser le bon volume, puis note le rendu.
fn evaluate(mut p: Params, full: &[f32], mask: &[bool]) -> (Params, Score) {
    let (out, _) = Chain::render(&p, full);
    let speech: Vec<f32> = frame_rms_db(&out).iter().zip(mask).filter(|(_, m)| **m).map(|(l, _)| *l).collect();
    p.output_gain_db = (TARGET_SPEECH_DB - percentile(&speech, 0.5)).clamp(-6.0, 6.0);
    let (out, _) = Chain::render(&p, full);
    let s = score(&out, mask);
    (p, s)
}

/// Signal de test synthétique (voix simulée + bruit), utilisé par les tests.
///
/// Renvoie `(silence, voix)` : un quart de la durée de bruit seul, puis `secs`
/// secondes de syllabes harmoniques (200 ms de son, 100 ms de pause) sur ce bruit.
/// Déterministe (générateur congruentiel à graine fixe).
pub fn synthetic_voice(secs: f32, noise_db: f32, voice_db: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (secs * SAMPLE_RATE) as usize;
    let mut seed = 12345u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let nl = db_to_lin(noise_db) * 1.7;
    let silence: Vec<f32> = (0..n / 4).map(|_| noise() * nl).collect();
    let vl = db_to_lin(voice_db) * 1.414;
    let speech: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            // Syllabes de 200 ms séparées de 100 ms de silence.
            let on = (t % 0.3) < 0.2;
            let f0 = 140.0;
            let v: f32 = (1..12).map(|h| (2.0 * std::f32::consts::PI * f0 * h as f32 * t).sin() / h as f32).sum();
            (if on { v * vl * 0.6 } else { 0.0 }) + noise() * nl
        })
        .collect();
    (silence, speech)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_on_synthetic_voice_is_sane() {
        let (sil, sp) = synthetic_voice(6.0, -60.0, -30.0);
        let a = analyze(&sil, &sp);
        assert!((a.noise_floor_db + 60.0).abs() < 3.0, "bruit {}", a.noise_floor_db);
        assert!(a.snr_db > 20.0);
        let r = run(&sil, &sp, |_| {});
        assert!(r.params.input_gain_db > 0.0, "voix faible : il faut monter le gain");
        assert!(r.params.gate_threshold_db > a.noise_floor_db + r.params.input_gain_db);
        assert!(r.score.words_cut_ratio < 0.1, "{:?}", r.score);
        assert!(r.score.total > r.baseline_score.total, "{:?} vs {:?}", r.score, r.baseline_score);
    }
}
