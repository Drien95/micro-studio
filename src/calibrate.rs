//! Calibrage automatique (« benchmark ») : à partir de quelques secondes de
//! silence, de frappe au clavier et d'une lecture à voix haute, mesure le
//! micro, la pièce, les bruits parasites et la voix, propose des réglages,
//! puis organise un tournoi entre variantes pour garder celle qui obtient le
//! meilleur score.

use crate::dnsmos::{Dnsmos, Mos};
use crate::dsp::denoise::Denoiser;
use crate::dsp::loudness::integrated_lufs;
use crate::dsp::speech_level::{self, ActiveSpeech};
use crate::dsp::util::{SAMPLE_RATE, db_to_lin, lin_to_db};
use crate::dsp::{Chain, FRAME};
use crate::params::{DenoiseEngine, MAX_NOTCHES, Params};
use rustfft::{FftPlanner, num_complex::Complex};
use std::ops::Range;

pub const SILENCE_SECS: f32 = 8.0;
/// Phase où l'on tape au clavier sans parler : mesure les bruits de touches.
pub const KEYBOARD_SECS: f32 = 8.0;
pub const SPEECH_SECS: f32 = 40.0;

/// Texte à lire pendant la mesure : il contient des « s », des « ch », des
/// plosives (« p », « b ») et des voyelles variées.
pub const READING_TEXT: &str = "Bonjour, je teste mon micro avec Micro Studio. \
Ce matin, six chasseurs sachant chasser sans leur chien sont passés près du pont. \
Le petit bateau bleu part vers la plage, puis il revient doucement au port. \
Je parle normalement, ni trop fort ni trop bas, comme pendant une partie entre amis. \
Parfois je ris, parfois je chuchote, et parfois je m'emporte un peu : c'est la vie ! \
Pour finir, je compte jusqu'à dix : un, deux, trois, quatre, cinq, six, sept, huit, neuf, dix.";

/// Sonie visée en sortie (LUFS intégrés, UIT-R BS.1770). L'AES (TD1004)
/// recommande -16 à -20 LUFS pour la voix diffusée en ligne ; -18 laisse une
/// marge aux applis de chat qui remontent elles-mêmes le volume.
pub const TARGET_LUFS: f32 = -18.0;
/// Niveau moyen de voix visé en sortie (dBFS RMS), utilisé pour le compresseur.
pub const TARGET_SPEECH_DB: f32 = -18.0;
/// Taux de compression maximal choisi par le calibrage. Les guides de
/// production de la voix (radio, podcast) s'accordent sur 2:1 à 4:1 pour
/// 3 à 6 dB de réduction ; au-delà, la voix devient plate et « pompée ».
const MAX_COMP_RATIO: f32 = 4.0;
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
    /// Niveau des touches de clavier (voir `burst_level`, dBFS RMS).
    pub keyboard_level_db: f32,
    /// Crête des touches de clavier (voir `burst_level`, dBFS).
    pub keyboard_peak_db: f32,
    /// Crête médiane de la voix (dBFS), à comparer à celle du clavier.
    pub speech_frame_peak_db: f32,
    /// Bourdonnements et sifflements continus trouvés dans le silence.
    pub tonal_noise: Vec<TonalPeak>,
}

/// Raie parasite continue (secteur 50 Hz et harmoniques, sifflement USB…).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TonalPeak {
    pub freq_hz: f32,
    /// Hauteur de la raie au-dessus du bruit voisin (dB).
    pub prominence_db: f32,
}

/// Enregistrement de l'assistant découpé en ses trois phases (48 kHz, mono).
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub silence: Vec<f32>,
    pub keyboard: Vec<f32>,
    pub speech: Vec<f32>,
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

fn frame_peak_db(x: &[f32]) -> Vec<f32> {
    x.chunks(FRAME)
        .filter(|c| c.len() == FRAME)
        .map(|c| lin_to_db(c.iter().fold(0f32, |m, s| m.max(s.abs()))))
        .collect()
}

/// Trames par fenêtre de 100 ms : on tape rarement plus de dix touches par seconde.
const BURST_FRAMES: usize = 10;

/// Niveau typique des bruits brefs (touches) : maximum de chaque fenêtre de
/// 100 ms, puis 75e centile. Un simple centile des trames raterait les clics,
/// qui n'occupent qu'une trame sur dix ou quinze.
fn burst_level(frame_levels: &[f32]) -> f32 {
    let maxima: Vec<f32> =
        frame_levels.chunks(BURST_FRAMES).map(|c| c.iter().copied().fold(f32::NEG_INFINITY, f32::max)).collect();
    percentile(&maxima, 0.75)
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
    spectrum_n(x, 2048, keep)
}

