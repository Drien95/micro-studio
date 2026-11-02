//! Tests de régression audio : chaque préréglage est appliqué à un signal de
//! référence et son « empreinte » (niveau par tranche de 100 ms) est comparée à
//! celle enregistrée dans `tests/golden/presets.json`.
//!
//! Si un changement modifie le son, ce test échoue : soit c'est un bug, soit le
//! changement est voulu et on régénère les références avec
//! `UPDATE_GOLDEN=1 cargo test --test regression`, puis on relit le diff du
//! fichier JSON dans la pull request.

use micro_studio::calibrate::synthetic_voice;
use micro_studio::dsp::Chain;
use micro_studio::params::{Params, builtin_presets};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Écart toléré par tranche (dB) : absorbe les différences d'arrondi entre
/// processeurs et systèmes, bien en dessous de ce qu'une oreille perçoit.
const TOLERANCE_DB: f32 = 0.05;
const BLOCK: usize = 4800; // 100 ms à 48 kHz

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/presets.json")
}

fn reference_signal() -> Vec<f32> {
    let (silence, speech) = synthetic_voice(3.0, -55.0, -25.0);
    [silence, speech].concat()
}

fn fingerprint(p: &Params, input: &[f32]) -> Vec<f32> {
    let (out, _) = Chain::render(p, input);
    out.chunks(BLOCK)
        .map(|b| {
            let e = b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32;
            (10.0 * e.max(1e-14).log10() * 100.0).round() / 100.0
        })
        .collect()
}

fn current() -> BTreeMap<String, Vec<f32>> {
    let input = reference_signal();
    let mut all: BTreeMap<String, Vec<f32>> =
        builtin_presets().into_iter().map(|(name, p)| (name.to_owned(), fingerprint(&p, &input))).collect();
    all.insert("Son brut".to_owned(), fingerprint(&Params { bypass: true, ..Params::default() }, &input));
    all
}

#[test]
fn presets_sound_the_same_as_the_reference() {
    let now = current();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let json = serde_json::to_string_pretty(&now).expect("sérialisation");
        std::fs::write(golden_path(), json + "\n").expect("écriture des références");
        return;
    }
    let text = std::fs::read_to_string(golden_path())
        .expect("références absentes : lancer `UPDATE_GOLDEN=1 cargo test --test regression`");
    let golden: BTreeMap<String, Vec<f32>> = serde_json::from_str(&text).expect("références illisibles");
    assert_eq!(golden.keys().collect::<Vec<_>>(), now.keys().collect::<Vec<_>>(), "liste des préréglages changée");
    for (name, reference) in &golden {
        let got = &now[name];
        assert_eq!(got.len(), reference.len(), "{name} : durée changée");
        for (i, (a, b)) in got.iter().zip(reference).enumerate() {
            assert!(
                (a - b).abs() <= TOLERANCE_DB,
                "{name}, tranche {i} ({:.1} s) : {a} dB au lieu de {b} dB",
                i as f32 * 0.1
            );
        }
    }
}
