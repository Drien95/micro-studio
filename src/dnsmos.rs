//! Note de qualité perçue DNSMOS P.835 (Reddy et al., ICASSP 2022).
//!
//! Un réseau de neurones entraîné par Microsoft sur des milliers de notes
//! d'auditeurs prédit, sans enregistrement de référence, les trois notes de
//! la recommandation ITU-T P.835, de 1 (mauvais) à 5 (excellent) :
//! - **SIG** : qualité de la voix elle-même (distorsion, voix « robotique ») ;
//! - **BAK** : discrétion du bruit de fond ;
//! - **OVRL** : impression d'ensemble.
//!
//! C'est la mesure de référence des concours de débruitage (DNS Challenge).
//! Les poids (`models/DNSMOS/`) sont ceux de Microsoft ; le code qui les
//! exécute est le nôtre et reproduit `dnsmos_local.py` :
//! extraits de 9,01 s à 16 kHz pris toutes les secondes, spectre de
//! puissance logarithmique (noyaux de STFT appris), 7 convolutions 3×3,
//! 3 sous-échantillonnages, maximum global, 3 couches denses, puis
//! correction polynomiale vers l'échelle des auditeurs.
//!
//! Le calcul est lourd (environ 11 milliards de multiplications par extrait) :
//! il est réservé au calibrage, jamais au temps réel. Les convolutions se
//! répartissent sur tous les cœurs.

use crate::dsp::dfn::LoadError;
use crate::dsp::dfn::net::{Weights, dot, take};
use crate::dsp::dfn::onnx;

/// Poids officiels embarqués dans l'exécutable.
const MODEL: &[u8] = include_bytes!("../models/DNSMOS/sig_bak_ovr.onnx");

/// Fréquence d'échantillonnage attendue par le modèle.
pub const SAMPLE_RATE: usize = 16_000;
/// Longueur d'un extrait : 9,01 s à 16 kHz.
pub const CLIP: usize = 144_160;
const HOP: usize = 160;
const WIN: usize = 320;
/// Trames du spectre (axe « hauteur » de l'image).
const FRAMES: usize = 900;
/// Raies du spectre (axe « largeur » de l'image).
const BINS: usize = 161;
/// Plancher de puissance avant le logarithme (graphe d'origine).
const POWER_FLOOR: f32 = 1e-12;

/// Corrections polynomiales (non personnalisées) de `dnsmos_local.py`,
/// coefficients du degré 2 au degré 0.
const POLY_SIG: [f32; 3] = [-0.083_972_78, 1.220_839_5, 0.005_243_9];
const POLY_BAK: [f32; 3] = [-0.131_668_88, 1.609_155, -0.396_045_46];
const POLY_OVR: [f32; 3] = [-0.067_662_83, 1.115_464_7, 0.046_025_35];

/// Les trois notes P.835, de 1 à 5.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mos {
    /// Qualité de la voix.
    pub sig: f32,
    /// Discrétion du bruit de fond.
    pub bak: f32,
    /// Note globale.
    pub ovrl: f32,
}

fn poly(c: [f32; 3], x: f32) -> f32 {
    (c[0] * x + c[1]) * x + c[2]
}

impl Mos {
    /// Notes brutes du réseau `[sig, bak, ovr]` → échelle des auditeurs.
    pub fn from_raw(raw: [f32; 3]) -> Self {
        Self { sig: poly(POLY_SIG, raw[0]), bak: poly(POLY_BAK, raw[1]), ovrl: poly(POLY_OVR, raw[2]) }
    }
}

/// Carte de caractéristiques `[canal][hauteur][largeur]`, bordée d'une ligne
/// et d'une colonne de zéros de chaque côté (le « padding » des convolutions).
struct Map {
    c: usize,
    h: usize,
    w: usize,
    data: Vec<f32>,
}

impl Map {
    fn zeros(c: usize, h: usize, w: usize) -> Self {
        Self { c, h, w, data: vec![0.0; c * (h + 2) * (w + 2)] }
    }

