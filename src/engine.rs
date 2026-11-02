//! Moteur audio temps réel : capture du micro, chaîne d'effets, envoi vers le
//! micro virtuel (VB-Cable par défaut).

use crate::dsp::resample::Resampler;
use crate::dsp::{Chain, FRAME};
use crate::params::Params;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub const INTERNAL_RATE: u32 = 48_000;
/// Durée maximale d'un enregistrement de calibrage (secondes).
const TAP_CAPACITY: usize = INTERNAL_RATE as usize * 120;

/// Flottant 32 bits atomique (stocké sous forme de bits dans un `AtomicU32`).
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Vumètres et compteurs publiés par le thread audio, lus par l'interface.
#[derive(Debug, Default)]
pub struct Meters {
    pub in_peak_db: AtomicF32,
    pub out_peak_db: AtomicF32,
    pub gate_open: AtomicBool,
    pub comp_gr_db: AtomicF32,
    pub deess_gr_db: AtomicF32,
    pub vad: AtomicF32,
    pub dropouts: AtomicU32,
}

/// État partagé entre l'interface et les threads audio.
#[derive(Debug)]
pub struct Shared {
    params: Mutex<Params>,
    version: AtomicU64,
    pub meters: Meters,
    /// Quand `Some`, le signal brut du micro (48 kHz mono, avant tout gain) y est enregistré.
    tap: Mutex<Option<Vec<f32>>>,
}

impl Shared {
    pub fn new(params: Params) -> Arc<Self> {
        Arc::new(Self {
            params: Mutex::new(params),
            version: AtomicU64::new(1),
            meters: Meters::default(),
            tap: Mutex::new(None),
        })
    }

    pub fn set_params(&self, p: &Params) {
        *lock(&self.params) = p.clone();
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn start_recording(&self) {
        *lock(&self.tap) = Some(Vec::with_capacity(TAP_CAPACITY));
    }

    pub fn stop_recording(&self) -> Vec<f32> {
        lock(&self.tap).take().unwrap_or_default()
    }
}

/// Verrouille un `Mutex` côté interface en ignorant l'empoisonnement : les données
/// protégées (réglages, enregistrement) restent valides même si un autre thread a
/// paniqué en les tenant. Ne jamais l'appeler depuis un callback audio (bloquant).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Noms des périphériques d'entrée et de sortie disponibles.
#[derive(Debug, Clone, Default)]
pub struct DeviceLists {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub fn list_devices() -> DeviceLists {
    let host = cpal::default_host();
    let names = |it: Result<_, _>| -> Vec<String> {
        match it {
            Ok(devs) => {
                let devs: Vec<cpal::Device> = devs;
                devs.iter().map(std::string::ToString::to_string).collect()
            }
            Err(_) => Vec::new(),
        }
    };
    DeviceLists {
        inputs: names(host.input_devices().map(|d| d.collect())),
        outputs: names(host.output_devices().map(|d| d.collect())),
    }
}

/// Vrai si ce nom de sortie est l'entrée du câble virtuel VB-Cable.
pub fn is_virtual_cable(name: &str) -> bool {
    name.to_lowercase().contains("cable input")
}

/// Le nom de la sortie VB-Cable, si elle est installée.
pub fn find_virtual_cable(outputs: &[String]) -> Option<String> {
    outputs.iter().find(|n| is_virtual_cable(n)).cloned()
}

/// Accès au matériel audio. L'interface ne passe que par ce trait : les tests
/// lui substituent un faux périphérique et n'ont pas besoin de carte son.
pub trait AudioBackend {
    /// Liste les périphériques d'entrée et de sortie.
    fn list_devices(&self) -> DeviceLists;
    /// Démarre capture et sortie ; `None` = choix automatique.
    fn start(&self, shared: &Arc<Shared>, input: Option<&str>, output: Option<&str>) -> Result<Engine>;
}

/// Le vrai backend, via cpal (WASAPI sous Windows).
#[derive(Debug, Default, Clone, Copy)]
pub struct CpalBackend;

impl AudioBackend for CpalBackend {
    fn list_devices(&self) -> DeviceLists {
        list_devices()
    }