/// Spectre de puissance moyen sur `n` points (fenêtre de Hann, recouvrement 50 %).
fn spectrum_n(x: &[f32], n: usize, keep: impl Fn(usize) -> bool) -> Vec<f32> {
    let n = n.clamp(64, 1 << 16);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let window: Vec<f32> =
        (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
    let mut acc = vec![0f32; n / 2];
    let mut count = 0;
    let mut start = 0;
    let mut buf = vec![Complex::new(0f32, 0f32); n];
    while start + n <= x.len() {
        if keep(start / FRAME) {
            for i in 0..n {
                buf[i] = Complex::new(x[start + i] * window[i], 0.0);
            }
            fft.process(&mut buf);
            for (a, c) in acc.iter_mut().zip(buf.iter()) {
                *a += c.norm_sqr();
            }
            count += 1;
        }
        start += n / 2;
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

/// Taille de FFT pour chercher les raies : 2,9 Hz par case à 48 kHz, assez
/// fin pour séparer 50 Hz de ses voisins.
const TONAL_FFT: usize = 16_384;
/// Une raie doit dépasser le bruit voisin d'au moins autant pour être filtrée.
const TONAL_MIN_PROMINENCE_DB: f32 = 12.0;

/// Cherche les raies continues (bourdonnement, sifflement) dans le silence.
/// Renvoie au plus [`MAX_NOTCHES`] raies, de la plus marquée à la moins marquée.
pub fn detect_tonal_noise(silence: &[f32]) -> Vec<TonalPeak> {
    if silence.len() < TONAL_FFT {
        return Vec::new();
    }
    let spec = spectrum_n(silence, TONAL_FFT, |_| true);
    let bin_hz = SAMPLE_RATE / TONAL_FFT as f32;
    let db: Vec<f32> = spec.iter().map(|e| 10.0 * e.max(1e-20).log10()).collect();
    let (lo, hi) = ((40.0 / bin_hz) as usize, ((16_000.0 / bin_hz) as usize).min(db.len() - 30));
    let mut found: Vec<TonalPeak> = Vec::new();
    let mut neighbours = Vec::with_capacity(48);
    for i in lo..hi {
        // Maximum local sur ±3 cases (largeur du lobe principal de Hann).
        if (i - 3..=i + 3).any(|j| db[j] > db[i]) {
            continue;
        }
        neighbours.clear();
        neighbours.extend(db[i.saturating_sub(27)..i - 4].iter().chain(&db[i + 5..i + 28]).copied());
        let prominence = db[i] - percentile(&neighbours, 0.5);
        if prominence < TONAL_MIN_PROMINENCE_DB {
            continue;
        }
        // Interpolation parabolique pour affiner la fréquence au-delà de la case.
        let (a, b, c) = (db[i - 1], db[i], db[i + 1]);
        let denom = a - 2.0 * b + c;
        let offset = if denom.abs() > 1e-6 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
        found.push(TonalPeak { freq_hz: (i as f32 + offset) * bin_hz, prominence_db: prominence });
    }
    found.sort_by(|x, y| y.prominence_db.total_cmp(&x.prominence_db));
    found.truncate(MAX_NOTCHES);
    found
}

/// Seuil du détecteur de voix de RNNoise pour une trame « parlée ».
const VAD_SPEECH: f32 = 0.5;
/// Élargissement du détecteur de voix (trames de chaque côté) : il hésite
/// sur les consonnes et les fins de mots.
const VAD_DILATION: usize = 3;

/// Activité vocale échantillon par échantillon : méthode B de l'UIT-T P.56
/// (enveloppe, seuil adaptatif, maintien de 200 ms), croisée avec le
/// détecteur de voix de RNNoise pour écarter les bruits forts qui ne sont
/// pas de la voix. Si RNNoise ne reconnaît presque rien (voix atypique,
/// signal de test), P.56 seule fait foi.
pub fn speech_activity(speech: &[f32]) -> (Option<ActiveSpeech>, Vec<bool>) {
    let Some((m, p56)) = speech_level::measure(speech) else {
        return (None, vec![false; speech.len()]);
    };
    let mut den = Denoiser::default();
    let raw: Vec<bool> = speech
        .as_chunks::<FRAME>()
        .0
        .iter()
        .map(|c| {
            let mut f = *c;
            den.process(&mut f, 1.0);
            den.vad() >= VAD_SPEECH
        })
        .collect();
    let vad: Vec<bool> = (0..raw.len())
        .map(|i| raw[i.saturating_sub(VAD_DILATION)..(i + VAD_DILATION + 1).min(raw.len())].iter().any(|&v| v))
        .collect();
    let crossed: Vec<bool> =
        p56.iter().enumerate().map(|(i, &a)| a && vad.get(i / FRAME).copied().unwrap_or(true)).collect();
    let (n_p56, n_both) = (p56.iter().filter(|&&a| a).count(), crossed.iter().filter(|&&a| a).count());
    let mask = if n_both as f32 >= 0.3 * n_p56 as f32 { crossed } else { p56 };
    (Some(m), mask)
}

/// Masque par trame de 10 ms à partir d'un masque par échantillon qui
/// commence à l'échantillon `start` : une trame est active si plus de la
/// moitié de ses échantillons le sont.
pub fn frames_from_samples(mask: &[bool], start: usize, frames: usize) -> Vec<bool> {
    (0..frames)
        .map(|i| {
            let (a, b) = ((i * FRAME).saturating_sub(start), ((i + 1) * FRAME).saturating_sub(start));
            if (i + 1) * FRAME <= start || a >= mask.len() {
                return false;
            }
            let part = &mask[a..b.min(mask.len())];
            part.iter().filter(|&&m| m).count() * 2 > FRAME
        })
        .collect()
}

pub fn analyze(silence: &[f32], keyboard: &[f32], speech: &[f32]) -> Analysis {
    let sil = frame_rms_db(silence);
    let sp = frame_rms_db(speech);
    let noise_floor = percentile(&sil, 0.5);

    // Trames « parlées » : activité vocale P.56 croisée avec le détecteur de
    // voix de RNNoise, et au moins 6 dB au-dessus du bruit de fond.
    let (measured, activity) = speech_activity(speech);
    let active: Vec<bool> = frames_from_samples(&activity, 0, sp.len())
        .into_iter()
        .zip(&sp)
        .map(|(a, &l)| a && l > noise_floor + 6.0)
        .collect();
    let active_levels: Vec<f32> = sp.iter().zip(&active).filter(|(_, a)| **a).map(|(l, _)| *l).collect();
    // Niveau de voix active (UIT-T P.56) ; à défaut, médiane des trames actives.
    let speech_level = measured.map_or_else(|| percentile(&active_levels, 0.5), |m| m.level_db);
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
        keyboard_level_db: if keyboard.len() >= FRAME { burst_level(&frame_rms_db(keyboard)) } else { noise_floor },
        keyboard_peak_db: burst_level(&frame_peak_db(keyboard)),
        speech_frame_peak_db: {
            let peaks = frame_peak_db(speech);
            let active_peaks: Vec<f32> = peaks.iter().zip(&active).filter(|(_, a)| **a).map(|(l, _)| *l).collect();
            percentile(&active_peaks, 0.5)
        },
        tonal_noise: detect_tonal_noise(silence),
    }
}

/// Écart voix/bruit minimal (dB) pour considérer que la voix a été entendue.
pub const MIN_SNR_DB: f32 = 6.0;
/// Proportion d'échantillons saturés au-delà de laquelle on prévient l'utilisateur.
pub const CLIPPING_WARNING_RATIO: f32 = 0.0005;

impl Analysis {
    /// Vrai si le micro a saturé pendant la lecture.
    pub fn clipped(&self) -> bool {
        self.clipped_ratio > CLIPPING_WARNING_RATIO
    }
}

/// Sépare l'enregistrement continu de l'assistant (48 kHz) en ses trois phases :
/// silence, frappe au clavier, lecture.
///
/// Une demi-seconde est ignorée au début et à la fin de chaque phase muette
/// (clic de souris, transitions).
pub fn split_recording(all: &[f32]) -> Recording {
    let at = |secs: f32| ((secs * SAMPLE_RATE) as usize).min(all.len());
    let part = |from: f32, to: f32| {
        let (a, b) = (at(from), at(to));
        all[a.min(b)..b].to_vec()
    };
    Recording {
        silence: part(0.5, SILENCE_SECS - 0.5),
        keyboard: part(SILENCE_SECS + 0.5, SILENCE_SECS + KEYBOARD_SECS - 0.5),
        speech: all[at(SILENCE_SECS + KEYBOARD_SECS)..].to_vec(),
    }
}

/// Vérifie qu'un calibrage est exploitable ; sinon, le message à afficher.
pub fn check(result: &CalibrationResult) -> Result<(), &'static str> {
    if result.analysis.snr_db < MIN_SNR_DB {
        Err("Je n'ai presque pas entendu ta voix. Vérifie que le bon micro est choisi et que tu as bien lu le texte.")
    } else {
        Ok(())
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

    // Clavier : si les touches sont nettement moins fortes que la voix, la porte
    // se règle juste au-dessus de leurs crêtes (l'enveloppe de la porte suit les
    // crêtes, pas la moyenne). Sinon, seule la porte vocale peut les distinguer.
    let keys_peak = a.keyboard_peak_db + g;
    let voice_peak = a.speech_frame_peak_db + g;
    let keys_audible = a.keyboard_level_db > a.noise_floor_db + 6.0;
    if keys_audible && keys_peak + 2.0 <= voice_peak - 8.0 {
        p.gate_threshold_db = p.gate_threshold_db.max(keys_peak + 2.0).clamp(-70.0, -20.0);
    }
    p.voice_gate_enabled = keys_audible;

    // Bourdonnement : un coupe-bande étroit sur chaque raie trouvée dans le silence.
    p.notch_freqs_hz = [0.0; MAX_NOTCHES];
    for (slot, peak) in p.notch_freqs_hz.iter_mut().zip(&a.tonal_noise) {
        *slot = peak.freq_hz;
    }

    // Débruitage : toujours actif, mais pas plus fort que nécessaire.
    p.denoise_limit_db = denoise_needed_db(a);

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
        MAX_COMP_RATIO
    };
    p.comp_threshold_db = PRE_COMP_SPEECH_DB - 4.0;
    let expected_gr = 4.0 * (1.0 - 1.0 / p.comp_ratio);
    p.comp_makeup_db = (TARGET_SPEECH_DB - PRE_COMP_SPEECH_DB + expected_gr).clamp(0.0, 12.0);
    p
}

/// Note détaillée d'un rendu, affichée dans le rapport du calibrage.
#[derive(Clone, Debug, Default)]
pub struct Score {
    pub total: f32,
    /// Écart entre la voix et le bruit de fond (dB, plus c'est haut mieux c'est).
    pub noise_reduction_db: f32,
    /// Écart entre la voix et les crêtes du clavier (dB, plus c'est haut mieux c'est).
    pub keyboard_reduction_db: f32,
    /// Sonie intégrée de la voix (LUFS).
    pub loudness_lufs: f32,
    pub level_spread_db: f32,
    pub loudness_error_db: f32,
    pub words_cut_ratio: f32,
    pub clipped_ratio: f32,
    pub tonal_error_db: f32,
    /// Qualité perçue DNSMOS P.835 de la voix (1 à 5), calculée seulement
    /// pour la référence et les meilleurs réglages (calcul lourd).
    pub mos: Option<Mos>,
}

/// Où se trouvent les phases dans l'enregistrement complet.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Trames (de [`FRAME`] échantillons) de la phase clavier.
    pub keyboard_frames: Range<usize>,
    /// Échantillons de la phase de lecture.
    pub speech_samples: Range<usize>,
    /// Réduction de bruit suffisante pour ce micro (dB), voir [`denoise_needed_db`].
    pub denoise_needed_db: f32,
}