    fn stride(&self) -> usize {
        self.w + 2
    }

    fn plane(&self) -> usize {
        (self.h + 2) * (self.w + 2)
    }

    /// Ligne `y` (0..h) du canal `c`, sans la bordure.
    fn row(&self, c: usize, y: usize) -> &[f32] {
        let start = c * self.plane() + (y + 1) * self.stride() + 1;
        &self.data[start..start + self.w]
    }

    /// Sous-échantillonnage 2×2 par le maximum (les bords impairs sont perdus).
    fn max_pool(&self) -> Self {
        let mut out = Self::zeros(self.c, self.h / 2, self.w / 2);
        let (plane, stride) = (out.plane(), out.stride());
        for c in 0..self.c {
            for y in 0..out.h {
                let (a, b) = (self.row(c, 2 * y), self.row(c, 2 * y + 1));
                let dst = &mut out.data[c * plane + (y + 1) * stride + 1..][..out.w];
                for (x, d) in dst.iter_mut().enumerate() {
                    *d = a[2 * x].max(a[2 * x + 1]).max(b[2 * x]).max(b[2 * x + 1]);
                }
            }
        }
        out
    }

    /// Maximum de chaque canal.
    fn global_max(&self) -> Vec<f32> {
        (0..self.c)
            .map(|c| (0..self.h).flat_map(|y| self.row(c, y).iter().copied()).fold(f32::NEG_INFINITY, f32::max))
            .collect()
    }
}

/// Convolution 3×3, pas de 1, bordure de zéros, suivie d'un `ReLU`.
#[derive(Debug)]
struct Conv3 {
    out: usize,
    inp: usize,
    /// `[sortie][entrée][3×3]`.
    w: Vec<f32>,
    b: Vec<f32>,
}

/// `dst += k · src`, vectorisé par le compilateur.
#[inline]
fn axpy(dst: &mut [f32], src: &[f32], k: f32) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += k * s;
    }
}

impl Conv3 {
    fn load(w: &Weights, name: &str, out: usize, inp: usize) -> Result<Self, LoadError> {
        Ok(Self {
            out,
            inp,
            w: take(w, &format!("{name}/kernel:0"), &[out, inp, 3, 3])?,
            b: take(w, &format!("{name}/bias:0"), &[out])?,
        })
    }

    /// Calcule les canaux de sortie `first..` dans `planes` (plans bordés).
    fn channels(&self, x: &Map, first: usize, planes: &mut [f32]) {
        let (w, stride, xplane) = (x.w, x.stride(), x.plane());
        for (i, plane) in planes.chunks_mut(xplane).enumerate() {
            let o = first + i;
            let kernel = &self.w[o * self.inp * 9..(o + 1) * self.inp * 9];
            for y in 0..x.h {
                let row = &mut plane[(y + 1) * stride + 1..][..w];
                row.fill(self.b[o]);
                for (c, k) in kernel.as_chunks::<9>().0.iter().enumerate() {
                    let base = c * xplane + y * stride;
                    for ky in 0..3 {
                        let src = &x.data[base + ky * stride..][..w + 2];
                        axpy(row, &src[..w], k[ky * 3]);
                        axpy(row, &src[1..=w], k[ky * 3 + 1]);
                        axpy(row, &src[2..], k[ky * 3 + 2]);
                    }
                }
                for v in row.iter_mut() {
                    *v = v.max(0.0);
                }
            }
        }
    }

    /// Applique la couche, en répartissant les canaux de sortie sur les cœurs.
    fn forward(&self, x: &Map) -> Map {
        debug_assert_eq!(x.c, self.inp);
        let mut out = Map::zeros(self.out, x.h, x.w);
        let plane = out.plane();
        let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(self.out);
        let per = self.out.div_ceil(workers);
        std::thread::scope(|s| {
            for (i, chunk) in out.data.chunks_mut(per * plane).enumerate() {
                s.spawn(move || self.channels(x, i * per, chunk));
            }
        });
        out
    }
}

