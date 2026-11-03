//! Niveau de voix active selon la recommandation UIT-T P.56, méthode B.
//!
//! Le niveau « moyen » d'un enregistrement de voix dépend énormément des
//! pauses : 30 % de silence en plus le font baisser de 1,5 dB. P.56 ne
//! compte que les moments où l'on parle : l'enveloppe du signal (deux
//! lissages de 30 ms) est comparée à une série de seuils ; un échantillon
//! est « actif » s'il dépasse le seuil ou s'il le dépassait il y a moins de
//! 200 ms (maintien, pour ne pas couper les fins de mots). Le bon seuil est
//! celui qui se trouve 15,9 dB sous le niveau de la voix active qu'il donne.
//!
//! Sert au calibrage : niveau de la voix et masque « voix » de référence.

use super::util::SAMPLE_RATE;

/// Constante de temps de l'enveloppe (s).
const TIME_CONSTANT_S: f32 = 0.03;
/// Maintien après le passage sous le seuil (s).
const HANGOVER_S: f32 = 0.2;
/// Marge entre le niveau actif et le seuil (dB).
pub const MARGIN_DB: f32 = 15.9;
/// Seuils essayés : de -100 à 0 dBFS par pas de 2 dB.
const THRESHOLD_STEP_DB: f32 = 2.0;
const THRESHOLD_MIN_DB: f32 = -100.0;
const THRESHOLDS: usize = 51;

/// Résultat de la mesure.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSpeech {
    /// Niveau de la voix active (dBFS efficaces).
    pub level_db: f32,
    /// Proportion du temps où la voix est active (0..1).
    pub activity: f32,
    /// Seuil retenu (dBFS).
    pub threshold_db: f32,
}

/// Enveloppe P.56 : deux lissages exponentiels successifs de |x|.
fn envelope(x: &[f32]) -> Vec<f32> {
    let g = (-1.0 / (SAMPLE_RATE * TIME_CONSTANT_S)).exp();
    let (mut q, mut p) = (0f32, 0f32);
    x.iter()
        .map(|&s| {
            q = g * q + (1.0 - g) * s.abs();
            p = g * p + (1.0 - g) * q;
            p
        })
        .collect()
}

/// Masque d'activité pour un seuil donné (avec maintien).
fn activity_mask(env: &[f32], threshold: f32) -> Vec<bool> {
    let hang = (HANGOVER_S * SAMPLE_RATE) as usize;
    let mut since = hang;
    env.iter()
        .map(|&p| {
            if p >= threshold {
                since = 0;
                true
            } else if since < hang {
                since += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

fn threshold_db(j: usize) -> f32 {
    THRESHOLD_MIN_DB + j as f32 * THRESHOLD_STEP_DB
}

/// Mesure le niveau de voix active et renvoie aussi le masque par
/// échantillon. `None` si le signal est muet.
pub fn measure(x: &[f32]) -> Option<(ActiveSpeech, Vec<bool>)> {
    let energy: f64 = x.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    if x.is_empty() || energy <= 0.0 {
        return None;
    }
    let env = envelope(x);
    // Pour chaque seuil : nombre d'échantillons actifs, puis écart entre le
    // niveau actif obtenu et le seuil.
    let hang = (HANGOVER_S * SAMPLE_RATE) as usize;
    let mut counts = [0usize; THRESHOLDS];
    let mut since = [hang; THRESHOLDS];
    let lin: [f32; THRESHOLDS] = std::array::from_fn(|j| 10f32.powf(threshold_db(j) / 20.0));
    for &p in &env {
        for j in 0..THRESHOLDS {
            if p >= lin[j] {
                since[j] = 0;
                counts[j] += 1;
            } else if since[j] < hang {
                since[j] += 1;
                counts[j] += 1;
            }
        }
    }
    let level =
        |j: usize| -> Option<f32> { (counts[j] > 0).then(|| (10.0 * (energy / counts[j] as f64).log10()) as f32) };
    // L'écart niveau - seuil décroît quand le seuil monte : on cherche le
    // premier seuil où il passe sous la marge, puis on interpole.
    let mut prev: Option<(f32, f32)> = None; // (écart, niveau)
    for j in 0..THRESHOLDS {
        let Some(a) = level(j) else { break };
        let diff = a - threshold_db(j);
        if diff <= MARGIN_DB {
            let (level_db, thr) = match prev {
                Some((d0, a0)) if (d0 - diff).abs() > 1e-6 => {
                    let t = (d0 - MARGIN_DB) / (d0 - diff);
                    (a0 + t * (a - a0), threshold_db(j - 1) + t * THRESHOLD_STEP_DB)
                }
                _ => (a, threshold_db(j)),
            };
            let mask = activity_mask(&env, 10f32.powf(thr / 20.0));
            let activity = mask.iter().filter(|&&m| m).count() as f32 / x.len() as f32;
            return Some((ActiveSpeech { level_db, activity, threshold_db: thr }, mask));
        }
        prev = Some((diff, a));
    }
    // Jamais sous la marge (signal très régulier) : tout est actif.
    let level_db = (10.0 * (energy / x.len() as f64).log10()) as f32;
    Some((ActiveSpeech { level_db, activity: 1.0, threshold_db: level_db - MARGIN_DB }, vec![true; x.len()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / SAMPLE_RATE).sin()).collect()
    }

    #[test]
    fn steady_tone_level_is_its_rms() {
        let (m, mask) = measure(&tone(48_000, 0.1)).expect("son");
        assert!((m.level_db + 23.01).abs() < 0.3, "{m:?}");
        assert!(m.activity > 0.95);
        assert!(mask.iter().skip(4_800).all(|&a| a));
    }

    #[test]
    fn pauses_do_not_lower_the_active_level() {
        // 1 s de son, 2 s de silence, 1 s de son : le niveau moyen baisse de
        // 3 dB, le niveau actif doit rester celui du son.
        let mut x = tone(48_000, 0.1);
        x.extend(vec![0.0; 96_000]);
        x.extend(tone(48_000, 0.1));
        let (m, mask) = measure(&x).expect("son");
        assert!((m.level_db + 23.01).abs() < 0.6, "{m:?}");
        assert!((0.45..0.65).contains(&m.activity), "{m:?}");
        // Le maintien couvre 200 ms après la fin du son, pas plus.
        assert!(mask[48_000 + 4_800]);
        assert!(!mask[48_000 + 14_400]);
    }

    #[test]
    fn silence_is_not_measured() {
        assert!(measure(&[]).is_none());
        assert!(measure(&vec![0.0; 4_800]).is_none());
    }
}
