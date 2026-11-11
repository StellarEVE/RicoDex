//! RicoDex audio analysis crate: song analysis. Decodes songs and detects beats, bars, phrases
//! and change points, exposing them to the engine as plain data.
//!
//! Decoding is pure Rust through Symphonia, in process, at the file native
//! sample rate (no resampling stage, so no quality loss and no resampler
//! dependency). Beat detection follows the approach proven in the previous
//! project: multiband spectral flux, where the spectrum is split into
//! bands, each band contributes the positive change in its magnitude
//! spectrum, and the bands are combined after normalizing each by its own
//! mean level, so a quiet hi hat counts alongside a loud kick. Peak picking
//! on the combined envelope is adaptive, and every accepted peak carries a
//! strength. Tempo comes from autocorrelating the onset envelope, refined
//! by parabolic interpolation around the best lag.
//!
//! Everything this crate publishes is in [`Tick`] time. Detected events
//! sit on exact sample positions, and sample positions convert to ticks
//! exactly whenever the sample rate divides the tick rate, which the
//! standard 44,100 and 48,000 rates (and their halves) all do. This crate
//! depends on RicoDex core crate for time only; bridging analysis into the project
//! model is the engine crate's job.

use std::fmt;
use std::fs::File;
use std::path::Path;

use num_complex::Complex;
use ricodex_core::{Tick, TICKS_PER_SECOND};
use realfft::RealFftPlanner;

// ------------------------------------------------------------------ errors

#[derive(Debug)]
pub enum AudioError {
    Message(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Message(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AudioError {}

pub type Result<T> = std::result::Result<T, AudioError>;

fn msg<T>(m: impl Into<String>) -> Result<T> {
    Err(AudioError::Message(m.into()))
}

// ------------------------------------------------------------- time helper

/// Convert an exact sample position to ticks. Uses the exact rational
/// conversion when the sample count lands on whole ticks, and rounds to
/// the nearest tick otherwise (only possible for sample rates that do not
/// divide the tick rate).
pub fn samples_to_tick(samples: i64, sample_rate: u32) -> Tick {
    if let Some(tick) = Tick::from_samples(samples, sample_rate) {
        return tick;
    }
    if sample_rate == 0 {
        return Tick::ZERO;
    }
    let numerator = (samples as i128) * (TICKS_PER_SECOND as i128);
    let denominator = sample_rate as i128;
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = if 2 * remainder.abs() >= denominator {
        quotient + numerator.signum()
    } else {
        quotient
    };
    Tick::from_ticks(rounded as i64)
}

// ---------------------------------------------------------------- decoding

/// A decoded audio file: mono f32 samples at the native sample rate.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl DecodedAudio {
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.samples.len() as f64 / self.sample_rate as f64
        }
    }

    pub fn duration(&self) -> Tick {
        samples_to_tick(self.samples.len() as i64, self.sample_rate)
    }
}

/// Decode any audio file Symphonia understands into mono f32 samples.
/// Stereo and multichannel material is averaged down to mono.
pub fn decode_file(path: &Path) -> Result<DecodedAudio> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = File::open(path)
        .map_err(|e| AudioError::Message(format!("cannot open {}: {e}", path.display())))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| AudioError::Message(format!("cannot probe {}: {e}", path.display())))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Message(format!("no audio track in {}", path.display())))?
        .clone();

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioError::Message(format!("no decoder for {}: {e}", path.display())))?;

    let mut out: Vec<f32> = Vec::new();
    let mut sample_rate = 0u32;
    let mut channels = 0usize;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymError::IoError(_)) => break,
            Err(e) => return msg(format!("read error in {}: {e}", path.display())),
        };
        if packet.track_id() != track.id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                if sample_buf.is_none() {
                    sample_rate = spec.rate;
                    channels = spec.channels.count();
                    sample_buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                }
                let buf = sample_buf.as_mut().unwrap();
                buf.copy_interleaved_ref(decoded);
                let interleaved = buf.samples();
                if channels <= 1 {
                    out.extend_from_slice(interleaved);
                } else {
                    for frame in interleaved.chunks_exact(channels) {
                        out.push(frame.iter().sum::<f32>() / channels as f32);
                    }
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return msg(format!("decode error in {}: {e}", path.display())),
        }
    }

    if out.is_empty() || sample_rate == 0 {
        return msg(format!("no audio decoded from {}", path.display()));
    }
    Ok(DecodedAudio {
        samples: out,
        sample_rate,
        channels: channels as u16,
    })
}