impl Layout {
    fn of(rec: &Recording) -> Self {
        let kb_start = rec.silence.len();
        let sp_start = kb_start + rec.keyboard.len();
        Self {
            keyboard_frames: kb_start.div_ceil(FRAME)..sp_start / FRAME,
            speech_samples: sp_start..sp_start + rec.speech.len(),
            denoise_needed_db: 0.0,
        }
    }
}

/// Bruit résiduel visé sous la voix (dB) : au-delà, il n'est plus audible
/// sous la parole (la porte de bruit s'occupe des silences).
const DENOISE_TARGET_SNR_DB: f32 = 50.0;
/// Bornes de la limite du débruiteur cherchée par le calibrage (dB).
pub const DENOISE_MIN_DB: f32 = 12.0;
pub const DENOISE_MAX_DB: f32 = 60.0;
/// Coût (points de score) de chaque dB de débruitage au-delà du nécessaire :
/// risque de son « métallique » que le score ne mesure pas directement.
const OVERSUPPRESSION_WEIGHT: f32 = 0.3;

/// Réduction de bruit suffisante pour amener le bruit de ce micro à
/// `DENOISE_TARGET_SNR_DB` sous la voix, plus 6 dB de marge, bornée.
pub fn denoise_needed_db(a: &Analysis) -> f32 {
    let snr = if a.snr_db.is_finite() { a.snr_db } else { 0.0 };
    (DENOISE_TARGET_SNR_DB - snr + 6.0).clamp(DENOISE_MIN_DB, DENOISE_MAX_DB)
}

/// Pénalité d'un débruitage plus fort que nécessaire (« sans limite » compte
/// pour `DENOISE_MAX_DB`).
pub fn oversuppression_penalty(limit_db: f32, needed_db: f32) -> f32 {
    OVERSUPPRESSION_WEIGHT * (limit_db.min(DENOISE_MAX_DB) - needed_db).max(0.0)
}

/// Écart maximal compté pour le bruit (au-delà, l'oreille ne fait plus la différence).
const MAX_REDUCTION_DB: f32 = 60.0;