    fn start(&self, shared: &Arc<Shared>, input: Option<&str>, output: Option<&str>) -> Result<Engine> {
        start(shared, input, output)
    }
}

/// Flux audio en cours ; les arrêter revient à détruire cette valeur.
pub struct Engine {
    _streams: Option<(Stream, Stream)>,
    pub input_name: String,
    pub output_name: String,
    pub input_rate: u32,
    pub output_rate: u32,
}

pub fn start(shared: &Arc<Shared>, input: Option<&str>, output: Option<&str>) -> Result<Engine> {
    let host = cpal::default_host();
    let in_dev = match input {
        Some(name) => host
            .input_devices()?
            .find(|d| d.to_string() == name)
            .ok_or_else(|| anyhow!("Micro introuvable : {name}"))?,
        None => host.default_input_device().ok_or_else(|| anyhow!("Aucun micro détecté"))?,
    };
    let out_dev = if let Some(name) = output {
        host.output_devices()?.find(|d| d.to_string() == name).ok_or_else(|| anyhow!("Sortie introuvable : {name}"))?
    } else {
        let outs: Vec<cpal::Device> = host.output_devices()?.collect();
        outs.into_iter()
            .find(|d| is_virtual_cable(&d.to_string()))
            .ok_or_else(|| anyhow!("VB-Cable n'est pas installé : choisis une sortie ou installe VB-Cable"))?
    };

    let in_cfg = pick_config(&in_dev.supported_input_configs()?.collect::<Vec<_>>(), in_dev.default_input_config()?);
    let out_cfg =
        pick_config(&out_dev.supported_output_configs()?.collect::<Vec<_>>(), out_dev.default_output_config()?);

    // 200 ms de marge dans le tampon entre capture et sortie.
    let rb = HeapRb::<f32>::new(INTERNAL_RATE as usize / 5);
    let (prod, cons) = rb.split();

    let in_rate = in_cfg.sample_rate();
    let out_rate = out_cfg.sample_rate();

    let input_stream = match in_cfg.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
        SampleFormat::I16 => build_input::<i16>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
        SampleFormat::I32 => build_input::<i32>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
        SampleFormat::U16 => build_input::<u16>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
        f => return Err(anyhow!("Format d'entrée non géré : {f:?}")),
    };
    let output_stream = match out_cfg.sample_format() {
        SampleFormat::F32 => build_output::<f32>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
        SampleFormat::I16 => build_output::<i16>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
        SampleFormat::I32 => build_output::<i32>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
        SampleFormat::U16 => build_output::<u16>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
        f => return Err(anyhow!("Format de sortie non géré : {f:?}")),
    };
    input_stream.play().context("Impossible de démarrer la capture du micro")?;
    output_stream.play().context("Impossible de démarrer la sortie")?;