// --------------------------------------------------------- analysis output

/// One detected onset: an exact time and a strength in 0.0..=1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    pub time: Tick,
    pub strength: f32,
}

/// One beat of the regular tempo grid, with the strength of the onset
/// evidence near it (a modest default where the music has no onset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    pub time: Tick,
    pub strength: f32,
}

/// One bar of the musical grid: its start, how many beats it holds, and
/// the strength of the strongest beat inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub start: Tick,
    pub beats: u32,
    pub strength: f32,
}

/// A phrase: a run of consecutive bars, usually four, with the strongest
/// bar strength of the group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phrase {
    pub start: Tick,
    pub bars: u32,
    pub strength: f32,
}

/// One change point: a time where the loudness level of the music shifts,
/// and the signed size of the shift in dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangePoint {
    pub time: Tick,
    pub delta_db: f32,
}

/// The complete analysis of one song, as plain data in tick time.
#[derive(Debug, Clone, PartialEq)]
pub struct SongAnalysis {
    pub duration: Tick,
    pub sample_rate: u32,
    /// Estimated tempo in beats per minute; zero when no tempo could be
    /// estimated (silence, or material too short to judge).
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub onsets: Vec<Onset>,
    pub beats: Vec<Beat>,
    pub bars: Vec<Bar>,
    pub phrases: Vec<Phrase>,
    pub change_points: Vec<ChangePoint>,
}

/// Knobs for one analysis run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisSettings {
    pub beats_per_bar: u32,
    /// Onset sensitivity: multiplies the local mean threshold in peak
    /// picking. Lower finds more onsets. 1.4 is the proven default.
    pub sensitivity: f64,
    /// Loudness step, in dB, that counts as a change point.
    pub change_threshold_db: f32,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        AnalysisSettings {
            beats_per_bar: 4,
            sensitivity: 1.4,
            change_threshold_db: 4.0,
        }
    }
}

// ----------------------------------------------------------------- onsets

/// Frequency band edges in Hz for the flux split: sub and kick, body,
/// presence, and air (hats and cymbals).
const BAND_EDGES: [f64; 3] = [150.0, 1500.0, 6000.0];

fn fft_size_for(sample_rate: u32) -> usize {
    // About 46 ms of audio per frame, as a power of two.
    let target = (sample_rate as f64 * 0.046) as usize;
    let mut size = 512usize;
    while size < target && size < 8192 {
        size *= 2;
    }
    size
}

/// The combined onset envelope, one value per hop, plus the hop size in
/// samples. Exposed so tempo estimation and tests share it.
pub fn onset_envelope(samples: &[f32], sample_rate: u32) -> (Vec<f32>, usize) {
    let win = fft_size_for(sample_rate);
    let hop = win / 4;
    if samples.len() < win * 2 {
        return (Vec::new(), hop);
    }
    let frames = (samples.len() - win) / hop + 1;
    let bins = win / 2 + 1;

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(win);
    let mut scratch = fft.make_scratch_vec();
    let mut input = vec![0f32; win];
    let mut spectrum = vec![Complex::default(); bins];
    let window: Vec<f32> = (0..win)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (win - 1) as f32).cos())
        .collect();

    let band_of_bin: Vec<usize> = (0..bins)
        .map(|b| {
            let freq = b as f64 * sample_rate as f64 / win as f64;
            BAND_EDGES.iter().filter(|e| freq >= **e).count()
        })
        .collect();
    let band_count = BAND_EDGES.len() + 1;
    let mut band_bins = vec![0usize; band_count];
    for &b in &band_of_bin {
        band_bins[b] += 1;
    }

    let mut band_flux = vec![vec![0f32; frames]; band_count];
    let mut prev_mag = vec![0f32; bins];
    for f in 0..frames {
        let start = f * hop;
        for i in 0..win {
            input[i] = samples[start + i] * window[i];
        }
        if fft.process_with_scratch(&mut input, &mut spectrum, &mut scratch).is_err() {
            return (Vec::new(), hop);
        }
        for b in 0..bins {
            let mag = spectrum[b].norm();
            let delta = (mag - prev_mag[b]).max(0.0);
            band_flux[band_of_bin[b]][f] += delta / band_bins[band_of_bin[b]].max(1) as f32;
            prev_mag[b] = mag;
        }
    }

    // Combine bands, each normalized by its own mean so quiet bands
    // still contribute.
    let mut envelope = vec![0f32; frames];
    for band in &band_flux {
        let mean = band.iter().sum::<f32>() / frames.max(1) as f32;
        if mean > 0.0 {
            for f in 0..frames {
                envelope[f] += band[f] / mean;
            }
        }
    }
    (envelope, hop)
}

