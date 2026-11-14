//! Integration tests for hyperdex-audio: real decode through Symphonia, the
//! proven tempo test target (a true 120 BPM fixture reading within about
//! 1 BPM), musical structure counts, and loudness change points.

use hyperdex_audio::*;
use hyperdex_core::Tick;

fn click_samples(rate: u32, bpm: f64, seconds: f64) -> Vec<f32> {
    let n = (rate as f64 * seconds) as usize;
    let mut samples = vec![0f32; n];
    let spacing = 60.0 / bpm;
    let mut beat = spacing;
    while beat < seconds {
        let pos = (beat * rate as f64) as usize;
        for j in 0..(rate as usize / 100) {
            if pos + j < n {
                let decay = 1.0 - j as f32 / (rate as f32 / 100.0);
                samples[pos + j] += 0.9 * decay * if j % 2 == 0 { 1.0 } else { -0.6 };
            }
        }
        beat += spacing;
    }
    samples
}

fn sine_samples(rate: u32, freq: f64, amp: f32, seconds: f64) -> Vec<f32> {
    let n = (rate as f64 * seconds) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            (2.0 * std::f64::consts::PI * freq * t).sin() as f32 * amp
        })
        .collect()
}

/// Minimal PCM16 WAV written with std only.
fn write_wav(path: &std::path::Path, samples: &[f32], rate: u32) {
    let mut data: Vec<u8> = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut wav: Vec<u8> = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, &wav).unwrap();
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("hyperdex_alpha9_audio_{}_{name}", std::process::id()))
}

#[test]
fn wav_fixture_at_true_120_bpm_reads_within_one_bpm() {
    let rate = 22_050u32;
    let samples = click_samples(rate, 120.0, 16.0);
    let path = temp_path("click120.wav");
    write_wav(&path, &samples, rate);

    let decoded = decode_file(&path).expect("decode wav fixture");
    assert_eq!(decoded.sample_rate, rate);
    assert!((decoded.duration_seconds() - 16.0).abs() < 0.05);

    let analysis = analyse_file(&path, &AnalysisSettings::default()).expect("analyse fixture");
    let _ = std::fs::remove_file(&path);

    assert!(
        (analysis.bpm - 120.0).abs() <= 1.0,
        "fixture at a true 120 BPM read as {} BPM",
        analysis.bpm
    );
    assert!(!analysis.onsets.is_empty(), "click train produced no onsets");
    assert!(analysis.beats.len() >= 28, "too few grid beats: {}", analysis.beats.len());
    // Grid spacing sits at the beat period of half a second.
    for w in analysis.beats.windows(2) {
        let gap = (w[1].time - w[0].time).to_seconds();
        assert!((gap - 0.5).abs() < 0.02, "grid gap {gap}");
    }
    // Onset times are exact sample positions converted to ticks: every
    // onset lands on a whole number of samples at this rate.
    for onset in &analysis.onsets {
        let back = onset.time.to_samples_floor(rate);
        assert_eq!(samples_to_tick(back, rate), onset.time);
    }
}

#[test]
fn mp3_fixture_decodes_and_analyses() {
    let path = temp_path("click120.mp3");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/click_120.mp3");
    if fixture.exists() {
        std::fs::copy(&fixture, &path).unwrap();
    } else {
        // The fixture is a binary and does not travel through the
        // text only push route, so a fresh checkout may not have
        // it. Regenerate it from the same synthetic click train
        // instead of failing the suite: the MP3 decode path is
        // what this test exercises, not the committed bytes.
        let wav = temp_path("click120_gen.wav");
        write_wav(&wav, &click_samples(22_050, 120.0, 8.0), 22_050);
        let status = std::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&wav)
            .args(["-codec:a", "libmp3lame", "-q:a", "4"])
            .arg(&path)
            .status()
            .expect("ffmpeg must be on PATH to regenerate the mp3 fixture");
        assert!(status.success(), "ffmpeg failed to encode the mp3 fixture");
        let _ = std::fs::remove_file(&wav);
    }

    let decoded = decode_file(&path).expect("decode mp3 fixture");
    assert_eq!(decoded.sample_rate, 22_050);
    // MP3 adds encoder padding, so duration is approximate.
    assert!(
        (decoded.duration_seconds() - 8.0).abs() < 0.6,
        "mp3 duration {}",
        decoded.duration_seconds()
    );

    let analysis = analyse_file(&path, &AnalysisSettings::default()).expect("analyse mp3");
    let _ = std::fs::remove_file(&path);
    assert!(analysis.onsets.len() >= 8, "mp3 onsets: {}", analysis.onsets.len());
    assert!(
        (analysis.bpm - 120.0).abs() <= 3.0,
        "mp3 fixture read as {} BPM",
        analysis.bpm
    );
}

