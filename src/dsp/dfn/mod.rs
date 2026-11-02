//! Moteur DeepFilterNet3 « maison » : le réseau, la STFT, les
//! caractéristiques et le filtrage profond, écrits pour Micro Studio et
//! **sans aucune allocation après le chargement**, donc utilisable
//! directement dans le callback audio.
//!
//! Seuls les poids viennent du projet DeepFilterNet (voir
//! `models/DeepFilterNet3/README.md`). Le traitement reproduit celui de la
//! bibliothèque officielle `libDF` ; le test `matches_the_official_engine`
//! compare les deux échantillon par échantillon.
//!
//! Chemin d'une trame de 10 ms :
//! 1. STFT (fenêtre de Vorbis de 960 points, pas de 480) ;
//! 2. caractéristiques : énergie de 32 bandes ERB (dB, normalisée) et
//!    spectre complexe normalisé des 96 premières cases (0–4,8 kHz) ;
//! 3. encodeur → rapport signal/bruit local (`lsnr`) ;
//! 4. selon `lsnr` : décodeur ERB (gains par bande, appliqués avec 2 trames
//!    d'anticipation) et décodeur DF (filtre complexe d'ordre 5 sur les
//!    96 cases basses) ;
//! 5. limite d'atténuation, puis STFT inverse.

pub mod net;
pub mod onnx;

use net::{
    Conv, GroupedLinear, Gru, HIDDEN, MissingWeight, Pointwise, UpsampleDw, Weights, relu, sigmoid_inplace, take,
};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

const SR: usize = 48_000;
const FFT: usize = 960;
/// Taille d'une trame (10 ms).
pub const HOP: usize = 480;
const FREQS: usize = FFT / 2 + 1;
const NB_ERB: usize = 32;
const NB_DF: usize = 96;
const DF_ORDER: usize = 5;
const CONV_LOOKAHEAD: usize = 2;
const LOOKAHEAD: usize = 2;
const CH: usize = 64;
const EMB: usize = 512;
/// Lissage des normalisations : exp(-10 ms / 1 s) arrondi comme `libDF`.
const ALPHA: f32 = 0.99;
const MIN_DB_THRESH: f32 = -10.0;
const MAX_DB_ERB_THRESH: f32 = 30.0;
const MAX_DB_DF_THRESH: f32 = 20.0;

/// Retard du moteur (échantillons) : recouvrement de la STFT + anticipation.
pub const LATENCY: usize = FFT - HOP + LOOKAHEAD * HOP;

const ENC: &[u8] = include_bytes!("../../../models/DeepFilterNet3/enc.onnx");
const ERB_DEC: &[u8] = include_bytes!("../../../models/DeepFilterNet3/erb_dec.onnx");
const DF_DEC: &[u8] = include_bytes!("../../../models/DeepFilterNet3/df_dec.onnx");