/// Note un rendu : plus c'est haut, mieux c'est.
/// `speech_mask` indique, trame par trame, où il y avait de la voix dans l'enregistrement d'origine.
pub fn score(output: &[f32], speech_mask: &[bool], layout: &Layout) -> Score {
    let levels = frame_rms_db(output);
    let n = levels.len().min(speech_mask.len());
    let sp: Vec<f32> = (0..n).filter(|&i| speech_mask[i]).map(|i| levels[i]).collect();
    let quiet: Vec<f32> = (0..n).filter(|&i| !speech_mask[i]).map(|i| levels[i]).collect();
    let speech_level = percentile(&sp, 0.5);
    let noise_level = percentile(&quiet, 0.5).max(-100.0);

    // Clavier : niveau des touches comparé au niveau de la voix.
    let kb: Vec<f32> = layout.keyboard_frames.clone().filter(|&i| i < n).map(|i| levels[i]).collect();
    let keyboard =
        if kb.is_empty() { MAX_REDUCTION_DB } else { (speech_level - burst_level(&kb)).min(MAX_REDUCTION_DB) };

    // Mots coupés : trames de voix rendues plus de 15 dB sous le niveau de voix médian.
    let cut = sp.iter().filter(|&&l| l < speech_level - 15.0).count() as f32 / sp.len().max(1) as f32;
    let spread = percentile(&sp, 0.9) - percentile(&sp, 0.1);
    let speech_part = output.get(layout.speech_samples.clone()).unwrap_or(output);
    let lufs = integrated_lufs(speech_part);
    let loud_err = (lufs - TARGET_LUFS).abs();
    let clipped = output.iter().filter(|s| s.abs() > 0.995).count() as f32 / output.len().max(1) as f32;

    let spec = average_spectrum(output, |i| speech_mask.get(i).copied().unwrap_or(false));
    let body = band_db(&spec, 200.0, 500.0);
    let tonal = ((band_db(&spec, 500.0, 1500.0) - body - TARGET_BOXY).abs()
        + (band_db(&spec, 2000.0, 5000.0) - body - TARGET_PRESENCE).abs()
        + (band_db(&spec, 9000.0, 14000.0) - body - TARGET_AIR).abs())
        / 3.0;

    let nr = (speech_level - noise_level).min(MAX_REDUCTION_DB);
    let total = nr * 1.0 + keyboard * 0.7
        - spread * 1.5
        - loud_err * 1.0
        - cut * WORDS_CUT_WEIGHT
        - clipped * 5000.0
        - tonal * 0.5;
    Score {
        total,
        noise_reduction_db: nr,
        keyboard_reduction_db: keyboard,
        loudness_lufs: lufs,
        level_spread_db: spread,
        loudness_error_db: loud_err,
        words_cut_ratio: cut,
        clipped_ratio: clipped,
        tonal_error_db: tonal,
        mos: None,
    }
}

/// Pénalité par mot coupé (en proportion) : 1 % de mots coupés coûte 3
/// points, autant que 3 dB de bruit de fond en plus.
const WORDS_CUT_WEIGHT: f32 = 300.0;

/// Résultat complet d'un calibrage.
#[derive(Clone, Debug)]
pub struct CalibrationResult {
    pub analysis: Analysis,
    pub params: Params,
    pub score: Score,
    pub baseline_score: Score,
    pub candidates_tested: usize,
    /// Les meilleurs réglages (le premier est `params`), proposés à l'écoute
    /// à l'aveugle.
    pub finalists: Vec<(Params, Score)>,
}

/// Variantes essayées autour des réglages proposés.
fn candidates(base: &Params) -> Vec<Params> {
    let mut out = Vec::new();
    for gate_off in [-4.0, 0.0, 3.0] {
        for ratio_mul in [0.75, 1.0, 1.3] {
            for eq_scale in [0.5, 1.0] {
                // Porte vocale : coupée, tolérante, stricte.
                for voice in [None, Some(0.5), Some(0.7)] {
                    let mut p = base.clone();
                    p.gate_threshold_db += gate_off;
                    p.comp_ratio = (p.comp_ratio * ratio_mul).clamp(1.5, MAX_COMP_RATIO);
                    for b in &mut p.eq_bands {
                        b.gain_db *= eq_scale;
                    }
                    p.voice_gate_enabled = voice.is_some();
                    p.voice_gate_threshold = voice.unwrap_or(p.voice_gate_threshold);
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Lance tout le calibrage : analyse, proposition, puis tournoi des variantes.
/// `progress` reçoit une valeur entre 0 et 1.
pub fn run(rec: &Recording, mut progress: impl FnMut(f32)) -> CalibrationResult {
    let analysis = analyze(&rec.silence, &rec.keyboard, &rec.speech);
    let base = propose(&analysis);

    // Enregistrement complet : silence, clavier puis voix, avec le masque « voix »
    // d'origine (limité à la phase de lecture : un clic n'est pas de la voix).
    let mut full = Vec::with_capacity(rec.silence.len() + rec.keyboard.len() + rec.speech.len());
    full.extend_from_slice(&rec.silence);
    full.extend_from_slice(&rec.keyboard);
    full.extend_from_slice(&rec.speech);
    let mut layout = Layout::of(rec);
    layout.denoise_needed_db = denoise_needed_db(&analysis);
    let first_speech_frame = layout.speech_samples.start.div_ceil(FRAME);
    let gate_level = analysis.noise_floor_db + 6.0;
    let activity = speech_activity(&rec.speech).1;
    let levels = frame_rms_db(&full);
    let mask: Vec<bool> = frames_from_samples(&activity, layout.speech_samples.start, levels.len())
        .into_iter()
        .zip(&levels)
        .enumerate()
        .map(|(i, (a, &l))| a && i >= first_speech_frame && l > gate_level)
        .collect();

    let baseline = Params { bypass: true, input_gain_db: base.input_gain_db, ..Params::default() };
    let baseline_out = Chain::render(&baseline, &full).0;
    let mut baseline_score = score(&baseline_out, &mask, &layout);

    // Manche 1 : toutes les variantes avec RNNoise (rapide). Manche 2 : les
    // meilleures repassent avec DeepFilterNet (3 fois plus lent). Manche 3 :
    // l'oreille artificielle DNSMOS départage les meilleures de toutes.
    let candidates = candidates(&base);
    let net = Dnsmos::new().ok();
    let rated = if net.is_some() { MOS_FINALISTS + 1 } else { 0 };
    let total = candidates.len() + FINALISTS + REFINE_ROUNDS * REFINE_MOVES + rated;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut results = evaluate_all(&candidates, &full, &mask, &layout, &done, total, &mut progress);
    results.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));
    let finalists: Vec<Params> = results
        .iter()
        .take(FINALISTS)
        .map(|(p, _)| Params { denoise_engine: DenoiseEngine::DeepFilter, ..p.clone() })
        .collect();
    results.extend(evaluate_all(&finalists, &full, &mask, &layout, &done, total, &mut progress));
    results.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));

    // Manche d'affinage : on part du meilleur et on retouche chaque réglage
    // fin, un à la fois, dans les deux sens ; le pas diminue de moitié quand
    // plus rien ne s'améliore (descente par coordonnées).
    let mut refined = 0;
    if let Some(best) = results.first().cloned() {
        let mut current = best;
        let mut step = 1.0;
        for _ in 0..REFINE_ROUNDS {
            let tries = refinements(&current.0, step);
            refined += tries.len();
            let tried = evaluate_all(&tries, &full, &mask, &layout, &done, total, &mut progress);
            let round_best = tried.iter().max_by(|a, b| a.1.total.total_cmp(&b.1.total)).cloned();
            results.extend(tried);
            match round_best {
                Some(b) if b.1.total > current.1.total => current = b,
                _ => step *= 0.5,
            }
        }
        results.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));
    }

    if let Some(net) = &net {
        let speech = layout.speech_samples.clone();
        let mut step = |audio: &[f32]| {
            let m = net.rate(audio.get(speech.clone()).unwrap_or(audio), MOS_CLIPS);
            progress(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as f32 / total as f32);
            m
        };
        baseline_score.mos = Some(step(&baseline_out));
        results.truncate(MOS_FINALISTS);
        for (p, s) in &mut results {
            let m = step(&render_aligned(p, &full));
            s.mos = Some(m);
            s.total += MOS_BAK_WEIGHT * m.bak;
        }
    }
    progress(1.0);
    results.truncate(MOS_FINALISTS);
    results.sort_by(|a, b| b.1.total.total_cmp(&a.1.total));
    let (params, score) = results.first().cloned().expect("au moins un candidat");
    CalibrationResult {
        analysis,
        params,
        score,
        baseline_score,
        candidates_tested: candidates.len() + FINALISTS + refined,
        finalists: results,
    }
}