#[test]
fn bars_and_phrases_counts_on_a_synthetic_grid() {
    // 32 beats, half a second apart, strong every fourth beat.
    let rate = 22_050u32;
    let beats: Vec<Beat> = (0..32)
        .map(|i| Beat {
            time: samples_to_tick(i * 11_025, rate),
            strength: if i % 4 == 0 { 1.0 } else { 0.4 },
        })
        .collect();
    let (bars, phrases) = structure_from_beats(&beats, 4);
    assert_eq!(bars.len(), 8, "bars: {bars:?}");
    for (k, bar) in bars.iter().enumerate() {
        assert_eq!(bar.start, samples_to_tick(k as i64 * 44_100, rate));
        assert_eq!(bar.beats, 4);
        assert!((bar.strength - 1.0).abs() < 1e-6);
    }
    assert_eq!(phrases.len(), 2, "phrases: {phrases:?}");
    assert_eq!(phrases[0].bars, 4);
    assert_eq!(phrases[1].bars, 4);
    assert_eq!(phrases[0].start, Tick::ZERO);
    assert_eq!(phrases[1].start, samples_to_tick(4 * 44_100, rate));
}

#[test]
fn change_points_appear_at_a_loudness_jump() {
    let rate = 22_050u32;
    let mut samples = sine_samples(rate, 440.0, 0.1, 4.0);
    samples.extend(sine_samples(rate, 440.0, 0.8, 4.0));
    let analysis = analyse_samples(&samples, rate, &AnalysisSettings::default()).unwrap();
    let four_seconds = samples_to_tick(4 * rate as i64, rate);
    let tolerance = Tick::from_seconds(1.25);
    assert!(
        analysis
            .change_points
            .iter()
            .any(|p| (p.time - four_seconds).as_ticks().abs() <= tolerance.as_ticks() && p.delta_db > 0.0),
        "no positive change point near 4 s: {:?}",
        analysis.change_points
    );
    assert!(
        analysis.change_points.iter().all(|p| p.time >= Tick::from_seconds(2.0)),
        "change point inside the first 2 s: {:?}",
        analysis.change_points
    );
}

#[test]
fn silence_analyses_to_an_empty_grid() {
    let samples = vec![0f32; 22_050 * 4];
    let analysis = analyse_samples(&samples, 22_050, &AnalysisSettings::default()).unwrap();
    assert_eq!(analysis.bpm, 0.0);
    assert!(analysis.beats.is_empty());
    assert!(analysis.bars.is_empty());
    assert!(analysis.onsets.is_empty());
    assert_eq!(analysis.duration, Tick::from_seconds(4.0));
}


fn write_stereo_wav(path: &std::path::Path, left: &[f32], right: &[f32], rate: u32) {
    assert_eq!(left.len(), right.len());
    let mut data = Vec::<u8>::with_capacity(left.len() * 4);
    for (&l, &r) in left.iter().zip(right) {
        for sample in [l, r] {
            let value = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    let mut wav = Vec::<u8>::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

#[test]
fn stereo_decoder_preserves_left_and_right_channels() {
    let rate = 48_000u32;
    let seconds = 1.0;
    let left = sine_samples(rate, 440.0, 0.5, seconds);
    let right = sine_samples(rate, 880.0, 0.25, seconds);
    let path = temp_path("stereo.wav");
    write_stereo_wav(&path, &left, &right, rate);

    let decoded = decode_stereo_file(&path).expect("decode stereo wav");
    let _ = std::fs::remove_file(&path);
    assert_eq!(decoded.sample_rate, rate);
    assert_eq!(decoded.sample_frames(), left.len());
    assert!((decoded.duration_seconds() - seconds).abs() < 0.001);

    let rms = |channel: usize| {
        let energy = decoded.samples.iter().skip(channel).step_by(2)
            .map(|sample| sample * sample)
            .sum::<f32>();
        (energy / decoded.sample_frames() as f32).sqrt()
    };
    assert!((rms(0) - 0.5 / 2.0f32.sqrt()).abs() < 0.02);
    assert!((rms(1) - 0.25 / 2.0f32.sqrt()).abs() < 0.02);
    assert!(rms(0) > rms(1) * 1.8, "left/right channels were collapsed");
}