/// Erreur de chargement du modèle.
#[derive(Debug)]
pub enum LoadError {
    Onnx(onnx::Error),
    Weight(MissingWeight),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Onnx(e) => e.fmt(f),
            Self::Weight(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<onnx::Error> for LoadError {
    fn from(e: onnx::Error) -> Self {
        Self::Onnx(e)
    }
}

impl From<MissingWeight> for LoadError {
    fn from(e: MissingWeight) -> Self {
        Self::Weight(e)
    }
}

fn freq2erb(f: f32) -> f32 {
    9.265 * (f / (24.7 * 9.265)).ln_1p()
}

fn erb2freq(e: f32) -> f32 {
    24.7 * 9.265 * ((e / 9.265).exp() - 1.0)
}

/// Largeur (en cases FFT) de chacune des 32 bandes ERB, comme `libDF`.
fn erb_bands() -> [usize; NB_ERB] {
    let freq_width = SR as f32 / FFT as f32;
    let (lo, hi) = (freq2erb(0.0), freq2erb((SR / 2) as f32));
    let step = (hi - lo) / NB_ERB as f32;
    let mut erb = [0usize; NB_ERB];
    let (mut prev, mut over) = (0i64, 0i64);
    for (i, band) in erb.iter_mut().enumerate() {
        let fb = (erb2freq(lo + (i + 1) as f32 * step) / freq_width).round() as i64;
        let mut n = fb - prev - over;
        if n < 2 {
            over = 2 - n;
            n = 2;
        } else {
            over = 0;
        }
        *band = usize::try_from(n).unwrap_or(2);
        prev = fb;
    }
    erb[NB_ERB - 1] += 1;
    let total: usize = erb.iter().sum();
    erb[NB_ERB - 1] -= total.saturating_sub(FREQS);
    erb
}

/// Encodeur : caractéristiques → (e0..e3, emb, c0, lsnr).
#[derive(Debug)]
struct Encoder {
    erb_conv0: Conv,
    erb_conv1: (Conv, Pointwise),
    erb_conv2: (Conv, Pointwise),
    erb_conv3: (Conv, Pointwise),
    df_conv0: (Conv, Pointwise),
    df_conv1: (Conv, Pointwise),
    df_fc_emb: GroupedLinear,
    lin_in: GroupedLinear,
    gru: Gru,
    lin_out: GroupedLinear,
    lsnr_w: Vec<f32>,
    lsnr_b: f32,
    lsnr_scale: f32,
    lsnr_offset: f32,
}

impl Encoder {
    fn load(w: &Weights) -> Result<Self, MissingWeight> {
        let dw = |name: &str, stride| Conv::load(w, name, None, CH, CH, CH, (1, 3), stride, 1);
        let pw = |a: &str, b: &str| Pointwise::load(w, a, b, CH, CH);
        let scalar = |name: &str| take(w, name, &[]).map(|v| v[0]);
        Ok(Self {
            erb_conv0: Conv::load(w, "onnx::Conv_282", Some("onnx::Conv_283"), CH, 1, 1, (3, 3), 1, 1)?,
            erb_conv1: (dw("erb_conv1.0.weight", 2)?, pw("onnx::Conv_285", "onnx::Conv_286")?),
            erb_conv2: (dw("erb_conv2.0.weight", 2)?, pw("onnx::Conv_288", "onnx::Conv_289")?),
            erb_conv3: (dw("erb_conv3.0.weight", 1)?, pw("onnx::Conv_291", "onnx::Conv_292")?),
            df_conv0: (
                Conv::load(w, "df_conv0.1.weight", None, CH, 2, 2, (3, 3), 1, 1)?,
                pw("onnx::Conv_294", "onnx::Conv_295")?,
            ),
            df_conv1: (dw("df_conv1.0.weight", 2)?, pw("onnx::Conv_297", "onnx::Conv_298")?),
            df_fc_emb: GroupedLinear::load(w, "df_fc_emb.0.weight", 32, 96, 16)?,
            lin_in: GroupedLinear::load(w, "emb_gru.linear_in.0.weight", 16, 32, 16)?,
            gru: Gru::load(
                w,
                "/emb_gru/Constant_3_output_0",
                "/emb_gru/Constant_4_output_0",
                "/emb_gru/Constant_5_output_0",
                HIDDEN,
            )?,
            lin_out: GroupedLinear::load(w, "emb_gru.linear_out.0.weight", 16, 16, 32)?,
            lsnr_w: take(w, "onnx::MatMul_337", &[EMB, 1])?,
            lsnr_b: take(w, "lsnr_fc.0.bias", &[1])?[0],
            lsnr_scale: scalar("/Constant_8_output_0")?,
            lsnr_offset: scalar("/Constant_9_output_0")?,
        })
    }
}

/// Décodeur ERB : gains des 32 bandes.
#[derive(Debug)]
struct ErbDecoder {
    lin_in: GroupedLinear,
    gru1: Gru,
    gru2: Gru,
    lin_out: GroupedLinear,
    conv3p: Conv,
    convt3: (Conv, Pointwise),
    conv2p: Conv,
    convt2: (UpsampleDw, Pointwise),
    conv1p: Conv,
    convt1: (UpsampleDw, Pointwise),
    conv0p: Conv,
    conv0_out: Conv,
}

impl ErbDecoder {
    fn load(w: &Weights) -> Result<Self, MissingWeight> {
        let skip = |a: &str, b: &str| Conv::load(w, a, Some(b), CH, CH, CH, (1, 1), 1, 0);
        let pw = |a: &str, b: &str| Pointwise::load(w, a, b, CH, CH);
        let gru = |a: &str, b: &str, c: &str| Gru::load(w, a, b, c, HIDDEN);
        Ok(Self {
            lin_in: GroupedLinear::load(w, "emb_gru.linear_in.0.weight", 16, 32, 16)?,
            gru1: gru("/emb_gru/Constant_3_output_0", "/emb_gru/Constant_4_output_0", "/emb_gru/Constant_5_output_0")?,
            gru2: gru(
                "/emb_gru/Constant_9_output_0",
                "/emb_gru/Constant_10_output_0",
                "/emb_gru/Constant_11_output_0",
            )?,
            lin_out: GroupedLinear::load(w, "emb_gru.linear_out.0.weight", 16, 16, 32)?,
            conv3p: skip("onnx::Conv_288", "onnx::Conv_289")?,
            convt3: (
                Conv::load(w, "convt3.0.weight", None, CH, CH, CH, (1, 3), 1, 1)?,
                pw("onnx::Conv_291", "onnx::Conv_292")?,
            ),
            conv2p: skip("onnx::Conv_294", "onnx::Conv_295")?,
            convt2: (UpsampleDw::load(w, "convt2.0.weight", CH)?, pw("onnx::Conv_297", "onnx::Conv_298")?),
            conv1p: skip("onnx::Conv_300", "onnx::Conv_301")?,
            convt1: (UpsampleDw::load(w, "convt1.0.weight", CH)?, pw("onnx::Conv_303", "onnx::Conv_304")?),
            conv0p: skip("onnx::Conv_306", "onnx::Conv_307")?,
            conv0_out: Conv::load(w, "onnx::Conv_309", Some("onnx::Conv_310"), 1, CH, 1, (1, 3), 1, 1)?,
        })
    }
}

/// Décodeur DF : coefficients complexes du filtre profond.
#[derive(Debug)]
struct DfDecoder {
    lin_in: GroupedLinear,
    gru1: Gru,
    gru2: Gru,
    skip: GroupedLinear,
    convp: (Conv, Pointwise),
    out: GroupedLinear,
}

impl DfDecoder {
    fn load(w: &Weights) -> Result<Self, MissingWeight> {
        Ok(Self {
            lin_in: GroupedLinear::load(w, "df_gru.linear_in.0.weight", 8, 64, 32)?,
            gru1: Gru::load(w, "onnx::GRU_291", "onnx::GRU_292", "onnx::GRU_293", HIDDEN)?,
            gru2: Gru::load(w, "onnx::GRU_311", "onnx::GRU_312", "onnx::GRU_313", HIDDEN)?,
            skip: GroupedLinear::load(w, "df_skip.weight", 16, 32, 16)?,
            convp: (
                Conv::load(w, "df_convp.1.weight", None, 2 * DF_ORDER, CH, 2, (5, 1), 1, 0)?,
                Pointwise::load(w, "onnx::Conv_268", "onnx::Conv_269", 2 * DF_ORDER, 2 * DF_ORDER)?,
            ),
            out: GroupedLinear::load(w, "df_out.0.weight", 16, 16, 60)?,
        })
    }
}

/// Tampons de travail, alloués une fois.
#[derive(Debug)]
struct Scratch {
    feat_erb: [Vec<f32>; 3],
    feat_spec: [Vec<f32>; 3],
    c0_hist: [Vec<f32>; 5],
    e0: Vec<f32>,
    e1: Vec<f32>,
    e2: Vec<f32>,
    e3: Vec<f32>,
    c0: Vec<f32>,
    tmp: Vec<f32>,
    tmp2: Vec<f32>,
    emb: Vec<f32>,
    vec_a: Vec<f32>,
    vec_b: Vec<f32>,
    gains: [f32; NB_ERB],
    coefs: Vec<f32>,
    p: Vec<f32>,
}

impl Scratch {
    fn new() -> Self {
        let z = |n| vec![0f32; n];
        Self {
            feat_erb: [z(NB_ERB), z(NB_ERB), z(NB_ERB)],
            feat_spec: [z(2 * NB_DF), z(2 * NB_DF), z(2 * NB_DF)],
            c0_hist: [z(CH * NB_DF), z(CH * NB_DF), z(CH * NB_DF), z(CH * NB_DF), z(CH * NB_DF)],
            e0: z(CH * 32),
            e1: z(CH * 16),
            e2: z(CH * 8),
            e3: z(CH * 8),
            c0: z(CH * NB_DF),
            tmp: z(CH * NB_DF),
            tmp2: z(CH * NB_DF),
            emb: z(EMB),
            vec_a: z(1024),
            vec_b: z(1024),
            gains: [0.0; NB_ERB],
            coefs: z(NB_DF * 2 * DF_ORDER),
            p: z(2 * DF_ORDER * NB_DF),
        }
    }
}

/// Fait tourner un tableau de tampons : le plus ancien devient le plus récent.
fn push_frame<const N: usize>(hist: &mut [Vec<f32>; N], frame: &[f32]) {
    hist.rotate_left(1);
    hist[N - 1].copy_from_slice(frame);
}

/// DeepFilterNet3 complet, mono, 48 kHz, trames de [`HOP`] échantillons.
pub struct DfnEngine {
    enc: Encoder,
    erb_dec: ErbDecoder,
    df_dec: DfDecoder,
    s: Scratch,
    erb: [usize; NB_ERB],
    window: Vec<f32>,
    wnorm: f32,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    fft_buf: Vec<Complex32>,
    fft_scratch: Vec<Complex32>,
    analysis_mem: Vec<f32>,
    synthesis_mem: Vec<f32>,
    mean_norm: [f32; NB_ERB],
    unit_norm: [f32; NB_DF],
    spec: Vec<Complex32>,
    /// Spectres d'origine pour les gains, du plus ancien au plus récent.
    spec_y: Vec<Vec<Complex32>>,
    /// Spectres d'origine pour le filtre profond.
    spec_x: Vec<Vec<Complex32>>,
    out_spec: Vec<Complex32>,
    skip_counter: u32,
    atten_lim: Option<f32>,
}

impl std::fmt::Debug for DfnEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DfnEngine").field("atten_lim", &self.atten_lim).finish_non_exhaustive()
    }
}

