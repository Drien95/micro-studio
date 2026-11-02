//! Mesure de sonie intégrée en LUFS selon la recommandation UIT-R BS.1770-5
//! (celle qu'utilisent la norme broadcast EBU R128 et les plateformes de
//! streaming) : pondération K, blocs de 400 ms recouverts à 75 %, puis double
//! portillonnage absolu (-70 LUFS) et relatif (-10 LU).

use super::util::SAMPLE_RATE;

/// Valeur renvoyée quand il n'y a rien à mesurer (silence ou signal trop court).
pub const SILENT_LUFS: f32 = -120.0;

const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const RELATIVE_GATE_LU: f64 = -10.0;

/// Biquad en double précision : les pôles du filtre RLB sont très proches du
/// cercle unité, la simple précision perdrait des dixièmes de dB.
#[derive(Clone, Copy)]
struct Stage {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Stage {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Coefficients de la pondération K à 48 kHz, tels que publiés dans BS.1770.
fn k_weighting() -> [Stage; 2] {
    [
        // Étage 1 : plateau aigu (+4 dB) qui modélise l'effet de la tête.
        Stage {
            b: [1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85],
            a: [-1.690_659_293_182_41, 0.732_480_774_215_85],
            z: [0.0; 2],
        },
        // Étage 2 : passe-haut « RLB » vers 38 Hz.
        Stage { b: [1.0, -2.0, 1.0], a: [-1.990_047_454_833_98, 0.990_072_250_366_21], z: [0.0; 2] },
    ]
}

fn block_lufs(mean_square: f64) -> f64 {
    -0.691 + 10.0 * mean_square.max(1e-30).log10()
}

/// Sonie intégrée (LUFS) d'un signal mono à 48 kHz.
///
/// Renvoie [`SILENT_LUFS`] si le signal dure moins de 400 ms ou si tous les
/// blocs sont sous le seuil absolu.
#[must_use]
pub fn integrated_lufs(x: &[f32]) -> f32 {
    let block = (0.4 * SAMPLE_RATE) as usize;
    let step = block / 4;
    if x.len() < block {
        return SILENT_LUFS;
    }
    let mut stages = k_weighting();
    let squares: Vec<f64> = x
        .iter()
        .map(|&s| {
            let s = if s.is_finite() { f64::from(s) } else { 0.0 };
            let y = stages.iter_mut().fold(s, |acc, st| st.process(acc));
            y * y
        })
        .collect();

    let mut blocks = Vec::with_capacity((x.len() - block) / step + 1);
    let mut start = 0;
    while start + block <= squares.len() {
        blocks.push(squares[start..start + block].iter().sum::<f64>() / block as f64);
        start += step;
    }

    let above_abs: Vec<f64> = blocks.into_iter().filter(|&ms| block_lufs(ms) > ABSOLUTE_GATE_LUFS).collect();
    if above_abs.is_empty() {
        return SILENT_LUFS;
    }
    let relative_gate = block_lufs(above_abs.iter().sum::<f64>() / above_abs.len() as f64) + RELATIVE_GATE_LU;
    let kept: Vec<f64> = above_abs.into_iter().filter(|&ms| block_lufs(ms) > relative_gate).collect();
    // `kept` n'est jamais vide : le bloc le plus fort dépasse toujours la moyenne -10 LU.
    block_lufs(kept.iter().sum::<f64>() / kept.len().max(1) as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE).sin())
            .collect()
    }

    #[test]
    fn full_scale_1khz_sine_reads_minus_3_lufs() {
        // Valeur de référence de BS.1770 : sinus 997 Hz à 0 dBFS sur un canal = -3,01 LUFS.
        let l = integrated_lufs(&sine(997.0, 1.0, 3.0));
        assert!((l + 3.01).abs() < 0.1, "{l}");
        let quieter = integrated_lufs(&sine(997.0, 0.1, 3.0));
        assert!((quieter + 23.01).abs() < 0.1, "{quieter}");
    }

    #[test]
    fn k_weighting_favours_presence_and_ignores_rumble() {
        let presence = integrated_lufs(&sine(3000.0, 0.1, 2.0));
        let mid = integrated_lufs(&sine(997.0, 0.1, 2.0));
        let rumble = integrated_lufs(&sine(20.0, 0.1, 2.0));
        assert!(presence > mid + 2.0, "{presence} / {mid}");
        assert!(rumble < mid - 10.0, "{rumble} / {mid}");
    }

    #[test]
    fn pauses_are_gated_out() {
        // 2 s de voix, 6 s de quasi-silence : la sonie est celle de la voix seule.
        let mut x = sine(997.0, 0.1, 2.0);
        x.extend(sine(997.0, 0.0001, 6.0));
        assert!((integrated_lufs(&x) + 23.0).abs() < 0.5, "{}", integrated_lufs(&x));
    }

    #[test]
    fn silence_and_short_signals_have_no_loudness() {
        assert!((integrated_lufs(&[]) - SILENT_LUFS).abs() < f32::EPSILON);
        assert!((integrated_lufs(&sine(997.0, 1.0, 0.2)) - SILENT_LUFS).abs() < f32::EPSILON);
        assert!((integrated_lufs(&vec![0.0; 48_000]) - SILENT_LUFS).abs() < f32::EPSILON);
        let mut bad = sine(997.0, 0.1, 1.0);
        bad[100] = f32::NAN;
        assert!(integrated_lufs(&bad).is_finite());
    }
}