/// Tours d'affinage après le tournoi.
const REFINE_ROUNDS: usize = 3;
/// Retouches essayées au plus par tour (voir [`refinements`]).
const REFINE_MOVES: usize = 33;

/// Retouches d'un réglage : chaque paramètre fin, un peu plus haut puis un
/// peu plus bas. `step` (1 au départ) règle l'ampleur des retouches. Le
/// compresseur multibande, l'égaliseur dynamique et l'anti-réverbération sont
/// essayés allumés et éteints ; leurs réglages, comme ceux de la porte vocale, ne sont retouchés
/// que s'ils sont actifs.
pub fn refinements(p: &Params, step: f32) -> Vec<Params> {
    type Move = fn(&mut Params, f32);
    let moves: [Move; 8] = [
        |p, d| p.gate_threshold_db = (p.gate_threshold_db + 2.0 * d).clamp(-80.0, -10.0),
        |p, d| p.gate_release_ms = (p.gate_release_ms * (1.0 + 0.3 * d)).clamp(30.0, 600.0),
        |p, d| {
            p.denoise_limit_db =
                (p.denoise_limit_db.min(DENOISE_MAX_DB) + 6.0 * d).clamp(DENOISE_MIN_DB, DENOISE_MAX_DB);
        },
        |p, d| p.deesser_threshold_db = (p.deesser_threshold_db + 3.0 * d).clamp(-50.0, -5.0),
        |p, d| p.comp_threshold_db = (p.comp_threshold_db + 2.0 * d).clamp(-50.0, -5.0),
        |p, d| p.comp_ratio = (p.comp_ratio * (1.0 + 0.2 * d)).clamp(1.5, MAX_COMP_RATIO),
        |p, d| p.hpf_hz = (p.hpf_hz + 20.0 * d).clamp(50.0, 160.0),
        |p, d| {
            for b in &mut p.eq_bands {
                b.gain_db *= 1.0 + 0.3 * d;
            }
        },
    ];
    let voice: [Move; 2] = [
        |p, d| p.voice_gate_threshold = (p.voice_gate_threshold + 0.08 * d).clamp(0.1, 0.9),
        |p, d| p.voice_gate_hold_ms = (p.voice_gate_hold_ms + 80.0 * d).clamp(100.0, 800.0),
    ];
    let multiband: [Move; 3] = [
        // Essayer d'allumer (ou d'éteindre) le compresseur multibande.
        |p, d| {
            if d > 0.0 {
                p.mb_enabled = !p.mb_enabled;
            }
        },
        |p, d| {
            if p.mb_enabled {
                for t in &mut p.mb_threshold_db {
                    *t = (*t + 3.0 * d).clamp(-50.0, -5.0);
                }
            }
        },
        |p, d| {
            if p.mb_enabled {
                for r in &mut p.mb_ratio {
                    *r = (*r * (1.0 + 0.25 * d)).clamp(1.2, 8.0);
                }
            }
        },
    ];
    let dynamic_eq: [Move; 2] = [
        |p, d| {
            if d > 0.0 {
                p.deq_enabled = !p.deq_enabled;
            }
        },
        |p, d| {
            if p.deq_enabled {
                for b in &mut p.deq_bands {
                    b.threshold_db = (b.threshold_db + 3.0 * d).clamp(-60.0, -5.0);
                }
            }
        },
    ];
    let dereverb: [Move; 3] = [
        |p, d| {
            if d > 0.0 {
                p.dereverb_enabled = !p.dereverb_enabled;
            }
        },
        |p, d| {
            if p.dereverb_enabled {
                p.dereverb_strength = (p.dereverb_strength + 0.2 * d).clamp(0.1, 1.0);
            }
        },
        |p, d| {
            if p.dereverb_enabled {
                p.dereverb_t60_s = (p.dereverb_t60_s * (1.0 + 0.4 * d)).clamp(0.2, 1.5);
            }
        },
    ];
    let active = moves.iter().chain(&multiband).chain(&dynamic_eq).chain(&dereverb).chain(if p.voice_gate_enabled {
        &voice[..]
    } else {
        &[]
    });
    let mut out = Vec::with_capacity(REFINE_MOVES);
    for m in active {
        for d in [step, -step] {
            let mut q = p.clone();
            m(&mut q, d);
            if q != *p {
                out.push(q);
            }
        }
    }
    out
}

/// Meilleurs réglages notés par DNSMOS à la fin du tournoi.
const MOS_FINALISTS: usize = 3;
/// Extraits de 9 s notés par DNSMOS dans la lecture (début et fin).
const MOS_CLIPS: usize = 2;
/// Correction de volume de sortie maximale (dB) appliquée par le calibrage.
/// 12 dB : assez pour un micro USB faible (voix vers -40 dBFS), relevé par
/// le gain d'entrée puis par celui-ci.
const MAX_OUTPUT_GAIN_DB: f32 = 12.0;

/// Poids d'un point de note DNSMOS « bruit de fond » (BAK) dans le départage.
/// Seule la propreté compte : DNSMOS travaille à 16 kHz (il n'entend rien
/// au-dessus de 8 kHz) et sa note de clarté de la voix (SIG) ne suit les
/// auditeurs qu'à 0,71 de corrélation extrait par extrait. La qualité de la
/// voix est départagée à l'oreille (écoute à l'aveugle des finalistes).
const MOS_BAK_WEIGHT: f32 = 10.0;

/// Variantes de la manche 1 qui repassent avec DeepFilterNet.
const FINALISTS: usize = 6;