/// Couche dense `y = x · W + b` (W rangé `[entrée][sortie]`).
#[derive(Debug)]
struct Dense {
    out: usize,
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Dense {
    fn load(w: &Weights, name: &str, inp: usize, out: usize) -> Result<Self, LoadError> {
        let p = "mos_estimator_logpow";
        Ok(Self {
            out,
            w: take(w, &format!("{p}/{name}/MatMul/ReadVariableOp/resource:0"), &[inp, out])?,
            b: take(w, &format!("{p}/{name}/BiasAdd/ReadVariableOp/resource:0"), &[out])?,
        })
    }

    fn forward(&self, x: &[f32], relu: bool) -> Vec<f32> {
        let mut y = self.b.clone();
        for (xi, row) in x.iter().zip(self.w.chunks_exact(self.out)) {
            axpy(&mut y, row, *xi);
        }
        if relu {
            for v in &mut y {
                *v = v.max(0.0);
            }
        }
        y
    }
}

/// Le réseau DNSMOS P.835 chargé en mémoire.
#[derive(Debug)]
pub struct Dnsmos {
    /// Noyaux de la STFT apprise, `[raie][échantillon]`.
    stft_re: Vec<f32>,
    stft_im: Vec<f32>,
    convs: [Conv3; 7],
    dense: [Dense; 3],
}

impl Dnsmos {
    /// Charge les poids embarqués.
    ///
    /// # Errors
    /// Si les poids embarqués sont illisibles.
    pub fn new() -> Result<Self, LoadError> {
        let w = onnx::weights(MODEL)?;
        let conv = |name: &str, out, inp| Conv3::load(&w, name, out, inp);
        Ok(Self {
            stft_re: take(&w, "time2freq/stft-real/kernel:0", &[BINS, WIN, 1])?,
            stft_im: take(&w, "time2freq/stft-imag/kernel:0", &[BINS, WIN, 1])?,
            convs: [
                conv("conv2d", 128, 1)?,
                conv("conv2d_1", 64, 128)?,
                conv("conv2d_2", 64, 64)?,
                conv("conv2d_3", 32, 64)?,
                conv("conv2d_4", 32, 32)?,
                conv("conv2d_5", 32, 32)?,
                conv("conv2d_6", 64, 32)?,
            ],
            dense: [
                Dense::load(&w, "dense", 64, 128)?,
                Dense::load(&w, "dense_1", 128, 64)?,
                Dense::load(&w, "dense_3", 64, 3)?,
            ],
        })
    }

    /// Spectre de puissance logarithmique : image `[1][trame][raie]`.
    fn log_power(&self, clip: &[f32; CLIP]) -> Map {
        let mut img = Map::zeros(1, FRAMES, BINS);
        let stride = img.stride();
        for t in 0..FRAMES {
            let frame = &clip[t * HOP..t * HOP + WIN];
            let row = &mut img.data[(t + 1) * stride + 1..][..BINS];
            for ((d, kr), ki) in
                row.iter_mut().zip(self.stft_re.as_chunks::<WIN>().0).zip(self.stft_im.as_chunks::<WIN>().0)
            {
                let (re, im) = (dot(frame, kr), dot(frame, ki));
                *d = (re * re + im * im).max(POWER_FLOOR).log10();
            }
        }
        img
    }

    /// Notes brutes `[sig, bak, ovr]` d'un extrait de 9,01 s à 16 kHz.
    pub fn raw(&self, clip: &[f32; CLIP]) -> [f32; 3] {
        let [c0, c1, c2, c3, c4, c5, c6] = &self.convs;
        let mut h = c3.forward(&c2.forward(&c1.forward(&c0.forward(&self.log_power(clip)))));
        h = c4.forward(&h.max_pool()).max_pool();
        h = c5.forward(&h).max_pool();
        let v = c6.forward(&h).global_max();
        let [d0, d1, d2] = &self.dense;
        let y = d2.forward(&d1.forward(&d0.forward(&v, true), true), false);
        [y[0], y[1], y[2]]
    }

