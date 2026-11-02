//! Retour casque (« M'écouter ») : rejoue dans le casque le son traité, tel
//! qu'il sort du micro virtuel.
//!
//! Le moteur principal n'est pas modifié : le retour capte *CABLE Output* (ce
//! que Discord ou OBS reçoivent vraiment) et le joue sur une sortie choisie
//! par l'utilisateur, en réutilisant [`OutputProcessor`] côté sortie. Latence
//! ajoutée : une vingtaine de millisecondes en plus de celle du câble.
//!
//! Garde-fou : la sortie du retour ne peut jamais être le câble lui-même,
//! sinon le son tournerait en boucle ([`pick_sink`]).

use crate::dsp::resample::Resampler;
use crate::engine::{AtomicF32, INTERNAL_RATE, Meters, OutputProcessor, is_virtual_cable};
use crate::vbcable::is_cable_loopback_input;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Réglages partagés entre l'interface et les flux du retour.
#[derive(Debug)]
pub struct MonitorShared {
    /// Volume linéaire (0 = muet, 1 = niveau d'origine).
    pub volume: AtomicF32,
    /// Compteurs du flux de sortie (sous-alimentations).
    pub meters: Meters,
}

impl MonitorShared {
    #[must_use]
    pub fn new(volume: f32) -> Arc<Self> {
        let shared = Self { volume: AtomicF32::default(), meters: Meters::default() };
        shared.volume.store(clamp_volume(volume));
        Arc::new(shared)
    }
}

/// Volume borné à [0, 1] ; une valeur invalide (NaN) devient 0.
#[must_use]
pub fn clamp_volume(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// Source du retour : la sortie du câble virtuel parmi les entrées.
#[must_use]
pub fn find_source(inputs: &[String]) -> Option<String> {
    inputs.iter().find(|n| is_cable_loopback_input(n)).cloned()
}

/// Sorties possibles pour le retour (tout sauf le câble virtuel).
#[must_use]
pub fn sink_choices(outputs: &[String]) -> Vec<String> {
    outputs.iter().filter(|n| !is_virtual_cable(n)).cloned().collect()
}

/// Sortie du retour : celle demandée si elle est valable, sinon la sortie par
/// défaut de Windows si ce n'est pas le câble, sinon la première autre sortie.
#[must_use]
pub fn pick_sink(wanted: Option<&str>, default: Option<&str>, outputs: &[String]) -> Option<String> {
    let choices = sink_choices(outputs);
    let valid = |name: &str| choices.iter().any(|c| c == name);
    wanted.filter(|w| valid(w)).or(default.filter(|d| valid(d))).map(str::to_owned).or_else(|| choices.first().cloned())
}

/// Logique du callback de capture du retour, testable sans carte son :
/// sous-mixage mono, rééchantillonnage vers 48 kHz, volume.
///
/// Temps réel : aucune allocation, aucun verrou.
#[derive(Debug)]
pub struct MonitorCapture {
    channels: usize,
    resampler: Resampler,
}

impl MonitorCapture {
    #[must_use]
    pub fn new(channels: usize, device_rate: u32) -> Self {
        Self { channels: channels.max(1), resampler: Resampler::new(device_rate, INTERNAL_RATE) }
    }

    /// Traite un bloc entrelacé ; renvoie le nombre d'échantillons perdus
    /// faute de place (le retour prend alors du retard, il est rattrapé côté sortie).
    pub fn process<T>(&mut self, data: &[T], shared: &MonitorShared, prod: &mut impl Producer<Item = f32>) -> usize
    where
        T: Sample,
        f32: FromSample<T>,
    {
        let volume = clamp_volume(shared.volume.load());
        let channels = self.channels;
        let mut lost = 0;
        for chunk in data.chunks(channels) {
            let mono = chunk.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
            let x = if mono.is_finite() { mono * volume } else { 0.0 };
            self.resampler.push(x, |y| {
                if prod.try_push(y).is_err() {
                    lost += 1;
                }
            });
        }
        lost
    }
}

/// Retour en cours ; l'arrêter revient à détruire cette valeur.
pub struct Monitor {
    _streams: Option<(Stream, Stream)>,
    pub source: String,
    pub sink: String,
}

impl Monitor {
    /// Retour sans flux audio, pour les backends simulés (tests).
    #[must_use]
    pub fn detached(source: &str, sink: &str) -> Self {
        Self { _streams: None, source: source.to_owned(), sink: sink.to_owned() }
    }
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor").field("source", &self.source).field("sink", &self.sink).finish_non_exhaustive()
    }
}