/// Note des variantes en parallèle sur tous les cœurs du processeur.
/// `done` compte les variantes notées, sur `total` pour toute la séance.
fn evaluate_all(
    candidates: &[Params],
    full: &[f32],
    mask: &[bool],
    layout: &Layout,
    done: &std::sync::atomic::AtomicUsize,
    total: usize,
    progress: &mut impl FnMut(f32),
) -> Vec<(Params, Score)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism().map_or(2, std::num::NonZero::get).min(candidates.len());
    let per = candidates.len().div_ceil(workers);
    let mut results: Vec<(Params, Score)> = Vec::with_capacity(candidates.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = candidates
            .chunks(per)
            .map(|chunk| {
                let (full, mask, layout, done) = (full, mask, layout, done);
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|p| {
                            let r = evaluate(p.clone(), full, mask, layout);
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
    results
}

/// Ajuste le gain de sortie pour viser la bonne sonie, puis note le rendu.
///
/// Le rendu est réaligné sur l'original (retard du débruiteur et de la porte
/// vocale) : sinon le début de chaque mot compterait comme « coupé » et les
/// moteurs les plus lents seraient injustement pénalisés.
///
/// La correction s'ajoute au gain de sortie déjà présent : les variantes de
/// l'affinage héritent du gain du meilleur réglage (bogue constaté sur le
/// premier calibrage réel : le gain était remplacé au lieu d'être corrigé, et
/// la voix finissait à -21,9 LUFS au lieu de -18).
fn evaluate(mut p: Params, full: &[f32], mask: &[bool], layout: &Layout) -> (Params, Score) {
    let out = render_aligned(&p, full);
    let lufs = integrated_lufs(out.get(layout.speech_samples.clone()).unwrap_or(&out));
    let correction =
        if lufs.is_finite() && lufs > crate::dsp::loudness::SILENT_LUFS { TARGET_LUFS - lufs } else { 0.0 };
    p.output_gain_db = (p.output_gain_db + correction).clamp(-MAX_OUTPUT_GAIN_DB, MAX_OUTPUT_GAIN_DB);
    let mut s = score(&render_aligned(&p, full), mask, layout);
    if p.denoise_enabled {
        s.total -= oversuppression_penalty(p.denoise_limit_db, layout.denoise_needed_db);
    }
    (p, s)
}

/// Rendu hors temps réel, recalé sur l'original (sans le retard de la chaîne).
fn render_aligned(p: &Params, full: &[f32]) -> Vec<f32> {
    let (mut out, _) = Chain::render(p, full);
    let lag = Chain::latency_samples(p).min(out.len());
    out.drain(..lag);
    out.resize(full.len(), 0.0);
    out
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

/// Frappe au clavier synthétique : un clic bref (5 ms de bruit qui décroît)
/// toutes les 150 ms, au niveau de crête `keys_db`, sur le bruit de fond.
pub fn synthetic_keyboard(len: usize, noise_db: f32, keys_db: f32) -> Vec<f32> {
    let mut seed = 777u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let (nl, kl) = (db_to_lin(noise_db) * 1.7, db_to_lin(keys_db));
    let period = (0.15 * SAMPLE_RATE) as usize;
    let click = (0.005 * SAMPLE_RATE) as usize;
    (0..len)
        .map(|i| {
            let k = i % period;
            let c = if k < click { noise() * kl * (1.0 - k as f32 / click as f32) } else { 0.0 };
            c + noise() * nl
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE).sin()).collect()
    }

    #[test]
    fn frame_mask_follows_the_sample_mask_with_an_offset() {
        // Voix active sur les échantillons 480..1440 d'une partie qui commence
        // à l'échantillon 960 de l'enregistrement complet.
        let mut m = vec![false; 2_400];
        m[480..1_440].fill(true);
        let f = frames_from_samples(&m, 960, 8);
        assert_eq!(f, [false, false, false, true, true, false, false, false]);
        assert_eq!(frames_from_samples(&[], 0, 2), [false, false]);
    }

    #[test]
    fn speech_activity_finds_the_voice_and_its_level() {
        let (silence, speech) = synthetic_voice(4.0, -60.0, -25.0);
        let x: Vec<f32> = [&silence[..], &speech[..]].concat();
        let (m, mask) = speech_activity(&x);
        let m = m.expect("voix");
        // Le silence du début n'est pas de la voix ; la lecture, si.
        let quiet = mask[..silence.len() - 9_600].iter().filter(|&&a| a).count();
        let voiced = mask[silence.len()..].iter().filter(|&&a| a).count();
        assert_eq!(quiet, 0);
        assert!(voiced as f32 > 0.6 * speech.len() as f32, "{voiced}");
        // Niveau des syllabes (2/3 du temps) : le maintien de 200 ms compte
        // aussi les pauses de 100 ms entre elles, d'où au plus 1,8 dB de moins.
        let e: f64 = speech.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() * 1.5 / speech.len() as f64;
        let syllables = (10.0 * e.log10()) as f32;
        assert!((syllables - m.level_db) > -0.5 && (syllables - m.level_db) < 2.5, "{syllables} / {m:?}");
        assert!(speech_activity(&[]).0.is_none());
    }

    #[test]
    fn denoise_is_limited_to_what_the_microphone_needs() {
        let noisy = Analysis { snr_db: 20.0, ..Analysis::default() };
        let clean = Analysis { snr_db: 55.0, ..Analysis::default() };
        assert!((denoise_needed_db(&noisy) - 36.0).abs() < 1e-4, "50 - 20 + 6 dB de marge");
        assert!((denoise_needed_db(&clean) - DENOISE_MIN_DB).abs() < f32::EPSILON, "jamais moins de 12 dB");
        let terrible = Analysis { snr_db: -10.0, ..Analysis::default() };
        assert!((denoise_needed_db(&terrible) - DENOISE_MAX_DB).abs() < f32::EPSILON);
        let broken = Analysis { snr_db: f32::NAN, ..Analysis::default() };
        assert!(denoise_needed_db(&broken).is_finite());
        assert!((propose(&noisy).denoise_limit_db - 36.0).abs() < 1e-4);
    }

    #[test]
    fn only_extra_denoising_is_penalized() {
        assert!(oversuppression_penalty(20.0, 30.0).abs() < f32::EPSILON, "moins que nécessaire : rien");
        assert!((oversuppression_penalty(40.0, 30.0) - 3.0).abs() < 1e-5, "0,3 point par dB en trop");
        assert!(
            (oversuppression_penalty(100.0, 30.0) - oversuppression_penalty(DENOISE_MAX_DB, 30.0)).abs() < f32::EPSILON,
            "« sans limite » compte pour 60 dB"
        );
    }

    #[test]
    fn output_gain_is_corrected_not_replaced() {
        let (silence, speech) = synthetic_voice(3.0, -60.0, -30.0);
        let rec = Recording { keyboard: silence.clone(), silence, speech };
        let layout = Layout::of(&rec);
        let full: Vec<f32> = [&rec.silence[..], &rec.keyboard[..], &rec.speech[..]].concat();
        let mask = vec![true; full.len() / FRAME];
        let base = propose(&analyze(&rec.silence, &rec.keyboard, &rec.speech));
        let (first, s1) = evaluate(base, &full, &mask, &layout);
        // Une variante héritée du meilleur réglage garde un volume correct.
        let (second, s2) = evaluate(first.clone(), &full, &mask, &layout);
        assert!((s1.loudness_lufs - TARGET_LUFS).abs() < 1.5, "{}", s1.loudness_lufs);
        assert!((s2.loudness_lufs - TARGET_LUFS).abs() < 1.5, "{}", s2.loudness_lufs);
        assert!((second.output_gain_db - first.output_gain_db).abs() < 1.5);
    }

    #[test]
    fn percentile_handles_edges() {
        assert!((percentile(&[], 0.5) + 120.0).abs() < f32::EPSILON);
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert!((percentile(&v, 0.0) - 1.0).abs() < f32::EPSILON);
        assert!((percentile(&v, 0.5) - 3.0).abs() < f32::EPSILON);
        assert!((percentile(&v, 1.0) - 5.0).abs() < f32::EPSILON);
        assert!((percentile(&v, 7.0) - 5.0).abs() < f32::EPSILON, "p borné à [0, 1]");
        // Régression : un NaN faisait paniquer le tri (`partial_cmp().unwrap()`).
        let _ = percentile(&[1.0, f32::NAN, 2.0], 0.5);
    }

    #[test]
    fn frame_rms_ignores_incomplete_frames() {
        let x = vec![0.5f32; FRAME * 2 + 10];
        let l = frame_rms_db(&x);
        assert_eq!(l.len(), 2);
        assert!(l.iter().all(|db| (db + 6.02).abs() < 0.01));
    }

    #[test]
    fn spectrum_finds_the_tone() {
        let x = tone(3000.0, 0.5, 48_000);
        let spec = average_spectrum(&x, |_| true);
        assert_eq!(spec.len(), 1024);
        assert!((peak_freq(&spec, 1000.0, 8000.0) - 3000.0).abs() < 30.0);
        assert!(band_db(&spec, 2500.0, 3500.0) > band_db(&spec, 500.0, 1500.0) + 40.0);
        let none = average_spectrum(&x, |_| false);
        assert!(none.iter().all(|&e| e == 0.0), "aucune trame retenue : spectre nul");
    }

    /// Enregistrement synthétique complet : silence, clavier (si `keys_db`), voix.
    fn recording(secs: f32, noise_db: f32, voice_db: f32, keys_db: Option<f32>) -> Recording {
        let (silence, speech) = synthetic_voice(secs, noise_db, voice_db);
        let keyboard = keys_db.map_or_else(|| silence.clone(), |k| synthetic_keyboard(silence.len(), noise_db, k));
        Recording { silence, keyboard, speech }
    }

    #[test]
    fn split_recording_drops_click_and_transition() {
        let rate = SAMPLE_RATE as usize;
        let (s, k) = (SILENCE_SECS as usize, KEYBOARD_SECS as usize);
        let all: Vec<f32> = (0..rate * 30).map(|i| i as f32).collect();
        let r = split_recording(&all);
        assert_eq!(r.silence.len(), (s - 1) * rate);
        assert!((r.silence[0] - (rate / 2) as f32).abs() < f32::EPSILON);
        assert_eq!(r.keyboard.len(), (k - 1) * rate);
        assert!((r.keyboard[0] - (s * rate + rate / 2) as f32).abs() < f32::EPSILON);
        assert!((r.speech[0] - ((s + k) * rate) as f32).abs() < f32::EPSILON);
        assert_eq!(r.speech.len(), all.len() - (s + k) * rate);
        // Enregistrement trop court (bouton cliqué trop tôt) : pas de panique.
        for n in [0, 10, rate, rate * 8, rate * 12] {
            let r = split_recording(&all[..n]);
            assert!(r.silence.len() + r.keyboard.len() + r.speech.len() <= n);
        }
    }

    #[test]
    fn check_rejects_inaudible_voice() {
        let mut r = run(&recording(3.0, -60.0, -30.0, None), |_| {});
        assert!(check(&r).is_ok());
        r.analysis.snr_db = MIN_SNR_DB - 1.0;
        assert!(check(&r).is_err());
    }

    #[test]
    fn clipping_is_detected() {
        let (sil, _) = synthetic_voice(2.0, -60.0, -30.0);
        let a = analyze(&sil, &sil, &tone(200.0, 1.5, 48_000).iter().map(|x| x.clamp(-1.0, 1.0)).collect::<Vec<_>>());
        assert!(a.clipped());
        let b = analyze(&sil, &sil, &tone(200.0, 0.3, 48_000));
        assert!(!b.clipped());
    }

    #[test]
    fn propose_raises_highpass_for_rumbling_rooms() {
        let quiet = Analysis { low_noise_excess_db: 2.0, ..Analysis::default() };
        let rumble = Analysis { low_noise_excess_db: 15.0, ..Analysis::default() };
        assert!((propose(&quiet).hpf_hz - 90.0).abs() < f32::EPSILON);
        assert!((propose(&rumble).hpf_hz - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn propose_scales_compression_with_dynamics() {
        let ratio = |d| propose(&Analysis { dynamic_range_db: d, ..Analysis::default() }).comp_ratio;
        assert!(ratio(5.0) < ratio(10.0) && ratio(10.0) < ratio(20.0));
        // Jamais au-delà de 4:1, même dans le tournoi et l'affinage.
        let base = propose(&Analysis { dynamic_range_db: 30.0, ..Analysis::default() });
        assert!(candidates(&base).iter().all(|p| p.comp_ratio <= MAX_COMP_RATIO));
        assert!(refinements(&base, 1.0).iter().all(|p| p.comp_ratio <= MAX_COMP_RATIO));
    }

    #[test]
    fn propose_keeps_values_in_safe_ranges() {
        for (noise, speech) in [(-90.0, -60.0), (-60.0, -20.0), (-30.0, -3.0), (-10.0, -5.0)] {
            let a = Analysis {
                noise_floor_db: noise,
                speech_level_db: speech,
                speech_peak_db: speech + 12.0,
                snr_db: speech - noise,
                ..Analysis::default()
            };
            let p = propose(&a);
            assert!((-12.0..=30.0).contains(&p.input_gain_db));
            assert!((-70.0..=-20.0).contains(&p.gate_threshold_db));
            assert!((0.0..=12.0).contains(&p.comp_makeup_db));
            assert!(p.eq_bands.iter().all(|b| b.gain_db.abs() <= 6.0));
        }
    }

    #[test]
    fn score_prefers_clean_and_steady_output() {
        let mask: Vec<bool> = (0..200).map(|i| i % 3 != 0).collect();
        let render = |noise: f32, voice: f32| -> Vec<f32> {
            mask.iter()
                .flat_map(|&m| {
                    let amp = if m { voice } else { noise };
                    tone(1000.0, amp, FRAME)
                })
                .collect()
        };
        let layout = Layout::default();
        let clean = score(&render(0.0001, 0.18), &mask, &layout);
        let noisy = score(&render(0.05, 0.18), &mask, &layout);
        assert!(clean.noise_reduction_db > noisy.noise_reduction_db);
        assert!(clean.total > noisy.total);
    }

    #[test]
    fn score_penalises_keyboard_leaks() {
        // 100 trames de « clavier » puis 200 trames de voix et de pauses.
        let mask: Vec<bool> = (0..300).map(|i| i >= 100 && i % 3 != 0).collect();
        let layout =
            Layout { keyboard_frames: 0..100, speech_samples: 100 * FRAME..300 * FRAME, denoise_needed_db: 0.0 };
        let render = |keys: f32| -> Vec<f32> {
            (0..300)
                .flat_map(|i| {
                    let amp = if i < 100 {
                        if i % 10 == 0 { keys } else { 0.0001 }
                    } else if mask[i] {
                        0.18
                    } else {
                        0.0001
                    };
                    tone(1000.0, amp, FRAME)
                })
                .collect()
        };
        let quiet = score(&render(0.0001), &mask, &layout);
        let loud = score(&render(0.1), &mask, &layout);
        assert!(quiet.keyboard_reduction_db > loud.keyboard_reduction_db + 30.0, "{quiet:?} / {loud:?}");
        assert!(quiet.total > loud.total);
        assert!(quiet.loudness_lufs > -40.0 && quiet.loudness_lufs < 0.0, "{}", quiet.loudness_lufs);
    }

    #[test]
    fn hum_and_its_harmonics_are_found_in_the_silence() {
        let (mut sil, _) = synthetic_voice(16.0, -60.0, -30.0);
        for (i, x) in sil.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE;
            *x += 0.01 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                + 0.005 * (2.0 * std::f32::consts::PI * 250.0 * t).sin();
        }
        let peaks = detect_tonal_noise(&sil);
        assert_eq!(peaks.len(), 2, "{peaks:?}");
        assert!((peaks[0].freq_hz - 150.0).abs() < 1.0, "{peaks:?}");
        assert!((peaks[1].freq_hz - 250.0).abs() < 1.0, "{peaks:?}");
        assert!(peaks[0].prominence_db > peaks[1].prominence_db);
        let p = propose(&Analysis { tonal_noise: peaks, ..Analysis::default() });
        assert!((p.notch_freqs_hz[0] - 150.0).abs() < 1.0 && p.notch_freqs_hz[2].abs() < f32::EPSILON);
    }

    #[test]
    fn plain_noise_has_no_hum_and_short_silences_are_ignored() {
        let (sil, _) = synthetic_voice(16.0, -60.0, -30.0);
        assert_eq!(detect_tonal_noise(&sil), Vec::new());
        assert_eq!(detect_tonal_noise(&sil[..1000]), Vec::new());
        // Beaucoup de raies : on garde les plus marquées, dans la limite des filtres.
        let mut many = sil.clone();
        for (i, x) in many.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE;
            *x += (1..=10)
                .map(|h| 0.01 / h as f32 * (2.0 * std::f32::consts::PI * 100.0 * h as f32 * t).sin())
                .sum::<f32>();
        }
        let peaks = detect_tonal_noise(&many);
        assert_eq!(peaks.len(), MAX_NOTCHES);
        assert!((peaks[0].freq_hz - 100.0).abs() < 1.0, "{peaks:?}");
    }

    #[test]
    fn quiet_keys_raise_the_gate_and_loud_keys_need_the_voice_gate() {
        let base = Analysis {
            noise_floor_db: -70.0,
            speech_level_db: -30.0,
            speech_peak_db: -15.0,
            speech_frame_peak_db: -22.0,
            keyboard_level_db: -70.0,
            keyboard_peak_db: -65.0,
            ..Analysis::default()
        };
        let silent = propose(&base);
        assert!(!silent.voice_gate_enabled, "clavier inaudible : pas de porte vocale");
        let quiet_keys = propose(&Analysis { keyboard_level_db: -55.0, keyboard_peak_db: -38.0, ..base.clone() });
        assert!(quiet_keys.voice_gate_enabled);
        assert!(
            quiet_keys.gate_threshold_db >= -38.0 + quiet_keys.input_gain_db + 2.0 - 0.01,
            "porte au-dessus des touches : {} (gain {})",
            quiet_keys.gate_threshold_db,
            quiet_keys.input_gain_db
        );
        let loud_keys = propose(&Analysis { keyboard_level_db: -40.0, keyboard_peak_db: -20.0, ..base });
        assert!(loud_keys.voice_gate_enabled);
        assert!(loud_keys.gate_threshold_db < quiet_keys.gate_threshold_db, "la porte ne coupe pas la voix");
    }

    #[test]
    fn keyboard_phase_is_measured_and_never_counted_as_voice() {
        let rec = recording(4.0, -60.0, -30.0, Some(-20.0));
        let a = analyze(&rec.silence, &rec.keyboard, &rec.speech);
        assert!(a.keyboard_peak_db > a.noise_floor_db + 20.0, "{a:?}");
        let r = run(&rec, |_| {});
        assert!(
            r.score.keyboard_reduction_db > r.baseline_score.keyboard_reduction_db,
            "{:?} / {:?}",
            r.score,
            r.baseline_score
        );
        assert!(r.baseline_score.keyboard_reduction_db < MAX_REDUCTION_DB, "sans traitement, les touches s'entendent");
        assert!(r.score.words_cut_ratio < 0.2, "{:?}", r.score);
    }

    #[test]
    fn calibration_on_synthetic_voice_is_sane() {
        let rec = recording(6.0, -60.0, -30.0, None);
        let a = analyze(&rec.silence, &rec.keyboard, &rec.speech);
        assert!((a.noise_floor_db + 60.0).abs() < 3.0, "bruit {}", a.noise_floor_db);
        assert!(a.snr_db > 20.0);
        let r = run(&rec, |_| {});
        assert!(r.params.input_gain_db > 0.0, "voix faible : il faut monter le gain");
        assert!(r.params.gate_threshold_db > a.noise_floor_db + r.params.input_gain_db);
        assert!(r.score.words_cut_ratio < 0.1, "{:?}", r.score);
        assert!(r.score.total > r.baseline_score.total, "{:?} vs {:?}", r.score, r.baseline_score);
        assert!(r.score.loudness_error_db < r.baseline_score.loudness_error_db + 1.0, "{:?}", r.score);
    }
}
