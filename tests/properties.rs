//! Tests par propriétés : au lieu de quelques exemples choisis à la main,
//! proptest génère des centaines de cas aléatoires (réglages et signaux) et
//! vérifie des règles qui doivent toujours tenir. En cas d'échec, il réduit
//! automatiquement le cas au plus petit exemple qui casse.

use micro_studio::dsp::resample::Resampler;
use micro_studio::dsp::util::db_to_lin;
use micro_studio::dsp::{Chain, FRAME};
use micro_studio::params::{EqBand, EqKind, Params};
use proptest::prelude::*;

fn eq_band() -> impl Strategy<Value = EqBand> {
    (
        prop_oneof![Just(EqKind::LowShelf), Just(EqKind::Peak), Just(EqKind::HighShelf)],
        20.0f32..20_000.0,
        -12.0f32..12.0,
        0.1f32..10.0,
    )
        .prop_map(|(kind, freq_hz, gain_db, q)| EqBand { kind, freq_hz, gain_db, q })
}

/// Réglages quelconques dans les plages de l'interface, et un peu au-delà.
fn params() -> impl Strategy<Value = Params> {
    (
        (any::<bool>(), -20.0f32..36.0, any::<bool>(), 20.0f32..400.0, any::<bool>(), -90.0f32..0.0, -90.0f32..0.0),
        (any::<bool>(), 0.0f32..1.0, any::<bool>(), 2000.0f32..12_000.0, -70.0f32..0.0, 0.0f32..20.0),
        (any::<bool>(), [eq_band(), eq_band(), eq_band(), eq_band()]),
        (any::<bool>(), -60.0f32..0.0, 1.0f32..30.0, 0.1f32..200.0, 5.0f32..2000.0, 0.0f32..18.0, 0.0f32..30.0),
        (any::<bool>(), -20.0f32..0.0, -20.0f32..20.0),
    )
        .prop_map(|(a, b, c, d, e)| Params {
            bypass: false,
            input_gain_db: a.1,
            hpf_enabled: a.2,
            hpf_hz: a.3,
            gate_enabled: a.4,
            gate_threshold_db: a.5,
            gate_range_db: a.6,
            denoise_enabled: b.0,
            denoise_mix: b.1,
            deesser_enabled: b.2,
            deesser_freq_hz: b.3,
            deesser_threshold_db: b.4,
            deesser_max_reduction_db: b.5,
            eq_enabled: c.0,
            eq_bands: c.1,
            comp_enabled: d.0,
            comp_threshold_db: d.1,
            comp_ratio: d.2,
            comp_attack_ms: d.3,
            comp_release_ms: d.4,
            comp_knee_db: d.5,
            comp_makeup_db: d.6,
            limiter_enabled: e.0,
            limiter_ceiling_db: e.1,
            output_gain_db: e.2,
            ..Params::default()
        })
}

fn signal() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(-1.5f32..1.5, 0..FRAME * 6)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Quels que soient les réglages et le signal : jamais de NaN ni d'infini,
    /// même durée en sortie, et le plafond du limiteur est respecté.
    #[test]
    fn chain_output_is_always_finite_and_limited(p in params(), x in signal()) {
        let (out, stats) = Chain::render(&p, &x);
        prop_assert_eq!(out.len(), x.len());
        prop_assert_eq!(stats.len(), x.len().div_ceil(FRAME));
        prop_assert!(out.iter().all(|s| s.is_finite()));
        if p.limiter_enabled {
            let ceiling = db_to_lin(p.limiter_ceiling_db);
            prop_assert!(out.iter().all(|s| s.abs() <= ceiling + 1e-6));
        }
    }

    /// Le traitement est déterministe : même entrée, même sortie.
    #[test]
    fn chain_is_deterministic(p in params(), x in signal()) {
        prop_assert_eq!(Chain::render(&p, &x).0, Chain::render(&p, &x).0);
    }

    /// Le silence reste du silence (ni souffle ni oscillation ajoutés), sauf
    /// le gain d'entrée appliqué à zéro qui reste zéro.
    #[test]
    fn silence_stays_silent(p in params()) {
        let (out, _) = Chain::render(&p, &vec![0.0; FRAME * 4]);
        prop_assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    /// N'importe quels réglages survivent à la sauvegarde JSON à l'identique.
    #[test]
    fn params_round_trip_through_json(p in params()) {
        let json = serde_json::to_string(&p).expect("sérialisation");
        let back: Params = serde_json::from_str(&json).expect("désérialisation");
        prop_assert_eq!(p, back);
    }

    /// Le rééchantillonneur produit le bon nombre d'échantillons pour tout
    /// couple de fréquences courant.
    #[test]
    fn resampler_rate_ratio(from in prop::sample::select(vec![8_000u32, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000]),
                            to in prop::sample::select(vec![44_100u32, 48_000])) {
        let mut r = Resampler::new(from, to);
        let mut n = 0usize;
        for _ in 0..from / 4 {
            r.push(0.0, |_| n += 1);
        }
        let expected = (to / 4) as usize;
        prop_assert!(n.abs_diff(expected) <= 2, "{} → {} : {} au lieu de {}", from, to, n, expected);
    }

    /// Un fichier de réglages arbitraire (texte quelconque) ne fait jamais planter le chargement.
    #[test]
    fn any_settings_text_loads_or_falls_back(text in ".{0,200}") {
        let _ = serde_json::from_str::<micro_studio::settings::Settings>(&text);
    }
}