/// Detect onsets with an adaptive threshold over the combined envelope.
/// Returns (sample position, strength) pairs; positions are exact, at
/// the centre of the analysis frame that produced the peak.
/// `sensitivity` multiplies the local mean: lower finds more onsets.
pub fn detect_onsets(samples: &[f32], sample_rate: u32, sensitivity: f64) -> Vec<(i64, f32)> {
    let (envelope, hop) = onset_envelope(samples, sample_rate);
    if envelope.len() < 3 || sample_rate == 0 {
        return Vec::new();
    }
    let frames = envelope.len();
    let hop_secs = hop as f64 / sample_rate as f64;
    let radius = ((0.35 / hop_secs).round() as usize).max(2);
    let min_gap = ((0.20 / hop_secs).round() as usize).max(1);

    // Robust scale for strengths: the 99th percentile of the envelope.
    let mut sorted = envelope.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = sorted[((frames as f64 * 0.99) as usize).min(frames - 1)].max(f32::MIN_POSITIVE);
    let global_floor = p99 * 0.08;

    let win = fft_size_for(sample_rate);
    let mut onsets = Vec::new();
    let mut last: Option<usize> = None;
    for f in 1..frames.saturating_sub(1) {
        let is_peak = envelope[f] >= envelope[f - 1] && envelope[f] >= envelope[f + 1];
        if !is_peak || envelope[f] < global_floor {
            continue;
        }
        let lo = f.saturating_sub(radius);
        let hi = (f + radius + 1).min(frames);
        let mean = envelope[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
        if (envelope[f] as f64) < mean as f64 * sensitivity {
            continue;
        }
        if last.map(|l| f - l < min_gap).unwrap_or(false) {
            continue;
        }
        let sample_pos = (f * hop + win / 2) as i64;
        onsets.push((sample_pos, (envelope[f] / p99).clamp(0.0, 1.0)));
        last = Some(f);
    }
    onsets
}

// ------------------------------------------------------------------ tempo

/// Estimate tempo in BPM by autocorrelating the onset envelope over
/// lags that correspond to 40..240 BPM, with a mild preference for the
/// 70..180 range to fight octave errors. The winning lag is refined by
/// parabolic interpolation, which takes the estimate below the one hop
/// quantization of a plain lag search.
pub fn estimate_tempo(samples: &[f32], sample_rate: u32) -> Option<f64> {
    let (envelope, hop) = onset_envelope(samples, sample_rate);
    if sample_rate == 0 || hop == 0 {
        return None;
    }
    tempo_from_envelope(&envelope, hop as f64 / sample_rate as f64)
}

pub fn tempo_from_envelope(envelope: &[f32], hop_secs: f64) -> Option<f64> {
    let frames = envelope.len();
    if frames < 16 || hop_secs <= 0.0 {
        return None;
    }
    // A flat envelope (silence, a steady tone) has no periodicity to find.
    if envelope.iter().copied().fold(0.0f32, f32::max) <= 0.0 {
        return None;
    }
    let mean = envelope.iter().sum::<f32>() / frames as f32;
    let centered: Vec<f32> = envelope.iter().map(|v| v - mean).collect();
    let min_lag = ((60.0 / 240.0) / hop_secs).ceil() as usize;
    let max_lag = (((60.0 / 40.0) / hop_secs).floor() as usize).min(frames / 2);
    if max_lag <= min_lag {
        return None;
    }
    let mut corr = vec![0f32; max_lag + 2];
    let mut best: Option<(f32, usize)> = None;
    for lag in min_lag..=max_lag {
        let mut c = 0f32;
        for i in 0..frames - lag {
            c += centered[i] * centered[i + lag];
        }
        corr[lag] = c;
        let bpm = 60.0 / (lag as f64 * hop_secs);
        let weight = if (70.0..=180.0).contains(&bpm) { 1.0 } else { 0.85 };
        let score = c * weight;
        if best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, lag));
        }
    }
    let (_, lag) = best?;
    // Parabolic interpolation around the peak for a sub hop estimate.
    let refined_lag = if lag > min_lag && lag < max_lag {
        let (a, b, c) = (corr[lag - 1], corr[lag], corr[lag + 1]);
        let denom = a - 2.0 * b + c;
        if denom.abs() > f32::EPSILON {
            let offset = 0.5 * (a - c) / denom;
            lag as f64 + offset.clamp(-0.5, 0.5) as f64
        } else {
            lag as f64
        }
    } else {
        lag as f64
    };
    Some(60.0 / (refined_lag * hop_secs))
}

