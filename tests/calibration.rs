//! Calibrage automatique : résultats cohérents et aucun plantage, même avec un
//! enregistrement vide, muet ou saturé.

use micro_studio::calibrate::{Recording, analyze, propose, refinements, run, synthetic_keyboard, synthetic_voice};
use micro_studio::params::Params;

fn recording(secs: f32, noise_db: f32, voice_db: f32) -> Recording {
    let (silence, speech) = synthetic_voice(secs, noise_db, voice_db);
    let keyboard = synthetic_keyboard(silence.len(), noise_db, voice_db + 6.0);
    Recording { silence, keyboard, speech }
}

#[test]
fn calibration_is_deterministic() {
    let rec = recording(4.0, -55.0, -25.0);
    let a = run(&rec, |_| {});
    let b = run(&rec, |_| {});
    assert_eq!(a.params, b.params);
}

#[test]
fn progress_is_reported_up_to_one() {
    let rec = recording(3.0, -55.0, -25.0);
    let mut last = 0f32;
    let r = run(&rec, |p| {
        assert!((0.0..=1.0).contains(&p));
        last = p;
    });
    assert!((last - 1.0).abs() < f32::EPSILON);
    // 54 variantes RNNoise + 6 finalistes DeepFilterNet + 3 tours d'affinage.
    assert!((61..=120).contains(&r.candidates_tested), "{} réglages", r.candidates_tested);
    // Les meilleurs réglages et la référence sont notés par DNSMOS.
    for m in [r.score.mos, r.baseline_score.mos] {
        let m = m.expect("note DNSMOS");
        for v in [m.sig, m.bak, m.ovrl] {
            assert!(v.is_finite() && (0.5..=5.5).contains(&v), "{m:?}");
        }
    }
}

#[test]
fn louder_voice_gets_less_input_gain() {
    let (sil, quiet) = synthetic_voice(4.0, -60.0, -35.0);
    let (_, loud) = synthetic_voice(4.0, -60.0, -15.0);
    let gq = propose(&analyze(&sil, &sil, &quiet)).input_gain_db;
    let gl = propose(&analyze(&sil, &sil, &loud)).input_gain_db;
    assert!(gq > gl, "voix faible {gq} dB, voix forte {gl} dB");
}

#[test]
fn degenerate_recordings_do_not_panic() {
    let silence = vec![0f32; 48_000];
    let clipped = vec![1f32; 48_000];
    let cases = [
        Recording::default(),
        Recording { silence: silence.clone(), keyboard: silence.clone(), speech: silence.clone() },
        Recording { silence: silence.clone(), keyboard: Vec::new(), speech: clipped.clone() },
        Recording { silence: Vec::new(), keyboard: clipped.clone(), speech: clipped },
    ];
    for rec in &cases {
        let r = run(rec, |_| {});
        assert!(r.params.input_gain_db.is_finite());
        assert!(r.params.gate_threshold_db.is_finite());
        assert!(r.params.notch_freqs_hz.iter().all(|f| f.is_finite()));
        assert!(r.score.loudness_lufs.is_finite());
    }
}

#[test]
fn refinements_nudge_one_setting_at_a_time() {
    let p = Params::default();
    let r = refinements(&p, 1.0);
    assert_eq!(r.len(), 15, "8 réglages fins dans les deux sens, sauf le débruitage déjà au maximum");
    assert!(r.iter().all(|q| *q != p));
    let with_voice = Params { voice_gate_enabled: true, ..Params::default() };
    assert_eq!(refinements(&with_voice, 1.0).len(), 19, "+ seuil et maintien de la porte vocale");
    // Un pas plus petit retouche moins.
    let small = refinements(&p, 0.25);
    assert!(
        (small[0].gate_threshold_db - p.gate_threshold_db).abs() < (r[0].gate_threshold_db - p.gate_threshold_db).abs()
    );
    // Les bornes sont respectées.
    assert!(r.iter().all(|q| (0.5..=1.0).contains(&q.denoise_mix)));
    let half = Params { denoise_mix: 0.7, ..Params::default() };
    assert_eq!(refinements(&half, 1.0).len(), 16);
}