    /// Note un signal à 16 kHz comme `dnsmos_local.py` : le signal trop court
    /// est répété jusqu'à 9,01 s, puis les extraits pris toutes les secondes
    /// sont notés et moyennés. Pour borner le calcul, au plus `max_clips`
    /// extraits régulièrement espacés sont retenus.
    pub fn rate_16k(&self, audio: &[f32], max_clips: usize) -> Mos {
        let mut audio = if audio.is_empty() { vec![0.0; CLIP] } else { audio.to_vec() };
        while audio.len() < CLIP {
            audio.extend_from_within(..);
        }
        let hops = (audio.len() / SAMPLE_RATE).saturating_sub(9).max(1);
        let n = max_clips.clamp(1, hops);
        let mut sum = Mos::default();
        for i in 0..n {
            // Extraits répartis entre le premier et le dernier possible.
            let hop = if n > 1 { i * (hops - 1) / (n - 1) } else { 0 };
            let start = (hop * SAMPLE_RATE).min(audio.len() - CLIP);
            let clip = <&[f32; CLIP]>::try_from(&audio[start..start + CLIP]).expect("extrait de 9,01 s");
            let m = Mos::from_raw(self.raw(clip));
            sum.sig += m.sig;
            sum.bak += m.bak;
            sum.ovrl += m.ovrl;
        }
        let n = n as f32;
        Mos { sig: sum.sig / n, bak: sum.bak / n, ovrl: sum.ovrl / n }
    }

    /// Note un signal à 48 kHz (voir [`Self::rate_16k`]).
    pub fn rate(&self, audio_48k: &[f32], max_clips: usize) -> Mos {
        self.rate_16k(&decimate_48k_to_16k(audio_48k), max_clips)
    }
}

/// Longueur du filtre anti-repliement (impaire : retard entier).
const DECIM_TAPS: usize = 191;
/// Fréquence de coupure (Hz) : la bande utile s'arrête à 8 kHz.
const DECIM_CUTOFF_HZ: f64 = 7_500.0;
/// Paramètre de la fenêtre de Kaiser (atténuation ≈ 65 dB au repli).
const KAISER_BETA: f64 = 8.6;

/// Fonction de Bessel modifiée de première espèce, ordre 0 (série).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..50 {
        term *= (x / (2.0 * f64::from(k))).powi(2);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Filtre passe-bas à phase linéaire (sinus cardinal fenêtré par Kaiser),
/// de gain unité en continu.
fn decimation_filter() -> Vec<f32> {
    let m = (DECIM_TAPS - 1) as f64 / 2.0;
    let fc = DECIM_CUTOFF_HZ / 48_000.0;
    let i0b = bessel_i0(KAISER_BETA);
    let h: Vec<f64> = (0..DECIM_TAPS)
        .map(|n| {
            let t = n as f64 - m;
            let sinc = if t == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t)
            };
            let r = t / m;
            sinc * bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0b
        })
        .collect();
    let dc: f64 = h.iter().sum();
    h.iter().map(|v| (v / dc) as f32).collect()
}

