//! Couches du réseau DeepFilterNet3, trame par trame (temps réel).
//!
//! Chaque couche lit ses poids au chargement puis ne fait plus aucune
//! allocation. Les cartes de caractéristiques sont rangées `[canal][fréquence]`
//! dans des tranches plates.

use super::onnx::Tensor;
use std::collections::HashMap;

/// Erreur de chargement : poids absent ou de forme inattendue.
#[derive(Debug, PartialEq, Eq)]
pub struct MissingWeight(pub String);

impl std::fmt::Display for MissingWeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "poids manquant ou mal formé : {}", self.0)
    }
}

impl std::error::Error for MissingWeight {}

pub type Weights = HashMap<String, Tensor>;

/// Récupère un tenseur et vérifie sa forme.
pub fn take(w: &Weights, name: &str, dims: &[usize]) -> Result<Vec<f32>, MissingWeight> {
    match w.get(name) {
        Some(t) if t.dims == dims => Ok(t.data.clone()),
        _ => Err(MissingWeight(format!("{name} {dims:?}"))),
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    // Seize accumulateurs indépendants : le compilateur en fait 4 registres
    // vectoriels (8 avec AVX) et les additions ne s'attendent plus les unes
    // les autres (latence de l'addition flottante masquée).
    const LANES: usize = 16;
    let mut acc = [0f32; LANES];
    let (ca, ra) = a.as_chunks::<LANES>();
    let (cb, rb) = b.as_chunks::<LANES>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..LANES {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s = 0.0;
    for half in acc.as_chunks::<4>().0 {
        s += (half[0] + half[1]) + (half[2] + half[3]);
    }
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

/// Convolution 2D sur (temps, fréquence), causale en temps.
///
/// `hist` contient les `kt` dernières trames d'entrée, de la plus ancienne à
/// la plus récente, chacune rangée `[canal][fréquence]`.
#[derive(Debug)]
pub struct Conv {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_per_group: usize,
    groups: usize,
    kt: usize,
    kf: usize,
    stride: usize,
    pad: usize,
}

impl Conv {
    /// `w` de forme `[out, in/groups, kt, kf]` ; `b` optionnel.
    #[expect(clippy::too_many_arguments, reason = "hyperparamètres d'une convolution")]
    pub fn load(
        w: &Weights,
        weight: &str,
        bias: Option<&str>,
        out_ch: usize,
        in_ch: usize,
        groups: usize,
        (kt, kf): (usize, usize),
        stride: usize,
        pad: usize,
    ) -> Result<Self, MissingWeight> {
        let in_per_group = in_ch / groups;
        let wv = take(w, weight, &[out_ch, in_per_group, kt, kf])?;
        let b = match bias {
            Some(name) => take(w, name, &[out_ch])?,
            None => vec![0.0; out_ch],
        };
        Ok(Self { w: wv, b, out_ch, in_per_group, groups, kt, kf, stride, pad })
    }

    /// Nombre de fréquences en sortie pour `f_in` en entrée.
    pub fn out_len(&self, f_in: usize) -> usize {
        (f_in + 2 * self.pad - self.kf) / self.stride + 1
    }

    pub fn kt(&self) -> usize {
        self.kt
    }

    pub fn forward(&self, hist: &[&[f32]], f_in: usize, out: &mut [f32]) {
        debug_assert_eq!(hist.len(), self.kt);
        let f_out = self.out_len(f_in);
        let out_per_group = self.out_ch / self.groups;
        for o in 0..self.out_ch {
            let g = o / out_per_group;
            let row = &mut out[o * f_out..(o + 1) * f_out];
            row.fill(self.b[o]);
            for c in 0..self.in_per_group {
                let ch = g * self.in_per_group + c;
                for (i, frame) in hist.iter().enumerate() {
                    let input = &frame[ch * f_in..(ch + 1) * f_in];
                    let wk = &self.w[((o * self.in_per_group + c) * self.kt + i) * self.kf..][..self.kf];
                    for (j, &wj) in wk.iter().enumerate() {
                        // fi = fo * stride + j - pad ; hors de 0..f_in, l'entrée
                        // vaut zéro (remplissage) : on restreint fo d'avance,
                        // ce qui laisse une boucle sans test, vectorisable.
                        let lo = self.pad.saturating_sub(j).div_ceil(self.stride);
                        let hi = (f_in + self.pad).saturating_sub(j).div_ceil(self.stride).min(f_out);
                        if lo >= hi {
                            continue;
                        }
                        let first = lo * self.stride + j - self.pad;
                        if self.stride == 1 {
                            for (r, &v) in row[lo..hi].iter_mut().zip(&input[first..]) {
                                *r += wj * v;
                            }
                        } else {
                            for (r, &v) in row[lo..hi].iter_mut().zip(input[first..].iter().step_by(self.stride)) {
                                *r += wj * v;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Convolution 1×1 (mélange des canaux), écrite comme un produit matriciel.
#[derive(Debug)]
pub struct Pointwise {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
}

impl Pointwise {
    pub fn load(w: &Weights, weight: &str, bias: &str, out_ch: usize, in_ch: usize) -> Result<Self, MissingWeight> {
        Ok(Self { w: take(w, weight, &[out_ch, in_ch, 1, 1])?, b: take(w, bias, &[out_ch])?, out_ch, in_ch })
    }

    /// `x` `[in][f]` → `out` `[out][f]`, puis ReLU si `relu`.
    pub fn forward(&self, x: &[f32], f: usize, out: &mut [f32], relu: bool) {
        for o in 0..self.out_ch {
            let row = &mut out[o * f..(o + 1) * f];
            row.fill(self.b[o]);
            for c in 0..self.in_ch {
                let wc = self.w[o * self.in_ch + c];
                for (r, &xi) in row.iter_mut().zip(&x[c * f..(c + 1) * f]) {
                    *r += wc * xi;
                }
            }
            if relu {
                for r in row.iter_mut() {
                    *r = r.max(0.0);
                }
            }
        }
    }
}

/// Convolution transposée « depthwise » sur la fréquence (noyau 3, pas 2,
/// remplissage 1, remplissage de sortie 1) : double la résolution.
#[derive(Debug)]
pub struct UpsampleDw {
    w: Vec<f32>,
    ch: usize,
}

impl UpsampleDw {
    pub fn load(w: &Weights, weight: &str, ch: usize) -> Result<Self, MissingWeight> {
        Ok(Self { w: take(w, weight, &[ch, 1, 1, 3])?, ch })
    }

    /// `x` `[ch][f]` → `out` `[ch][2f]`.
    pub fn forward(&self, x: &[f32], f: usize, out: &mut [f32]) {
        let fo = 2 * f;
        out[..self.ch * fo].fill(0.0);
        for c in 0..self.ch {
            let wk = &self.w[c * 3..c * 3 + 3];
            for i in 0..f {
                let xi = x[c * f + i];
                for (j, &wj) in wk.iter().enumerate() {
                    let o = (2 * i + j).wrapping_sub(1);
                    if o < fo {
                        out[c * fo + o] += xi * wj;
                    }
                }
            }
        }
    }
}

/// Couche linéaire par groupes (`btgi,gih->btgh`), sans biais.
#[derive(Debug)]
pub struct GroupedLinear {
    w: Vec<f32>,
    groups: usize,
    inp: usize,
    hid: usize,
}

impl GroupedLinear {
    pub fn load(w: &Weights, weight: &str, groups: usize, inp: usize, hid: usize) -> Result<Self, MissingWeight> {
        Ok(Self { w: take(w, weight, &[groups, inp, hid])?, groups, inp, hid })
    }

    pub fn out_len(&self) -> usize {
        self.groups * self.hid
    }

    pub fn forward(&self, x: &[f32], out: &mut [f32]) {
        out[..self.groups * self.hid].fill(0.0);
        for g in 0..self.groups {
            let o = &mut out[g * self.hid..(g + 1) * self.hid];
            for i in 0..self.inp {
                let xi = x[g * self.inp + i];
                let wr = &self.w[(g * self.inp + i) * self.hid..][..self.hid];
                for (oh, &wh) in o.iter_mut().zip(wr) {
                    *oh += xi * wh;
                }
            }
        }
    }
}

/// Cellule GRU (convention ONNX : portes z, r, h ; `linear_before_reset`).
#[derive(Debug)]
pub struct Gru {
    w: Vec<f32>,
    r: Vec<f32>,
    b: Vec<f32>,
    inp: usize,
    h: Vec<f32>,
    gx: Vec<f32>,
    gh: Vec<f32>,
}

/// Taille de l'état caché des GRU de DeepFilterNet3.
pub const HIDDEN: usize = 256;

impl Gru {
    pub fn load(
        w: &Weights,
        prefix_w: &str,
        prefix_r: &str,
        prefix_b: &str,
        inp: usize,
    ) -> Result<Self, MissingWeight> {
        Ok(Self {
            w: take(w, prefix_w, &[1, 3 * HIDDEN, inp])?,
            r: take(w, prefix_r, &[1, 3 * HIDDEN, HIDDEN])?,
            b: take(w, prefix_b, &[1, 6 * HIDDEN])?,
            inp,
            h: vec![0.0; HIDDEN],
            gx: vec![0.0; 3 * HIDDEN],
            gh: vec![0.0; 3 * HIDDEN],
        })
    }

    pub fn reset(&mut self) {
        self.h.fill(0.0);
    }

    /// Avance d'une trame ; renvoie le nouvel état caché.
    pub fn step(&mut self, x: &[f32]) -> &[f32] {
        for (k, (gx, gh)) in self.gx.iter_mut().zip(self.gh.iter_mut()).enumerate() {
            *gx = dot(&self.w[k * self.inp..(k + 1) * self.inp], x) + self.b[k];
            *gh = dot(&self.r[k * HIDDEN..(k + 1) * HIDDEN], &self.h) + self.b[3 * HIDDEN + k];
        }
        for k in 0..HIDDEN {
            let z = sigmoid(self.gx[k] + self.gh[k]);
            let r = sigmoid(self.gx[HIDDEN + k] + self.gh[HIDDEN + k]);
            let n = (self.gx[2 * HIDDEN + k] + r * self.gh[2 * HIDDEN + k]).tanh();
            self.h[k] = (1.0 - z) * n + z * self.h[k];
        }
        &self.h
    }
}

/// Applique ReLU en place.
pub fn relu(x: &mut [f32]) {
    for v in x.iter_mut() {
        *v = v.max(0.0);
    }
}

/// Applique la sigmoïde en place.
pub fn sigmoid_inplace(x: &mut [f32]) {
    for v in x.iter_mut() {
        *v = sigmoid(*v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights(entries: &[(&str, &[usize], Vec<f32>)]) -> Weights {
        entries.iter().map(|(n, d, v)| ((*n).to_owned(), Tensor { dims: d.to_vec(), data: v.clone() })).collect()
    }

    #[test]
    fn strided_depthwise_conv_matches_a_hand_computation() {
        // 1 canal, noyau [1, 2, 3], pas 2, remplissage 1 : f_in 4 → f_out 2.
        let w = weights(&[("w", &[1, 1, 1, 3], vec![1.0, 2.0, 3.0]), ("b", &[1], vec![0.5])]);
        let c = Conv::load(&w, "w", Some("b"), 1, 1, 1, (1, 3), 2, 1).expect("poids");
        let x = [1.0, 10.0, 100.0, 1000.0];
        let mut out = [0.0; 2];
        c.forward(&[&x], 4, &mut out);
        // fo=0 : x[-1]*1 + x[0]*2 + x[1]*3 ; fo=1 : x[1]*1 + x[2]*2 + x[3]*3.
        assert_eq!(out, [0.5 + 2.0 + 30.0, 0.5 + 10.0 + 200.0 + 3000.0]);
        assert_eq!(c.out_len(4), 2);
        assert_eq!(c.kt(), 1);
    }

    #[test]
    fn causal_time_kernel_uses_the_history_in_order() {
        // Noyau temporel [1, 10, 100] : la trame la plus récente pèse 100.
        let w = weights(&[("w", &[1, 1, 3, 1], vec![1.0, 10.0, 100.0])]);
        let c = Conv::load(&w, "w", None, 1, 1, 1, (3, 1), 1, 0).expect("poids");
        let mut out = [0.0];
        c.forward(&[&[1.0], &[2.0], &[3.0]], 1, &mut out);
        assert!((out[0] - 321.0).abs() < 1e-6);
    }

    #[test]
    fn grouped_conv_keeps_groups_apart() {
        // 2 entrées, 2 sorties, 2 groupes : chaque sortie ne voit que son entrée.
        let w = weights(&[("w", &[2, 1, 1, 1], vec![2.0, 3.0])]);
        let c = Conv::load(&w, "w", None, 2, 2, 2, (1, 1), 1, 0).expect("poids");
        let mut out = [0.0; 2];
        c.forward(&[&[5.0, 7.0]], 1, &mut out);
        assert_eq!(out, [10.0, 21.0]);
    }

    #[test]
    fn pointwise_mixes_channels() {
        let w = weights(&[("w", &[1, 2, 1, 1], vec![1.0, -1.0]), ("b", &[1], vec![0.0])]);
        let p = Pointwise::load(&w, "w", "b", 1, 2).expect("poids");
        let mut out = [0.0; 2];
        p.forward(&[3.0, 1.0, 1.0, 4.0], 2, &mut out, false);
        assert_eq!(out, [2.0, -3.0]);
        p.forward(&[3.0, 1.0, 1.0, 4.0], 2, &mut out, true);
        assert_eq!(out, [2.0, 0.0]);
    }

    #[test]
    fn upsampling_doubles_the_resolution() {
        let w = weights(&[("w", &[1, 1, 1, 3], vec![1.0, 2.0, 3.0])]);
        let u = UpsampleDw::load(&w, "w", 1).expect("poids");
        let mut out = [0.0; 4];
        u.forward(&[1.0, 10.0], 2, &mut out);
        // o = 2i - 1 + j : i=0 → o=0 (j=1), o=1 (j=2) ; i=1 → o=1 (j=0), o=2 (j=1), o=3 (j=2).
        assert_eq!(out, [2.0, 3.0 + 10.0, 20.0, 30.0]);
    }

    #[test]
    fn grouped_linear_and_gru_shapes() {
        let w = weights(&[("l", &[2, 2, 1], vec![1.0, 2.0, 3.0, 4.0])]);
        let l = GroupedLinear::load(&w, "l", 2, 2, 1).expect("poids");
        let mut out = [0.0; 2];
        l.forward(&[1.0, 1.0, 1.0, 2.0], &mut out);
        assert_eq!(out, [3.0, 11.0]);
        assert_eq!(l.out_len(), 2);

        // GRU à poids nuls : z = r = 0,5, n = 0 → h reste à 0.
        let w = weights(&[
            ("W", &[1, 3 * HIDDEN, 4], vec![0.0; 3 * HIDDEN * 4]),
            ("R", &[1, 3 * HIDDEN, HIDDEN], vec![0.0; 3 * HIDDEN * HIDDEN]),
            ("B", &[1, 6 * HIDDEN], vec![0.0; 6 * HIDDEN]),
        ]);
        let mut g = Gru::load(&w, "W", "R", "B", 4).expect("poids");
        assert!(g.step(&[1.0; 4]).iter().all(|&h| h == 0.0));
        g.reset();
        assert!(Gru::load(&w, "W", "R", "absent", 4).is_err());
        assert_eq!(MissingWeight("x".into()).to_string(), "poids manquant ou mal formé : x");
    }

    #[test]
    fn dot_handles_remainders() {
        assert!((dot(&[1.0; 7], &[2.0; 7]) - 14.0).abs() < 1e-6);
        let mut v = [-1.0, 0.0, 2.0];
        relu(&mut v);
        assert_eq!(v, [0.0, 0.0, 2.0]);
        sigmoid_inplace(&mut v);
        assert!((v[0] - 0.5).abs() < 1e-6);
    }
}