impl DfnEngine {
    /// Charge les poids embarqués (environ 9 Mo).
    ///
    /// # Errors
    /// Si un fichier de poids est illisible ou incomplet.
    pub fn new() -> Result<Self, LoadError> {
        let enc = Encoder::load(&onnx::weights(ENC)?)?;
        let erb_dec = ErbDecoder::load(&onnx::weights(ERB_DEC)?)?;
        let df_dec = DfDecoder::load(&onnx::weights(DF_DEC)?)?;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT);
        let ifft = planner.plan_fft_inverse(FFT);
        let scratch_len = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        let half = FFT / 2;
        let window = (0..FFT)
            .map(|i| {
                let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half as f64).sin();
                (0.5 * std::f64::consts::PI * s * s).sin() as f32
            })
            .collect();
        let mut e = Self {
            enc,
            erb_dec,
            df_dec,
            s: Scratch::new(),
            erb: erb_bands(),
            window,
            wnorm: 1.0 / ((FFT * FFT) as f32 / (2 * HOP) as f32),
            fft,
            ifft,
            fft_buf: vec![Complex32::default(); FFT],
            fft_scratch: vec![Complex32::default(); scratch_len],
            analysis_mem: vec![0.0; FFT - HOP],
            synthesis_mem: vec![0.0; FFT - HOP],
            mean_norm: [0.0; NB_ERB],
            unit_norm: [0.0; NB_DF],
            spec: vec![Complex32::default(); FREQS],
            spec_y: vec![vec![Complex32::default(); FREQS]; DF_ORDER + CONV_LOOKAHEAD],
            spec_x: vec![vec![Complex32::default(); FREQS]; DF_ORDER.max(LOOKAHEAD)],
            out_spec: vec![Complex32::default(); FREQS],
            skip_counter: 0,
            atten_lim: None,
        };
        e.reset();
        Ok(e)
    }

    /// Remet tous les états à zéro (nouveau flux).
    pub fn reset(&mut self) {
        self.analysis_mem.fill(0.0);
        self.synthesis_mem.fill(0.0);
        for (i, m) in self.mean_norm.iter_mut().enumerate() {
            *m = -60.0 + i as f32 * (-90.0 + 60.0) / (NB_ERB - 1) as f32;
        }
        for (i, u) in self.unit_norm.iter_mut().enumerate() {
            *u = 0.001 + i as f32 * (0.0001 - 0.001) / (NB_DF - 1) as f32;
        }
        for b in self.spec_y.iter_mut().chain(self.spec_x.iter_mut()) {
            b.fill(Complex32::default());
        }
        let s = &mut self.s;
        for b in s.feat_erb.iter_mut().chain(s.feat_spec.iter_mut()).chain(s.c0_hist.iter_mut()) {
            b.fill(0.0);
        }
        for g in [&mut self.enc.gru, &mut self.erb_dec.gru1, &mut self.erb_dec.gru2] {
            g.reset();
        }
        self.df_dec.gru1.reset();
        self.df_dec.gru2.reset();
        self.skip_counter = 0;
    }

    /// Limite l'atténuation (dB) ; 100 ou plus = pas de limite.
    pub fn set_atten_lim(&mut self, db: f32) {
        let lim = db.abs();
        self.atten_lim = if lim >= 100.0 {
            None
        } else if lim < 0.01 {
            Some(1.0)
        } else {
            Some(10f32.powf(-lim / 20.0))
        };
    }

    /// Débruite `input` (une trame) dans `output` ; renvoie le rapport
    /// signal/bruit local estimé (dB). Aucune allocation.
    pub fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> f32 {
        let energy = input.iter().map(|x| x * x).sum::<f32>() / HOP as f32;
        if energy < 1e-7 {
            self.skip_counter += 1;
        } else {
            self.skip_counter = 0;
        }
        if self.skip_counter > 5 {
            output.fill(0.0);
            return -15.0;
        }

        self.analysis(input);
        for ring in [&mut self.spec_y, &mut self.spec_x] {
            ring.rotate_left(1);
            if let Some(newest) = ring.last_mut() {
                newest.copy_from_slice(&self.spec);
            }
        }
        let noisy_index = DF_ORDER.max(LOOKAHEAD) - LOOKAHEAD - 1;

        if self.atten_lim == Some(1.0) {
            self.out_spec.copy_from_slice(&self.spec_x[noisy_index]);
            self.synthesis(output);
            return 35.0;
        }

        self.features();
        let lsnr = self.encode();
        let (apply_gains, gain_zeros, apply_df) = if lsnr < MIN_DB_THRESH {
            (false, true, false)
        } else if lsnr > MAX_DB_ERB_THRESH {
            (false, false, false)
        } else if lsnr > MAX_DB_DF_THRESH {
            (true, false, false)
        } else {
            (true, false, true)
        };

        let target = DF_ORDER - 1;
        if apply_gains {
            self.decode_erb();
            apply_band_gains(&mut self.spec_y[target], &self.s.gains, &self.erb);
            self.skip_counter = 0;
        } else if gain_zeros {
            apply_band_gains(&mut self.spec_y[target], &[0.0; NB_ERB], &self.erb);
            self.skip_counter = 0;
        } else {
            self.skip_counter += 1;
        }

        self.out_spec.copy_from_slice(&self.spec_y[target]);
        if apply_df {
            self.decode_df();
            self.deep_filter();
        }

        if let Some(lim) = self.atten_lim {
            for (o, n) in self.out_spec.iter_mut().zip(&self.spec_x[noisy_index]) {
                *o = *o * (1.0 - lim) + *n * lim;
            }
        }
        self.synthesis(output);
        lsnr
    }

    fn analysis(&mut self, input: &[f32; HOP]) {
        let split = FFT - HOP;
        for i in 0..split {
            self.fft_buf[i] = Complex32::new(self.analysis_mem[i] * self.window[i], 0.0);
        }
        for ((b, &x), &w) in self.fft_buf[split..].iter_mut().zip(input).zip(&self.window[split..]) {
            *b = Complex32::new(x * w, 0.0);
        }
        let keep = self.analysis_mem.len() - HOP;
        self.analysis_mem.rotate_left(HOP);
        self.analysis_mem[keep..].copy_from_slice(input);
        self.fft.process_with_scratch(&mut self.fft_buf, &mut self.fft_scratch);
        for (s, b) in self.spec.iter_mut().zip(&self.fft_buf) {
            *s = *b * self.wnorm;
        }
    }

    fn synthesis(&mut self, output: &mut [f32; HOP]) {
        // Spectre hermitien complet, puis FFT inverse complexe (partie réelle).
        let spec = &self.out_spec;
        self.fft_buf[0] = Complex32::new(spec[0].re, 0.0);
        self.fft_buf[FFT / 2] = Complex32::new(spec[FFT / 2].re, 0.0);
        for (k, &c) in spec.iter().enumerate().take(FFT / 2).skip(1) {
            self.fft_buf[k] = c;
            self.fft_buf[FFT - k] = c.conj();
        }
        self.ifft.process_with_scratch(&mut self.fft_buf, &mut self.fft_scratch);
        for (b, w) in self.fft_buf.iter_mut().zip(&self.window) {
            b.re *= w;
        }
        for ((o, b), m) in output.iter_mut().zip(&self.fft_buf).zip(&self.synthesis_mem) {
            *o = b.re + m;
        }
        let split = self.synthesis_mem.len() - HOP;
        self.synthesis_mem.rotate_left(HOP);
        for i in 0..split {
            self.synthesis_mem[i] += self.fft_buf[HOP + i].re;
        }
        for i in split..self.synthesis_mem.len() {
            self.synthesis_mem[i] = self.fft_buf[HOP + i].re;
        }
    }

    /// Caractéristiques de la trame la plus récente (`self.spec`).
    fn features(&mut self) {
        let mut erb = [0f32; NB_ERB];
        let mut start = 0;
        for (b, (&width, e)) in self.erb.iter().zip(erb.iter_mut()).enumerate() {
            let k = 1.0 / width as f32;
            let power: f32 = self.spec[start..start + width].iter().map(|c| (c.re * c.re + c.im * c.im) * k).sum();
            let db = (power + 1e-10).log10() * 10.0;
            let m = &mut self.mean_norm[b];
            *m = db * (1.0 - ALPHA) + *m * ALPHA;
            *e = (db - *m) / 40.0;
            start += width;
        }
        push_frame(&mut self.s.feat_erb, &erb);

        let mut cplx = [0f32; 2 * NB_DF];
        for (f, u) in self.unit_norm.iter_mut().enumerate() {
            let c = self.spec[f];
            *u = c.norm() * (1.0 - ALPHA) + *u * ALPHA;
            let d = u.sqrt();
            cplx[f] = c.re / d;
            cplx[NB_DF + f] = c.im / d;
        }
        push_frame(&mut self.s.feat_spec, &cplx);
    }

    fn encode(&mut self) -> f32 {
        let (e, s) = (&mut self.enc, &mut self.s);
        let hist = [&s.feat_erb[0][..], &s.feat_erb[1][..], &s.feat_erb[2][..]];
        e.erb_conv0.forward(&hist, NB_ERB, &mut s.e0);
        relu(&mut s.e0);
        e.erb_conv1.0.forward(&[&s.e0], 32, &mut s.tmp);
        e.erb_conv1.1.forward(&s.tmp, 16, &mut s.e1, true);
        e.erb_conv2.0.forward(&[&s.e1], 16, &mut s.tmp);
        e.erb_conv2.1.forward(&s.tmp, 8, &mut s.e2, true);
        e.erb_conv3.0.forward(&[&s.e2], 8, &mut s.tmp);
        e.erb_conv3.1.forward(&s.tmp, 8, &mut s.e3, true);

        let hist = [&s.feat_spec[0][..], &s.feat_spec[1][..], &s.feat_spec[2][..]];
        e.df_conv0.0.forward(&hist, NB_DF, &mut s.tmp);
        e.df_conv0.1.forward(&s.tmp, NB_DF, &mut s.c0, true);
        e.df_conv1.0.forward(&[&s.c0], NB_DF, &mut s.tmp);
        e.df_conv1.1.forward(&s.tmp, NB_DF / 2, &mut s.tmp2, true);
        // [64][48] → [48][64] puis couche linéaire par groupes de 96.
        transpose(&s.tmp2, CH, NB_DF / 2, &mut s.tmp);
        e.df_fc_emb.forward(&s.tmp[..CH * NB_DF / 2], &mut s.vec_a);
        relu(&mut s.vec_a[..EMB]);
        // e3 [64][8] → [8][64], additionné.
        transpose(&s.e3, CH, 8, &mut s.tmp);
        for (a, b) in s.vec_a[..EMB].iter_mut().zip(&s.tmp[..EMB]) {
            *a += b;
        }
        e.lin_in.forward(&s.vec_a, &mut s.vec_b);
        relu(&mut s.vec_b[..HIDDEN]);
        let h = e.gru.step(&s.vec_b[..HIDDEN]);
        e.lin_out.forward(h, &mut s.emb);
        relu(&mut s.emb);
        let z: f32 = s.emb.iter().zip(&e.lsnr_w).map(|(a, b)| a * b).sum::<f32>() + e.lsnr_b;
        (1.0 / (1.0 + (-z).exp())) * e.lsnr_scale + e.lsnr_offset
    }

    fn decode_erb(&mut self) {
        let (d, s) = (&mut self.erb_dec, &mut self.s);
        d.lin_in.forward(&s.emb, &mut s.vec_a);
        relu(&mut s.vec_a[..HIDDEN]);
        let h1 = d.gru1.step(&s.vec_a[..HIDDEN]);
        let h2 = d.gru2.step(h1);
        d.lin_out.forward(h2, &mut s.vec_b);
        relu(&mut s.vec_b[..EMB]);
        // [8][64] → [64][8].
        transpose(&s.vec_b[..EMB], 8, CH, &mut s.tmp);

        // Niveau 3 (8 bandes).
        d.conv3p.forward(&[&s.e3], 8, &mut s.tmp2);
        relu(&mut s.tmp2[..CH * 8]);
        add(&mut s.tmp[..CH * 8], &s.tmp2[..CH * 8]);
        d.convt3.0.forward(&[&s.tmp[..CH * 8]], 8, &mut s.tmp2);
        d.convt3.1.forward(&s.tmp2, 8, &mut s.tmp, true);
        // Niveau 2 (8 → 16).
        d.conv2p.forward(&[&s.e2], 8, &mut s.tmp2);
        relu(&mut s.tmp2[..CH * 8]);
        add(&mut s.tmp[..CH * 8], &s.tmp2[..CH * 8]);
        d.convt2.0.forward(&s.tmp, 8, &mut s.tmp2);
        d.convt2.1.forward(&s.tmp2, 16, &mut s.tmp, true);
        // Niveau 1 (16 → 32).
        d.conv1p.forward(&[&s.e1], 16, &mut s.tmp2);
        relu(&mut s.tmp2[..CH * 16]);
        add(&mut s.tmp[..CH * 16], &s.tmp2[..CH * 16]);
        d.convt1.0.forward(&s.tmp, 16, &mut s.tmp2);
        d.convt1.1.forward(&s.tmp2, 32, &mut s.tmp, true);
        // Niveau 0 (32 bandes) → gains.
        d.conv0p.forward(&[&s.e0], 32, &mut s.tmp2);
        relu(&mut s.tmp2[..CH * 32]);
        add(&mut s.tmp[..CH * 32], &s.tmp2[..CH * 32]);
        d.conv0_out.forward(&[&s.tmp[..CH * 32]], 32, &mut s.gains);
        sigmoid_inplace(&mut s.gains);
    }

    fn decode_df(&mut self) {
        let (d, s) = (&mut self.df_dec, &mut self.s);
        d.lin_in.forward(&s.emb, &mut s.vec_a);
        relu(&mut s.vec_a[..HIDDEN]);
        let h1 = d.gru1.step(&s.vec_a[..HIDDEN]);
        let h2 = d.gru2.step(h1);
        let mut c = [0f32; HIDDEN];
        c.copy_from_slice(h2);
        d.skip.forward(&s.emb, &mut s.vec_b);
        add(&mut c, &s.vec_b[..HIDDEN]);

        push_frame(&mut s.c0_hist, &s.c0);
        let hist = [&s.c0_hist[0][..], &s.c0_hist[1][..], &s.c0_hist[2][..], &s.c0_hist[3][..], &s.c0_hist[4][..]];
        d.convp.0.forward(&hist, NB_DF, &mut s.tmp);
        d.convp.1.forward(&s.tmp, NB_DF, &mut s.p, true);

        d.out.forward(&c, &mut s.coefs);
        // coefs [96][10] = tanh(sortie) + convp transposé ([10][96] → [96][10]).
        for f in 0..NB_DF {
            for k in 0..2 * DF_ORDER {
                let v = &mut s.coefs[f * 2 * DF_ORDER + k];
                *v = v.tanh() + s.p[k * NB_DF + f];
            }
        }
    }

    /// Filtre profond : pour chaque case basse, combinaison complexe des 5
    /// dernières trames (2 passées, la courante, 2 futures).
    fn deep_filter(&mut self) {
        for f in 0..NB_DF {
            let mut acc = Complex32::default();
            for (k, frame) in self.spec_x.iter().enumerate() {
                let c =
                    Complex32::new(self.s.coefs[f * 2 * DF_ORDER + 2 * k], self.s.coefs[f * 2 * DF_ORDER + 2 * k + 1]);
                acc += frame[f] * c;
            }
            self.out_spec[f] = acc;
        }
    }
}

