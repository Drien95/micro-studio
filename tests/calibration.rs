//! Calibrage automatique : résultats cohérents et aucun plantage, même avec un
//! enregistrement vide, muet ou saturé.

use micro_studio::calibrate::{Recording, analyze, propose, run, synthetic_keyboard, synthetic_voice};

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
    assert_eq!(r.candidates_tested, 60, "54 variantes RNNoise + 6 finalistes DeepFilterNet");
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
