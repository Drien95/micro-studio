//! Calibrage automatique : résultats cohérents et aucun plantage, même avec un
//! enregistrement vide, muet ou saturé.

use micro_studio::calibrate::{analyze, propose, run, synthetic_voice};

#[test]
fn calibration_is_deterministic() {
    let (sil, sp) = synthetic_voice(4.0, -55.0, -25.0);
    let a = run(&sil, &sp, |_| {});
    let b = run(&sil, &sp, |_| {});
    assert_eq!(a.params, b.params);
}

#[test]
fn progress_is_reported_up_to_one() {
    let (sil, sp) = synthetic_voice(3.0, -55.0, -25.0);
    let mut last = 0f32;
    let r = run(&sil, &sp, |p| {
        assert!((0.0..=1.0).contains(&p));
        last = p;
    });
    assert!((last - 1.0).abs() < f32::EPSILON);
    assert_eq!(r.candidates_tested, 36);
}

#[test]
fn louder_voice_gets_less_input_gain() {
    let (sil, quiet) = synthetic_voice(4.0, -60.0, -35.0);
    let (_, loud) = synthetic_voice(4.0, -60.0, -15.0);
    let gq = propose(&analyze(&sil, &quiet)).input_gain_db;
    let gl = propose(&analyze(&sil, &loud)).input_gain_db;
    assert!(gq > gl, "voix faible {gq} dB, voix forte {gl} dB");
}

#[test]
fn degenerate_recordings_do_not_panic() {
    let silence = vec![0f32; 48_000];
    let clipped = vec![1f32; 48_000];
    for (sil, sp) in [(&[][..], &[][..]), (&silence[..], &silence[..]), (&silence[..], &clipped[..])] {
        let r = run(sil, sp, |_| {});
        assert!(r.params.input_gain_db.is_finite());
        assert!(r.params.gate_threshold_db.is_finite());
    }
}