// ------------------------------------------------------------------- grid

/// Build the regular beat grid in the sample domain: grid positions are
/// anchored on the strongest onset and spaced by the tempo, and each
/// position takes the strength of the strongest onset within a quarter
/// spacing, or a modest default where the music has no onset nearby.
/// Returns (sample position, strength) pairs.
pub fn build_beats(
    onsets: &[(i64, f32)],
    bpm: f64,
    total_samples: i64,
    sample_rate: u32,
) -> Vec<(i64, f32)> {
    if bpm <= 0.0 || total_samples <= 0 || sample_rate == 0 || onsets.is_empty() {
        return Vec::new();
    }
    let spacing = 60.0 / bpm * sample_rate as f64;
    if spacing < 1.0 {
        return Vec::new();
    }
    let anchor = onsets
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap()
        .0 as f64;
    let mut beats = Vec::new();
    let mut t = anchor.rem_euclid(spacing);
    while (t.round() as i64) < total_samples {
        let pos = t.round() as i64;
        let strength = onsets
            .iter()
            .filter(|o| (o.0 as f64 - t).abs() <= spacing * 0.25)
            .map(|o| o.1)
            .fold(0.35f32, f32::max);
        beats.push((pos, strength));
        t += spacing;
    }
    beats
}

// --------------------------------------------------------------- structure

/// Derive bars and phrases from a beat grid and a beats per bar setting.
///
/// The downbeat phase is chosen by scoring every candidate offset with
/// the summed strength of the beats that would land on bar starts, so a
/// grid whose strong beats sit on the downbeat groups correctly. Beats
/// before the first full bar, when the chosen offset is not zero, form a
/// pickup bar. Phrases group consecutive bars in fours, with a boundary
/// forced early when a bar is much stronger than the bar before it (a
/// lift of more than 0.35), because in practice a sudden lift is where a
/// new musical phrase begins.
pub fn structure_from_beats(beats: &[Beat], beats_per_bar: u32) -> (Vec<Bar>, Vec<Phrase>) {
    if beats.is_empty() || beats_per_bar == 0 {
        return (Vec::new(), Vec::new());
    }
    let bpb = beats_per_bar as usize;
    let mut best_offset = 0usize;
    let mut best_score = f32::MIN;
    for offset in 0..bpb.min(beats.len()) {
        let mut score = 0.0f32;
        let mut i = offset;
        while i < beats.len() {
            score += beats[i].strength;
            i += bpb;
        }
        if score > best_score {
            best_score = score;
            best_offset = offset;
        }
    }

    let mut bars = Vec::new();
    if best_offset > 0 {
        let group = &beats[..best_offset];
        bars.push(Bar {
            start: group[0].time,
            beats: group.len() as u32,
            strength: group.iter().map(|b| b.strength).fold(0.0f32, f32::max),
        });
    }
    let mut i = best_offset;
    while i < beats.len() {
        let end = (i + bpb).min(beats.len());
        let group = &beats[i..end];
        bars.push(Bar {
            start: group[0].time,
            beats: group.len() as u32,
            strength: group.iter().map(|b| b.strength).fold(0.0f32, f32::max),
        });
        i = end;
    }

    let mut phrases = Vec::new();
    let mut i = 0;
    while i < bars.len() {
        let first = i;
        let mut strength = bars[i].strength;
        i += 1;
        while i < bars.len() && i - first < 4 {
            if bars[i].strength - bars[i - 1].strength > 0.35 {
                break;
            }
            strength = strength.max(bars[i].strength);
            i += 1;
        }
        phrases.push(Phrase {
            start: bars[first].start,
            bars: (i - first) as u32,
            strength,
        });
    }
    (bars, phrases)
}

