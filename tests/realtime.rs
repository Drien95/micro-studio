//! Règle temps réel (NASA Power of 10, règle 3 : pas d'allocation dynamique après
//! l'initialisation) : le code appelé depuis les callbacks audio ne doit jamais
//! allouer de mémoire, sinon l'allocateur du système peut bloquer le thread audio
//! et provoquer des craquements.
//!
//! Un allocateur global instrumenté compte les allocations faites par le thread
//! courant pendant la zone mesurée.

use micro_studio::dsp::resample::Resampler;
use micro_studio::dsp::{Chain, FRAME};
use micro_studio::params::{DenoiseEngine, Params, builtin_presets};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAlloc;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn note_alloc() {
    // `try_with` : l'allocateur peut être appelé pendant la destruction du thread.
    let _ = COUNTING.try_with(|c| {
        if c.get() {
            let _ = COUNT.try_with(|n| n.set(n.get() + 1));
        }
    });
}

// SAFETY : délègue entièrement à l'allocateur système, ne fait que compter.
#[allow(unsafe_code, reason = "allocateur de test uniquement, jamais dans le logiciel")]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Exécute `f` et renvoie le nombre d'allocations faites par ce thread.
fn allocations_during(f: impl FnOnce()) -> usize {
    COUNT.with(|n| n.set(0));
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    COUNT.with(Cell::get)
}

fn test_signal() -> Vec<f32> {
    let (_, speech) = micro_studio::calibrate::synthetic_voice(1.0, -50.0, -20.0);
    speech
}

/// Préréglages intégrés, plus un réglage où tout est actif (coupe-bandes et
/// porte vocale compris), comme après un calibrage.
fn all_settings() -> Vec<(&'static str, Params)> {
    let mut all = builtin_presets();
    let mut full = Params { voice_gate_enabled: true, voice_gate_confirm_ms: 50.0, ..Params::default() };
    full.notch_freqs_hz = [50.0, 100.0, 150.0, 1000.0, 2000.0, 4000.0];
    all.push(("Tout activé", full));
    all
}

#[test]
fn chain_process_does_not_allocate() {
    let input = test_signal();
    for (name, params) in all_settings() {
        let mut chain = Chain::new(&params);
        let mut frame = [0f32; FRAME];
        // Une trame de chauffe : les états internes sont créés à l'initialisation.
        chain.process(&mut frame);
        let n = allocations_during(|| {
            for chunk in input.as_chunks::<FRAME>().0 {
                frame = *chunk;
                chain.process(&mut frame);
            }
        });
        assert_eq!(n, 0, "le préréglage « {name} » alloue {n} fois dans Chain::process");
    }
}

#[test]
fn realtime_chain_with_deepfilter_does_not_allocate() {
    // DeepFilterNet alloue : il tourne dans son propre fil ; le fil audio ne
    // fait qu'échanger des trames par des canaux bornés.
    let params = Params { denoise_engine: DenoiseEngine::DeepFilter, voice_gate_enabled: true, ..Params::default() };
    let input = test_signal();
    let mut chain = Chain::new_realtime(&params);
    let mut frame = [0f32; FRAME];
    chain.process(&mut frame);
    let n = allocations_during(|| {
        for chunk in input.as_chunks::<FRAME>().0 {
            frame = *chunk;
            chain.process(&mut frame);
        }
    });
    assert_eq!(n, 0, "la chaîne temps réel avec DeepFilterNet alloue {n} fois");
    // Changer de moteur en cours de route n'alloue rien non plus.
    let n = allocations_during(|| chain.set_params(&Params::default()));
    assert_eq!(n, 0);
}

#[test]
fn set_params_does_not_allocate() {
    let presets = all_settings();
    let mut chain = Chain::new(&Params::default());
    let n = allocations_during(|| {
        for (_, p) in &presets {
            chain.set_params(p);
        }
    });
    assert_eq!(n, 0, "Chain::set_params alloue {n} fois");
}

#[test]
fn resampler_does_not_allocate() {
    let mut up = Resampler::new(44_100, 48_000);
    let mut down = Resampler::new(48_000, 44_100);
    let mut sink = 0f32;
    let n = allocations_during(|| {
        for i in 0..44_100 {
            up.push((i as f32 * 0.01).sin(), |y| sink += y);
            sink += down.pull(|| 0.1);
        }
    });
    assert!(sink.is_finite());
    assert_eq!(n, 0, "le rééchantillonneur alloue {n} fois");
}