    Ok(Engine {
        _streams: Some((input_stream, output_stream)),
        input_name: in_dev.to_string(),
        output_name: out_dev.to_string(),
        input_rate: in_rate,
        output_rate: out_rate,
    })
}

impl Engine {
    /// Moteur sans flux audio, pour les backends simulés (tests).
    pub fn detached(input_name: &str, output_name: &str, input_rate: u32, output_rate: u32) -> Self {
        Self {
            _streams: None,
            input_name: input_name.to_owned(),
            output_name: output_name.to_owned(),
            input_rate,
            output_rate,
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("input_name", &self.input_name)
            .field("output_name", &self.output_name)
            .field("input_rate", &self.input_rate)
            .field("output_rate", &self.output_rate)
            .finish_non_exhaustive()
    }
}

/// Préfère 48 kHz (fréquence native de la chaîne), en mono ou stéréo, dans le
/// format le plus précis (f32 > i16 > i32 > u16) ; sinon la configuration par défaut.
pub fn pick_config(
    ranges: &[cpal::SupportedStreamConfigRange],
    default: cpal::SupportedStreamConfig,
) -> cpal::SupportedStreamConfig {
    let wanted = [SampleFormat::F32, SampleFormat::I16, SampleFormat::I32, SampleFormat::U16];
    for fmt in wanted {
        for r in ranges {
            if r.sample_format() == fmt
                && r.channels() <= 2
                && let Some(c) = (*r).try_with_sample_rate(INTERNAL_RATE)
            {
                return c;
            }
        }
    }
    default
}

/// Marge visée dans le tampon entre capture et sortie (~20 ms) : assez pour
/// absorber les irrégularités des pilotes, assez peu pour la latence.
pub const OUTPUT_TARGET_FILL: usize = INTERNAL_RATE as usize / 50;

/// Logique du callback de capture, indépendante de cpal pour être testable :
/// sous-mixage mono, rééchantillonnage vers 48 kHz, découpage en trames de
/// 10 ms, enregistrement de calibrage, traitement, vumètres et envoi dans le
/// tampon vers la sortie.
///
/// Temps réel : aucune allocation, aucun verrou bloquant (`try_lock` seulement).
#[derive(Debug)]
pub struct InputProcessor {
    channels: usize,
    resampler: Resampler,
    chain: Chain,
    seen_version: u64,
    frame: [f32; FRAME],
    raw: [f32; FRAME],
    filled: usize,
}

impl InputProcessor {
    /// `channels` est borné à 1 minimum ; `device_rate` est la fréquence du micro.
    pub fn new(channels: usize, device_rate: u32, params: &Params) -> Self {
        Self {
            channels: channels.max(1),
            resampler: Resampler::new(device_rate, INTERNAL_RATE),
            chain: Chain::new(params),
            seen_version: 0,
            frame: [0.0; FRAME],
            raw: [0.0; FRAME],
            filled: 0,
        }
    }

    /// Traite un bloc entrelacé livré par le pilote.
    pub fn process<T>(&mut self, data: &[T], shared: &Shared, prod: &mut impl Producer<Item = f32>)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        let v = shared.version.load(Ordering::Acquire);
        if v != self.seen_version
            && let Ok(p) = shared.params.try_lock()
        {
            self.chain.set_params(&p);
            self.seen_version = v;
        }
        let channels = self.channels;
        for chunk in data.chunks(channels) {
            // Mono : moyenne des canaux.
            let mono = chunk.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
            let (frame, raw, filled, chain) = (&mut self.frame, &mut self.raw, &mut self.filled, &mut self.chain);
            self.resampler.push(mono, |x| {
                raw[*filled] = x;
                frame[*filled] = x;
                *filled += 1;
                if *filled == FRAME {
                    *filled = 0;
                    Self::frame_ready(chain, frame, raw, shared, prod);
                }
            });
        }
    }

