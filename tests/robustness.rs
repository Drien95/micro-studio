//! Fiabilité (ISO/IEC 25010 : tolérance aux fautes) : la chaîne doit rester
//! stable quelles que soient les entrées, y compris absurdes.

use micro_studio::dsp::util::db_to_lin;
use micro_studio::dsp::{Chain, FRAME};
use micro_studio::params::{Params, builtin_presets};

/// Bruit blanc déterministe dans [-amp, amp].
fn noise(len: usize, amp: f32, mut seed: u32) -> Vec<f32> {
    (0..len)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
        })
        .collect()
}

#[test]
fn every_preset_keeps_output_finite_and_below_ceiling() {
    for amp in [0.0, 1e-6, 0.01, 0.5, 1.0, 4.0] {
        let input = noise(48_000, amp, 7);
        for (name, p) in builtin_presets() {
            let (out, stats) = Chain::render(&p, &input);
            assert_eq!(out.len(), input.len());
            assert_eq!(stats.len(), input.len().div_ceil(FRAME));
            let ceiling = db_to_lin(p.limiter_ceiling_db);
            for s in &out {
                assert!(s.is_finite(), "{name}, amplitude {amp} : échantillon non fini");
                assert!(s.abs() <= ceiling + 1e-6, "{name}, amplitude {amp} : {s} dépasse le plafond");
            }
        }
    }
}

#[test]
fn chain_recovers_after_corrupted_samples() {
    // Un pilote audio défaillant peut livrer des NaN ou des infinis : ils ne doivent
    // pas « empoisonner » les filtres pour le reste de la session.
    let p = Params::default();
    let mut input = noise(48_000, 0.3, 3);
    input[1000] = f32::NAN;
    input[1001] = f32::INFINITY;
    input[1002] = f32::NEG_INFINITY;
    let (out, stats) = Chain::render(&p, &input);
    assert!(out.iter().all(|s| s.is_finite()), "un échantillon non fini a traversé la chaîne");
    assert!(stats.iter().all(|s| s.in_peak_db.is_finite() && s.out_peak_db.is_finite()));
}

#[test]
fn bypass_only_applies_input_gain() {
    let p = Params { bypass: true, input_gain_db: 6.0, ..Params::default() };
    let input = noise(4800, 0.1, 11);
    let (out, _) = Chain::render(&p, &input);
    let g = db_to_lin(6.0);
    for (x, y) in input.iter().zip(&out) {
        assert!((x * g - y).abs() < 1e-6);
    }
}

#[test]
fn render_handles_partial_and_empty_buffers() {
    let p = Params::default();
    assert!(Chain::render(&p, &[]).0.is_empty());
    let (out, stats) = Chain::render(&p, &noise(FRAME + 17, 0.2, 5));
    assert_eq!(out.len(), FRAME + 17);
    assert_eq!(stats.len(), 2);
}

#[test]
fn extreme_parameters_do_not_break_the_chain() {
    let mut p = Params {
        input_gain_db: 40.0,
        hpf_hz: 1e6,
        gate_attack_ms: 0.0,
        gate_release_ms: -5.0,
        comp_ratio: 0.0,
        comp_knee_db: -3.0,
        comp_attack_ms: 0.0,
        deesser_freq_hz: 0.0,
        limiter_ceiling_db: 3.0,
        ..Params::default()
    };
    for b in &mut p.eq_bands {
        b.q = 0.0;
        b.gain_db = 60.0;
        b.freq_hz = -1.0;
    }
    let (out, _) = Chain::render(&p, &noise(48_000, 0.5, 9));
    assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0 + 1e-6));
}