/// Passe de 48 kHz à 16 kHz : filtre anti-repliement puis un échantillon sur
/// trois. Sans retard (le filtre est centré) ; les bords sont complétés par
/// des zéros.
pub fn decimate_48k_to_16k(x: &[f32]) -> Vec<f32> {
    let h = decimation_filter();
    let half = DECIM_TAPS / 2;
    (0..x.len().div_ceil(3))
        .map(|n| {
            let center = 3 * n;
            h.iter()
                .enumerate()
                .filter_map(|(k, hk)| (center + k).checked_sub(half).and_then(|i| x.get(i)).map(|s| hk * s))
                .sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signal déterministe : la5 modulé + bruit blanc faible (le même que
    /// celui qui a servi à relever les notes de référence avec onnxruntime).
    fn golden_signal() -> Vec<f32> {
        let mut seed = 12_345u64;
        (0..CLIP)
            .map(|i| {
                seed = (seed * 1_664_525 + 1_013_904_223) % (1 << 32);
                let t = i as f64 / 16_000.0;
                let tone = 0.1
                    * (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                    * (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 3.0 * t).sin());
                (tone + 0.01 * ((seed as f64 / 4_294_967_295.0) * 2.0 - 1.0)) as f32
            })
            .collect()
    }

    #[test]
    fn matches_the_official_model() {
        // Sorties brutes d'onnxruntime 1.23 sur `sig_bak_ovr.onnx`.
        const ORT: [f32; 3] = [1.123_858_6, 1.008_946_5, 0.997_132_5];
        let net = Dnsmos::new().expect("modèle embarqué");
        let x = golden_signal();
        let raw = net.raw(x.as_slice().try_into().expect("9,01 s"));
        for (a, b) in raw.iter().zip(ORT) {
            assert!((a - b).abs() < 1e-3, "{raw:?} / {ORT:?}");
        }
        let m = net.rate_16k(&x, 4);
        assert_eq!(m, Mos::from_raw(raw), "un seul extrait possible");
    }

    #[test]
    fn polynomial_mapping_matches_the_reference_script() {
        let m = Mos::from_raw([3.0, 3.0, 3.0]);
        assert!((m.sig - (-0.083_972_78 * 9.0 + 1.220_839_5 * 3.0 + 0.005_243_9)).abs() < 1e-5);
        assert!((m.bak - 3.2464).abs() < 1e-4, "{}", m.bak);
        assert!((m.ovrl - 2.783_454).abs() < 1e-4, "{}", m.ovrl);
    }

    #[test]
    fn decimation_keeps_the_voice_band_and_rejects_aliases() {
        let tone = |f: f32| -> Vec<f32> {
            (0..48_000).map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin()).collect()
        };
        let rms =
            |x: &[f32]| (x[1000..x.len() - 1000].iter().map(|v| v * v).sum::<f32>() / (x.len() - 2000) as f32).sqrt();
        let pass = decimate_48k_to_16k(&tone(1000.0));
        assert_eq!(pass.len(), 16_000);
        assert!((rms(&pass) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "1 kHz : {}", rms(&pass));
        // Sans filtre, 12 kHz se replierait sur 4 kHz.
        let alias = decimate_48k_to_16k(&tone(12_000.0));
        assert!(20.0 * rms(&alias).log10() < -60.0, "12 kHz : {}", rms(&alias));
        // Pas de retard : le pic d'une impulsion reste en place.
        let mut imp = vec![0.0; 3000];
        imp[1500] = 1.0;
        let d = decimate_48k_to_16k(&imp);
        let peak = d.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
        assert_eq!(peak, Some(500));
        assert_eq!(decimate_48k_to_16k(&[]), Vec::<f32>::new());
    }

    #[test]
    fn background_noise_lowers_the_background_score() {
        let net = Dnsmos::new().expect("modèle embarqué");
        let clean = golden_signal();
        let mut seed = 7u32;
        let noisy: Vec<f32> = clean
            .iter()
            .map(|s| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                s + 0.05 * ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect();
        let (a, b) = (net.rate_16k(&clean, 1), net.rate_16k(&noisy, 1));
        assert!(b.bak < a.bak, "bruit : BAK {} → {}", a.bak, b.bak);
        // Signal court ou vide : répété jusqu'à 9,01 s, sans plantage.
        let short = net.rate_16k(&clean[..20_000], 3);
        assert!(short.ovrl.is_finite() && short.sig.is_finite() && short.bak.is_finite());
        assert!(net.rate_16k(&[], 1).ovrl.is_finite());
    }
}