    fn frame_ready(
        chain: &mut Chain,
        frame: &mut [f32; FRAME],
        raw: &[f32; FRAME],
        shared: &Shared,
        prod: &mut impl Producer<Item = f32>,
    ) {
        if let Ok(mut tap) = shared.tap.try_lock()
            && let Some(buf) = tap.as_mut()
            && buf.len() + FRAME <= buf.capacity()
        {
            buf.extend_from_slice(raw);
        }
        let st = chain.process(frame);
        let m = &shared.meters;
        m.in_peak_db.store(st.in_peak_db);
        m.out_peak_db.store(st.out_peak_db);
        m.gate_open.store(st.gate_open, Ordering::Relaxed);
        m.comp_gr_db.store(st.comp_gr_db);
        m.deess_gr_db.store(st.deess_gr_db);
        m.vad.store(st.vad);
        let pushed = prod.push_slice(frame);
        if pushed < FRAME {
            m.dropouts.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Logique du callback de sortie, indépendante de cpal pour être testable :
/// amorçage du tampon, rattrapage de la dérive d'horloge, rééchantillonnage
/// vers la fréquence de la sortie, recopie sur tous les canaux et détection
/// des sous-alimentations.
///
/// Temps réel : aucune allocation, aucun verrou.
#[derive(Debug)]
pub struct OutputProcessor {
    channels: usize,
    resampler: Resampler,
    target: usize,
    primed: bool,
}

impl OutputProcessor {
    pub fn new(channels: usize, device_rate: u32) -> Self {
        Self {
            channels: channels.max(1),
            resampler: Resampler::new(INTERNAL_RATE, device_rate),
            target: OUTPUT_TARGET_FILL,
            primed: false,
        }
    }

    /// Vrai une fois le tampon amorcé (le son sort).
    pub fn is_primed(&self) -> bool {
        self.primed
    }

    /// Remplit un bloc entrelacé demandé par le pilote.
    pub fn process<T>(&mut self, data: &mut [T], cons: &mut impl Consumer<Item = f32>, meters: &Meters)
    where
        T: Sample + FromSample<f32>,
    {
        let avail = cons.occupied_len();
        if !self.primed {
            if avail < self.target {
                data.fill(T::EQUILIBRIUM);
                return;
            }
            self.primed = true;
        }
        // Si le retard s'accumule (horloges légèrement différentes), on rattrape.
        if avail > self.target * 4 {
            cons.skip(avail - self.target);
        }
        let mut underrun = false;
        for chunk in data.chunks_mut(self.channels) {
            let y = self.resampler.pull(|| {
                cons.try_pop().unwrap_or_else(|| {
                    underrun = true;
                    0.0
                })
            });
            chunk.fill(T::from_sample(y));
        }
        if underrun {
            self.primed = false;
            meters.dropouts.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn build_input<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    shared: Arc<Shared>,
    mut prod: impl Producer<Item = f32> + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut proc = InputProcessor::new(cfg.channels as usize, cfg.sample_rate, &lock(&shared.params));
    let stream = dev.build_input_stream::<T, _, _>(
        cfg,
        move |data: &[T], _| proc.process(data, &shared, &mut prod),
        |e| eprintln!("Erreur de capture : {e}"),
        None,
    )?;
    Ok(stream)
}

fn build_output<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    shared: Arc<Shared>,
    mut cons: impl Consumer<Item = f32> + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut proc = OutputProcessor::new(cfg.channels as usize, cfg.sample_rate);
    let stream = dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], _| proc.process(data, &mut cons, &shared.meters),
        |e| eprintln!("Erreur de sortie : {e}"),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::{SupportedBufferSize, SupportedStreamConfigRange};
    use ringbuf::traits::Observer;

    fn shared() -> Arc<Shared> {
        // Chaîne neutre : on teste ici le transport, pas les effets.
        Shared::new(Params { bypass: true, ..Params::default() })
    }

    #[test]
    fn atomic_f32_round_trips() {
        let a = AtomicF32::default();
        assert!(a.load().abs() < f32::EPSILON);
        a.store(-12.5);
        assert!((a.load() + 12.5).abs() < f32::EPSILON);
    }

    #[test]
    fn virtual_cable_is_detected_by_name() {
        assert!(is_virtual_cable("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(!is_virtual_cable("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(!is_virtual_cable("Haut-parleurs (Realtek)"));
        let outs = vec!["Haut-parleurs".to_owned(), "CABLE Input (VB-Audio)".to_owned()];
        assert_eq!(find_virtual_cable(&outs).as_deref(), Some("CABLE Input (VB-Audio)"));
        assert_eq!(find_virtual_cable(&outs[..1]), None);
    }

    #[test]
    fn recording_tap_keeps_raw_input() {
        let sh = shared();
        assert!(sh.stop_recording().is_empty(), "rien n'est enregistré par défaut");
        sh.start_recording();
        let (mut prod, _cons) = HeapRb::<f32>::new(FRAME * 8).split();
        let mut input = InputProcessor::new(1, INTERNAL_RATE, &Params::default());
        input.process(&[0.25f32; FRAME * 3], &sh, &mut prod);
        let rec = sh.stop_recording();
        assert_eq!(rec.len(), FRAME * 3);
        assert!(rec.iter().all(|&x| (x - 0.25).abs() < 1e-6));
        assert!(sh.stop_recording().is_empty(), "l'enregistrement est rendu une seule fois");
    }

    #[test]
    fn poisoned_lock_is_recovered() {
        let sh = shared();
        let sh2 = Arc::clone(&sh);
        let _ = std::thread::spawn(move || {
            let _guard = sh2.params.lock();
            panic!("panique volontaire pendant que le verrou est tenu");
        })
        .join();
        assert!(sh.params.is_poisoned());
        sh.set_params(&Params::default()); // ne doit pas paniquer
        sh.start_recording();
        assert!(sh.stop_recording().is_empty());
    }

    #[test]
    fn input_downmixes_stereo_and_emits_whole_frames() {
        let sh = shared();
        let (mut prod, mut cons) = HeapRb::<f32>::new(FRAME * 8).split();
        let mut input = InputProcessor::new(2, INTERNAL_RATE, &Params { bypass: true, ..Params::default() });
        // Gauche 0,5, droite 0,1 → mono 0,3. Une trame et demie.
        let block: Vec<f32> = (0..FRAME * 3 / 2).flat_map(|_| [0.5, 0.1]).collect();
        input.process(&block, &sh, &mut prod);
        assert_eq!(cons.occupied_len(), FRAME, "seules les trames complètes partent");
        assert!(cons.pop_iter().all(|x| (x - 0.3).abs() < 1e-6));
        input.process(&block[..FRAME], &sh, &mut prod); // complète la 2e trame
        assert_eq!(cons.occupied_len(), FRAME);
    }

    #[test]
    fn input_converts_integer_samples() {
        let sh = shared();
        let (mut prod, mut cons) = HeapRb::<f32>::new(FRAME * 2).split();
        let mut input = InputProcessor::new(1, INTERNAL_RATE, &Params { bypass: true, ..Params::default() });
        input.process(&[i16::MAX / 2; FRAME], &sh, &mut prod);
        assert!(cons.pop_iter().all(|x| (x - 0.5).abs() < 1e-3));
    }

    #[test]
    fn input_resamples_44k1_to_48k() {
        let sh = shared();
        let (mut prod, cons) = HeapRb::<f32>::new(INTERNAL_RATE as usize * 2).split();
        let mut input = InputProcessor::new(1, 44_100, &Params { bypass: true, ..Params::default() });
        input.process(&vec![0.1f32; 44_100], &sh, &mut prod);
        let produced = cons.occupied_len();
        assert!(produced.abs_diff(48_000) <= FRAME, "{produced} échantillons pour 1 s");
    }

    #[test]
    fn input_counts_dropouts_when_output_is_stalled() {
        let sh = shared();
        let (mut prod, _cons) = HeapRb::<f32>::new(FRAME).split();
        let mut input = InputProcessor::new(1, INTERNAL_RATE, &Params::default());
        input.process(&[0.0f32; FRAME * 3], &sh, &mut prod);
        assert_eq!(sh.meters.dropouts.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn input_applies_new_params_and_updates_meters() {
        let sh = shared();
        let (mut prod, mut cons) = HeapRb::<f32>::new(FRAME * 4).split();
        let mut input = InputProcessor::new(1, INTERNAL_RATE, &Params { bypass: true, ..Params::default() });
        sh.set_params(&Params { bypass: true, input_gain_db: 6.0, ..Params::default() });
        input.process(&[0.25f32; FRAME], &sh, &mut prod);
        assert!(cons.pop_iter().all(|x| (x - 0.25 * 1.995_262).abs() < 1e-4), "gain de +6 dB appliqué");
        let peak = sh.meters.in_peak_db.load();
        assert!((peak - (-12.04 + 6.0)).abs() < 0.05, "vumètre d'entrée : {peak}");
    }

    #[test]
    fn output_stays_silent_until_primed() {
        let meters = Meters::default();
        let (mut prod, mut cons) = HeapRb::<f32>::new(OUTPUT_TARGET_FILL * 8).split();
        let mut out = OutputProcessor::new(2, INTERNAL_RATE);
        prod.push_slice(&[0.5; 100]);
        let mut buf = [1.0f32; 64];
        out.process(&mut buf, &mut cons, &meters);
        assert!(buf.iter().all(|&x| x == 0.0), "silence pendant l'amorçage");
        assert!(!out.is_primed());
        assert_eq!(cons.occupied_len(), 100, "rien n'est consommé avant l'amorçage");
    }

    #[test]
    fn output_copies_mono_to_every_channel() {
        let meters = Meters::default();
        let (mut prod, mut cons) = HeapRb::<f32>::new(OUTPUT_TARGET_FILL * 8).split();
        let mut out = OutputProcessor::new(2, INTERNAL_RATE);
        let ramp: Vec<f32> = (0..OUTPUT_TARGET_FILL).map(|i| i as f32 / 10_000.0).collect();
        prod.push_slice(&ramp);
        let mut buf = [0i16; 20];
        out.process(&mut buf, &mut cons, &meters);
        assert!(out.is_primed());
        for (i, pair) in buf.chunks(2).enumerate() {
            assert_eq!(pair[0], pair[1], "les deux canaux sont identiques");
            assert_eq!(pair[0], i16::from_sample(ramp[i]));
        }
        assert_eq!(meters.dropouts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn output_underrun_is_counted_and_reprimes() {
        let meters = Meters::default();
        let (mut prod, mut cons) = HeapRb::<f32>::new(OUTPUT_TARGET_FILL * 8).split();
        let mut out = OutputProcessor::new(1, INTERNAL_RATE);
        prod.push_slice(&vec![0.2; OUTPUT_TARGET_FILL]);
        let mut buf = vec![0f32; OUTPUT_TARGET_FILL + 10];
        out.process(&mut buf, &mut cons, &meters);
        assert_eq!(meters.dropouts.load(Ordering::Relaxed), 1);
        assert!(!out.is_primed(), "après une coupure, on réamorce le tampon");
        assert!(buf[OUTPUT_TARGET_FILL..].iter().all(|&x| x == 0.0));
    }

    #[test]
    fn output_catches_up_when_latency_drifts() {
        let meters = Meters::default();
        let (mut prod, mut cons) = HeapRb::<f32>::new(OUTPUT_TARGET_FILL * 8).split();
        let mut out = OutputProcessor::new(1, INTERNAL_RATE);
        prod.push_slice(&vec![0.1; OUTPUT_TARGET_FILL * 5]);
        let mut buf = [0f32; 10];
        out.process(&mut buf, &mut cons, &meters);
        assert_eq!(cons.occupied_len(), OUTPUT_TARGET_FILL - 10, "retard ramené à la cible");
    }

    fn range(fmt: SampleFormat, channels: u16, min: u32, max: u32) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, min, max, SupportedBufferSize::Unknown, fmt)
    }

    #[test]
    fn pick_config_prefers_48k_float() {
        let default = cpal::SupportedStreamConfig::new(2, 44_100, SupportedBufferSize::Unknown, SampleFormat::I16);
        let ranges = [
            range(SampleFormat::I16, 2, 8_000, 96_000),
            range(SampleFormat::F32, 8, 48_000, 48_000), // trop de canaux
            range(SampleFormat::F32, 1, 44_100, 48_000),
        ];
        let c = pick_config(&ranges, default);
        assert_eq!((c.sample_format(), c.sample_rate(), c.channels()), (SampleFormat::F32, 48_000, 1));

        let only_i16 = [range(SampleFormat::I16, 2, 8_000, 96_000)];
        let c = pick_config(&only_i16, default);
        assert_eq!((c.sample_format(), c.sample_rate()), (SampleFormat::I16, 48_000));

        let no_48k = [range(SampleFormat::F32, 2, 44_100, 44_100)];
        let c = pick_config(&no_48k, default);
        assert_eq!((c.sample_format(), c.sample_rate()), (SampleFormat::I16, 44_100), "repli sur le défaut");
    }

    #[test]
    fn detached_engine_reports_its_devices() {
        let e = Engine::detached("Micro", "CABLE Input", 48_000, 44_100);
        let dbg = format!("{e:?}");
        assert!(dbg.contains("Micro") && dbg.contains("44100"));
    }
}
