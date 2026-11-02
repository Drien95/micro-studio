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

/// Le nom de la sortie VB-Cable, si elle est installée.
pub fn find_virtual_cable(outputs: &[String]) -> Option<String> {
    outputs.iter().find(|n| n.to_lowercase().contains("cable input")).cloned()
}

/// Flux audio en cours ; les arrêter revient à détruire cette valeur.
pub struct Engine {
    _input: Stream,
    _output: Stream,
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
            .find(|d| d.to_string().to_lowercase().contains("cable input"))
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
        _input: input_stream,
        _output: output_stream,
        input_name: in_dev.to_string(),
        output_name: out_dev.to_string(),
        input_rate: in_rate,
        output_rate: out_rate,
    })
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

/// Préfère 48 kHz (fréquence native de la chaîne), sinon la configuration par défaut.
fn pick_config(
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
    let channels = cfg.channels as usize;
    let mut resampler = Resampler::new(cfg.sample_rate, INTERNAL_RATE);
    let mut chain = Chain::new(&lock(&shared.params));
    let mut seen_version = 0u64;
    let mut frame = [0f32; FRAME];
    let mut filled = 0usize;
    let mut raw = [0f32; FRAME];

    let stream = dev.build_input_stream::<T, _, _>(
        cfg,
        move |data: &[T], _| {
            let v = shared.version.load(Ordering::Acquire);
            if v != seen_version
                && let Ok(p) = shared.params.try_lock()
            {
                chain.set_params(&p);
                seen_version = v;
            }
            for chunk in data.chunks(channels) {
                // Mono : moyenne des canaux.
                let mono = chunk.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                resampler.push(mono, |x| {
                    raw[filled] = x;
                    frame[filled] = x;
                    filled += 1;
                    if filled == FRAME {
                        filled = 0;
                        if let Ok(mut tap) = shared.tap.try_lock()
                            && let Some(buf) = tap.as_mut()
                            && buf.len() + FRAME <= buf.capacity()
                        {
                            buf.extend_from_slice(&raw);
                        }
                        let st = chain.process(&mut frame);
                        let m = &shared.meters;
                        m.in_peak_db.store(st.in_peak_db);
                        m.out_peak_db.store(st.out_peak_db);
                        m.gate_open.store(st.gate_open, Ordering::Relaxed);
                        m.comp_gr_db.store(st.comp_gr_db);
                        m.deess_gr_db.store(st.deess_gr_db);
                        m.vad.store(st.vad);
                        let pushed = prod.push_slice(&frame);
                        if pushed < FRAME {
                            m.dropouts.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        },
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
    let channels = cfg.channels as usize;
    let mut resampler = Resampler::new(INTERNAL_RATE, cfg.sample_rate);
    // On garde ~20 ms d'avance : assez pour absorber les irrégularités, assez peu pour la latence.
    let target = INTERNAL_RATE as usize / 50;
    let mut primed = false;

    let stream = dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], _| {
            let avail = cons.occupied_len();
            if !primed {
                if avail < target {
                    data.fill(T::EQUILIBRIUM);
                    return;
                }
                primed = true;
            }
            // Si le retard s'accumule (horloges légèrement différentes), on rattrape.
            if avail > target * 4 {
                cons.skip(avail - target);
            }
            let mut underrun = false;
            for chunk in data.chunks_mut(channels) {
                let y = resampler.pull(|| {
                    cons.try_pop().unwrap_or_else(|| {
                        underrun = true;
                        0.0
                    })
                });
                let s = T::from_sample(y);
                chunk.fill(s);
            }
            if underrun {
                primed = false;
                shared.meters.dropouts.fetch_add(1, Ordering::Relaxed);
            }
        },
        |e| eprintln!("Erreur de sortie : {e}"),
        None,
    )?;
    Ok(stream)
}