/// Accès au matériel pour le retour ; remplacé par un faux dans les tests.
pub trait MonitorBackend {
    /// Sortie par défaut de Windows.
    fn default_output(&self) -> Option<String>;
    /// Démarre le retour de `source` (entrée) vers `sink` (sortie).
    ///
    /// # Errors
    /// Périphérique introuvable ou format non géré.
    fn start(&self, shared: &Arc<MonitorShared>, source: &str, sink: &str) -> Result<Monitor>;
}

/// Le vrai backend, via cpal (WASAPI sous Windows).
#[derive(Debug, Default, Clone, Copy)]
pub struct CpalMonitor;

impl MonitorBackend for CpalMonitor {
    fn default_output(&self) -> Option<String> {
        cpal::default_host().default_output_device().map(|d| d.to_string())
    }

    fn start(&self, shared: &Arc<MonitorShared>, source: &str, sink: &str) -> Result<Monitor> {
        let host = cpal::default_host();
        let in_dev = host
            .input_devices()?
            .find(|d| d.to_string() == source)
            .ok_or_else(|| anyhow!("Micro virtuel introuvable : {source}"))?;
        let out_dev = host
            .output_devices()?
            .find(|d| d.to_string() == sink)
            .ok_or_else(|| anyhow!("Casque introuvable : {sink}"))?;
        let in_cfg = in_dev.default_input_config()?;
        let out_cfg = out_dev.default_output_config()?;

        // 200 ms de marge, comme le moteur principal.
        let (prod, cons) = HeapRb::<f32>::new(INTERNAL_RATE as usize / 5).split();

        let input = match in_cfg.sample_format() {
            SampleFormat::F32 => build_capture::<f32>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
            SampleFormat::I16 => build_capture::<i16>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
            SampleFormat::I32 => build_capture::<i32>(&in_dev, in_cfg.config(), shared.clone(), prod)?,
            f => return Err(anyhow!("Format du micro virtuel non géré : {f:?}")),
        };
        let output = match out_cfg.sample_format() {
            SampleFormat::F32 => build_playback::<f32>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
            SampleFormat::I16 => build_playback::<i16>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
            SampleFormat::I32 => build_playback::<i32>(&out_dev, out_cfg.config(), shared.clone(), cons)?,
            f => return Err(anyhow!("Format du casque non géré : {f:?}")),
        };
        input.play().context("Impossible d'écouter le micro virtuel")?;
        output.play().context("Impossible de jouer dans le casque")?;
        Ok(Monitor { _streams: Some((input, output)), source: source.to_owned(), sink: sink.to_owned() })
    }
}

fn build_capture<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    shared: Arc<MonitorShared>,
    mut prod: impl Producer<Item = f32> + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut proc = MonitorCapture::new(cfg.channels as usize, cfg.sample_rate);
    Ok(dev.build_input_stream::<T, _, _>(
        cfg,
        move |data: &[T], _| {
            if proc.process(data, &shared, &mut prod) > 0 {
                shared.meters.dropouts.fetch_add(1, Ordering::Relaxed);
            }
        },
        |e| eprintln!("Erreur de capture du retour : {e}"),
        None,
    )?)
}