fn add(a: &mut [f32], b: &[f32]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

/// `x` `[rows][cols]` → `out` `[cols][rows]`.
fn transpose(x: &[f32], rows: usize, cols: usize, out: &mut [f32]) {
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = x[r * cols + c];
        }
    }
}

/// Multiplie chaque case du spectre par le gain de sa bande ERB.
fn apply_band_gains(spec: &mut [Complex32], gains: &[f32; NB_ERB], erb: &[usize; NB_ERB]) {
    let mut start = 0;
    for (&width, &g) in erb.iter().zip(gains) {
        for c in &mut spec[start..start + width] {
            *c *= g;
        }
        start += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deep_filter_rt::tract::DfTract;
    use ndarray::Array2;

    /// Voix synthétique (syllabes harmoniques) sur un bruit de fond, suivie
    /// d'un silence numérique (pour la logique de saut des trames muettes).
    fn noisy_voice(secs: f32, noise_db: f32) -> Vec<f32> {
        let (_, mut speech) = crate::calibrate::synthetic_voice(secs, noise_db, -22.0);
        speech.extend(std::iter::repeat_n(0.0, HOP * 10));
        speech
    }

    #[test]
    fn matches_the_official_engine() {
        // Bruit faible (gains seuls) et fort (gains + filtre profond).
        for noise_db in [-45.0, -28.0] {
            compare_with_official(&noisy_voice(2.0, noise_db), 100.0);
        }
        // Avec une limite d'atténuation.
        compare_with_official(&noisy_voice(1.0, -28.0), 12.0);
        compare_with_official(&noisy_voice(0.5, -28.0), 0.0);
    }

    fn compare_with_official(x: &[f32], atten_lim_db: f32) {
        let mut ours = DfnEngine::new().expect("poids embarqués");
        ours.set_atten_lim(atten_lim_db);
        let mut official = DfTract::default();
        official.set_atten_lim(atten_lim_db);
        let (mut inp, mut out) = (Array2::<f32>::zeros((1, HOP)), Array2::<f32>::zeros((1, HOP)));
        let (mut max_diff, mut max_ref, mut max_lsnr_diff) = (0f32, 0f32, 0f32);
        for chunk in x.as_chunks::<HOP>().0 {
            let mut mine = [0f32; HOP];
            let l1 = ours.process(chunk, &mut mine);
            inp.as_slice_mut().expect("contigu").copy_from_slice(chunk);
            let l2 = official.process(inp.view(), out.view_mut()).expect("modèle officiel");
            max_lsnr_diff = max_lsnr_diff.max((l1 - l2).abs());
            for (a, b) in mine.iter().zip(out.iter()) {
                max_diff = max_diff.max((a - b).abs());
                max_ref = max_ref.max(b.abs());
            }
        }
        assert!(max_ref > 0.01, "le signal de référence n'est pas muet");
        assert!(max_lsnr_diff < 0.05, "écart de SNR local : {max_lsnr_diff} dB");
        // Mesuré : 7e-8 pour une crête de 0,12 (arrondis du calcul flottant).
        assert!(max_diff < 1e-5, "écart maximal {max_diff} (crête {max_ref}, limite {atten_lim_db} dB)");
    }

    #[test]
    fn erb_bands_cover_the_spectrum() {
        let erb = erb_bands();
        assert_eq!(erb.iter().sum::<usize>(), FREQS);
        assert!(erb.iter().all(|&w| w >= 2));
        assert!(erb[0] <= erb[NB_ERB - 1], "bandes étroites dans les graves");
        assert_eq!(LATENCY, 1440);
    }

    #[test]
    fn transpose_and_band_gains() {
        let mut out = [0.0; 6];
        transpose(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2, 3, &mut out);
        assert_eq!(out, [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
        let mut spec = vec![Complex32::new(1.0, 1.0); FREQS];
        let mut g = [1.0; NB_ERB];
        g[0] = 0.0;
        apply_band_gains(&mut spec, &g, &erb_bands());
        assert_eq!(spec[0], Complex32::default());
        assert_eq!(spec[FREQS - 1], Complex32::new(1.0, 1.0));
    }
}