// ------------------------------------------------------------ change points

/// Per window loudness: the RMS of each window in dB, floored at -70.0
/// so silence reads as a finite quiet value.
pub fn loudness_envelope(samples: &[f32], sample_rate: u32, window_secs: f64) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 || window_secs <= 0.0 {
        return Vec::new();
    }
    let win = ((sample_rate as f64 * window_secs) as usize).max(1);
    samples
        .chunks(win)
        .map(|chunk| {
            let mean_sq = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            let rms = mean_sq.sqrt();
            if rms > 0.0 {
                (20.0 * rms.log10()).max(-70.0)
            } else {
                -70.0
            }
        })
        .collect()
}

/// Find change points in the loudness envelope, the automated music-video montage style
/// method: one second windows, and a point fires at window `i` when the
/// mean of the next two windows differs from the mean of the previous
/// two windows by more than `threshold_db`. Points are kept at least two
/// seconds apart. Window `i` starts at exactly `i` seconds, so change
/// point times convert to ticks exactly.
pub fn change_points(samples: &[f32], sample_rate: u32, threshold_db: f32) -> Vec<ChangePoint> {
    let env = loudness_envelope(samples, sample_rate, 1.0);
    let mut points = Vec::new();
    if env.len() < 5 || sample_rate == 0 {
        return points;
    }
    let mut last_window: Option<usize> = None;
    for i in 2..env.len() - 2 {
        let prev = (env[i - 2] + env[i - 1]) / 2.0;
        let next = (env[i + 1] + env[i + 2]) / 2.0;
        let delta = next - prev;
        if delta.abs() > threshold_db && last_window.map(|l| i - l >= 2).unwrap_or(true) {
            points.push(ChangePoint {
                time: samples_to_tick(i as i64 * sample_rate as i64, sample_rate),
                delta_db: delta,
            });
            last_window = Some(i);
        }
    }
    points
}

// ---------------------------------------------------------------- analysis

/// Analyse decoded samples into a [`SongAnalysis`], all in tick time.
pub fn analyse_samples(
    samples: &[f32],
    sample_rate: u32,
    settings: &AnalysisSettings,
) -> Result<SongAnalysis> {
    if samples.is_empty() || sample_rate == 0 {
        return msg("no samples to analyse");
    }
    let total = samples.len() as i64;
    let onset_pairs = detect_onsets(samples, sample_rate, settings.sensitivity);
    let bpm = estimate_tempo(samples, sample_rate).unwrap_or(0.0);

    let beats: Vec<Beat> = build_beats(&onset_pairs, bpm, total, sample_rate)
        .into_iter()
        .map(|(pos, strength)| Beat {
            time: samples_to_tick(pos, sample_rate),
            strength,
        })
        .collect();
    let (bars, phrases) = structure_from_beats(&beats, settings.beats_per_bar);

    Ok(SongAnalysis {
        duration: samples_to_tick(total, sample_rate),
        sample_rate,
        bpm,
        beats_per_bar: settings.beats_per_bar,
        onsets: onset_pairs
            .iter()
            .map(|&(pos, strength)| Onset {
                time: samples_to_tick(pos, sample_rate),
                strength,
            })
            .collect(),
        beats,
        bars,
        phrases,
        change_points: change_points(samples, sample_rate, settings.change_threshold_db),
    })
}

/// Decode a song file and analyse it in one call.
pub fn analyse_file(path: &Path, settings: &AnalysisSettings) -> Result<SongAnalysis> {
    let decoded = decode_file(path)?;
    analyse_samples(&decoded.samples, decoded.sample_rate, settings)
}

/// Min and max sample per bucket, for drawing a waveform overview.
pub fn waveform_peaks(samples: &[f32], buckets: usize) -> Vec<(f32, f32)> {
    if samples.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let per = (samples.len() as f64 / buckets as f64).ceil() as usize;
    samples
        .chunks(per.max(1))
        .map(|chunk| {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for &s in chunk {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            (lo, hi)
        })
        .collect()
}