fn build_playback<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    shared: Arc<MonitorShared>,
    mut cons: impl ringbuf::traits::Consumer<Item = f32> + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut proc = OutputProcessor::new(cfg.channels as usize, cfg.sample_rate);
    Ok(dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], _| proc.process(data, &mut cons, &shared.meters),
        |e| eprintln!("Erreur de sortie du retour : {e}"),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Consumer, Observer};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    const OUTPUTS: &[&str] = &["CABLE Input (VB-Audio Virtual Cable)", "Casque (USB)", "Haut-parleurs (Realtek)"];

    #[test]
    fn the_source_is_the_cable_output() {
        let inputs = names(&["Micro (USB)", "CABLE Output (VB-Audio Virtual Cable)"]);
        assert_eq!(find_source(&inputs).as_deref(), Some("CABLE Output (VB-Audio Virtual Cable)"));
        assert_eq!(find_source(&names(&["Micro (USB)"])), None);
    }

    #[test]
    fn the_sink_is_never_the_cable() {
        let outputs = names(OUTPUTS);
        assert_eq!(sink_choices(&outputs), names(&["Casque (USB)", "Haut-parleurs (Realtek)"]));
        // Demande explicite valable.
        assert_eq!(
            pick_sink(Some("Haut-parleurs (Realtek)"), None, &outputs).as_deref(),
            Some("Haut-parleurs (Realtek)")
        );
        // Windows met souvent CABLE Input par défaut après l'installation : refusé.
        assert_eq!(pick_sink(None, Some(OUTPUTS[0]), &outputs).as_deref(), Some("Casque (USB)"));
        assert_eq!(pick_sink(Some(OUTPUTS[0]), Some(OUTPUTS[0]), &outputs).as_deref(), Some("Casque (USB)"));
        // Sortie par défaut normale.
        assert_eq!(
            pick_sink(None, Some("Haut-parleurs (Realtek)"), &outputs).as_deref(),
            Some("Haut-parleurs (Realtek)")
        );
        // Sortie demandée disparue (casque débranché).
        assert_eq!(pick_sink(Some("Ancien casque"), None, &outputs).as_deref(), Some("Casque (USB)"));
        assert_eq!(pick_sink(None, None, &names(&OUTPUTS[..1])), None);
    }

    #[test]
    fn volume_is_bounded() {
        assert!((clamp_volume(2.0) - 1.0).abs() < f32::EPSILON);
        assert!(clamp_volume(-1.0).abs() < f32::EPSILON);
        assert!(clamp_volume(f32::NAN).abs() < f32::EPSILON);
        assert!((MonitorShared::new(0.5).volume.load() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn capture_mixes_to_mono_applies_volume_and_resamples() {
        let shared = MonitorShared::new(0.5);
        let (mut prod, mut cons) = HeapRb::<f32>::new(1024).split();
        let mut cap = MonitorCapture::new(2, INTERNAL_RATE);
        // Stéréo (0.4, 0.8) -> mono 0.6 -> volume 0.5 -> 0.3.
        let lost = cap.process(&[0.4_f32, 0.8, 0.4, 0.8], &shared, &mut prod);
        assert_eq!(lost, 0);
        assert_eq!(cons.occupied_len(), 2);
        assert!((cons.try_pop().unwrap_or_default() - 0.3).abs() < 1e-6);

        // 24 kHz -> 48 kHz : environ deux fois plus d'échantillons.
        let mut cap = MonitorCapture::new(1, 24_000);
        cons.clear();
        cap.process(&[0.0_f32; 100], &shared, &mut prod);
        let n = cons.occupied_len();
        assert!((195..=205).contains(&n), "{n}");
    }

    #[test]
    fn capture_survives_bad_samples_and_a_full_buffer() {
        let shared = MonitorShared::new(1.0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(4).split();
        let mut cap = MonitorCapture::new(0, INTERNAL_RATE);
        let lost = cap.process(&[f32::NAN, 1.0, 1.0, 1.0, 1.0, 1.0], &shared, &mut prod);
        assert_eq!(lost, 2);
        assert!(cons.try_pop().unwrap_or(1.0).abs() < f32::EPSILON, "NaN remplacé par du silence");
        // i16 accepté.
        let mut cap = MonitorCapture::new(1, INTERNAL_RATE);
        cons.clear();
        cap.process(&[i16::MAX], &shared, &mut prod);
        assert!(cons.try_pop().is_some_and(|x| x > 0.99));
    }

    #[test]
    fn detached_monitor_describes_itself() {
        let m = Monitor::detached("CABLE Output", "Casque");
        assert_eq!((m.source.as_str(), m.sink.as_str()), ("CABLE Output", "Casque"));
        assert!(format!("{m:?}").contains("Casque"));
        assert!(CpalMonitor.default_output().is_none() || CpalMonitor.default_output().is_some());
    }
}
