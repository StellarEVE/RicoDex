//! Verification for the first render path: the CPU oracle's
//! transitions (endpoints pixel exact, mid frames genuinely their
//! named forms), the effects' directions, GPU against CPU agreement
//! on the software adapter, an end to end render job whose output
//! decodes back with the right shape, and the render cache serving
//! a second identical job without rendering again.
//!
//! Fixtures are generated with the system ffmpeg, like the media
//! crate's tests. The GPU test skips itself when no wgpu adapter
//! exists on the machine running the tests; on the development
//! sandbox the llvmpipe software adapter is present (under Xvfb)
//! and the test runs for real, printing the measured differences.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use hyperdex_core::{
    Asset, AssetKind, Fingerprint, Item, ItemTransition, Project, Tick, Track, TrackKind,
};
use hyperdex_render::{
    apply_effects, composite_cpu, composite_video_layers, frame_difference, render_project, render_timeline, render_timeline_with_audio, render_timeline_with_audio_items, StereoAudioAssetInput, EffectOp, Frame, StereoAudioInput,
    GpuCompositor, RenderCache, RenderRequest, TransitionKind, TransitionParams,
    TransitionSpec,
};

// ------------------------------------------------------------ helpers

fn write_stereo_wav(path: &Path, seconds: f64, rate: u32) {
    let frames = (seconds * rate as f64) as usize;
    let mut data = Vec::<u8>::with_capacity(frames * 4);
    for index in 0..frames {
        let time = index as f64 / rate as f64;
        let left = (2.0 * std::f64::consts::PI * 440.0 * time).sin() * 0.5;
        let right = (2.0 * std::f64::consts::PI * 880.0 * time).sin() * 0.25;
        for sample in [left, right] {
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

fn positive_zero_crossings(samples: &[f32], channel: usize, start: usize, end: usize) -> usize {
    let mut crossings = 0usize;
    let mut previous = samples[start * 2 + channel];
    for frame in (start + 1)..end {
        let current = samples[frame * 2 + channel];
        if previous <= 0.0 && current > 0.0 {
            crossings += 1;
        }
        previous = current;
    }
    crossings
}


fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hyperdex-render-test-{}-{}-{:?}",
        name,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ffmpeg(args: &[&str]) {
    // Use the explicit fixture encoder selected by CI when present.
    // The package build remains linked to the LGPL FFmpeg libraries;
    // this executable is used only to create test media.
    let ffmpeg = std::env::var_os("HYPERDEX_TEST_FFMPEG")
        .unwrap_or_else(|| "ffmpeg".into());
    let status = Command::new(ffmpeg)
        .args(args)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed for fixture: {args:?}");
}

/// Two 2 second, 10 fps, 320x240 clips with strongly different
/// pictures: A a moving test pattern, B colour bars.
fn make_clips(dir: &Path) -> (PathBuf, PathBuf) {
    let a = dir.join("a.mp4");
    let b = dir.join("b.mp4");
    run_ffmpeg(&[
        "-y", "-v", "error", "-f", "lavfi",
        "-i", "testsrc=size=320x240:rate=10:duration=2",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
        a.to_str().unwrap(),
    ]);
    run_ffmpeg(&[
        "-y", "-v", "error", "-f", "lavfi",
        "-i", "smptebars=size=320x240:rate=10:duration=2",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
        b.to_str().unwrap(),
    ]);
    (a, b)
}

/// Pattern frame A: a horizontal red gradient over a vertical green
/// gradient, so every warp moves pixels measurably.
fn pattern_a(w: u32, h: u32) -> Frame {
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let offset = ((y * w + x) * 4) as usize;
            pixels[offset] = (x * 255 / (w - 1)) as u8;
            pixels[offset + 1] = (y * 255 / (h - 1)) as u8;
            pixels[offset + 2] = 40;
            pixels[offset + 3] = 255;
        }
    }
    Frame::new(w, h, pixels).unwrap()
}

/// Pattern frame B: a checkerboard of blue and yellow, opposite in
/// character to A's smooth gradients.
fn pattern_b(w: u32, h: u32) -> Frame {
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let cell = 8u32;
    for y in 0..h {
        for x in 0..w {
            let offset = ((y * w + x) * 4) as usize;
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            let (r, g, b) = if on { (30, 60, 230) } else { (240, 220, 60) };
            pixels[offset] = r;
            pixels[offset + 1] = g;
            pixels[offset + 2] = b;
            pixels[offset + 3] = 255;
        }
    }
    Frame::new(w, h, pixels).unwrap()
}

fn mean_abs_diff(a: &Frame, b: &Frame) -> f64 {
    frame_difference(a, b).unwrap().1
}

fn all_kinds() -> Vec<TransitionKind> {
    vec![
        TransitionKind::CrossDissolve,
        TransitionKind::DipToColor,
        TransitionKind::FilmDissolve,
        TransitionKind::DipToDissolve,
        TransitionKind::CrashZoom,
        TransitionKind::DirectionalZoom,
        TransitionKind::TwistWheel,
        TransitionKind::RadialTwist,
        TransitionKind::Push,
        TransitionKind::Slide,
        TransitionKind::Split,
    ]
}

// -------------------------------------------------- CPU transitions

#[test]
fn cpu_endpoints_are_pixel_exact_for_every_transition() {
    let a = pattern_a(64, 48);
    let b = pattern_b(64, 48);
    for kind in all_kinds() {
        for params in [TransitionParams::default(), TransitionParams::dip_white()] {
            let at_zero = composite_cpu(&a, &b, kind, 0.0, &params).unwrap();
            let at_one = composite_cpu(&a, &b, kind, 1.0, &params).unwrap();
            assert_eq!(at_zero.pixels, a.pixels, "{kind:?} at 0.0 must be frame A exact");
            assert_eq!(at_one.pixels, b.pixels, "{kind:?} at 1.0 must be frame B exact");
        }
    }
}

#[test]
fn cpu_mid_frames_are_genuinely_their_named_forms() {
    let a = pattern_a(64, 48);
    let b = pattern_b(64, 48);
    let params = TransitionParams::default();
    let crossfade = composite_cpu(&a, &b, TransitionKind::CrossDissolve, 0.5, &params).unwrap();
    for kind in all_kinds() {
        let mid = composite_cpu(&a, &b, kind, 0.5, &params).unwrap();
        let from_a = mean_abs_diff(&mid, &a);
        let from_b = mean_abs_diff(&mid, &b);
        assert!(
            from_a > 3.0,
            "{kind:?} mid frame is too close to frame A (mean diff {from_a})"
        );
        assert!(
            from_b > 3.0,
            "{kind:?} mid frame is too close to frame B (mean diff {from_b})"
        );
        if kind != TransitionKind::CrossDissolve {
            let from_fade = mean_abs_diff(&mid, &crossfade);
            assert!(
                from_fade > 3.0,
                "{kind:?} mid frame is just a crossfade (mean diff {from_fade})"
            );
        }
    }
}

#[test]
fn cross_dissolve_mid_is_the_plain_crossfade() {
    let a = pattern_a(32, 24);
    let b = pattern_b(32, 24);
    let params = TransitionParams::default();
    let mid = composite_cpu(&a, &b, TransitionKind::CrossDissolve, 0.5, &params).unwrap();
    // Every channel is the average of the two inputs, within rounding.
    for (m, (pa, pb)) in mid
        .pixels
        .iter()
        .zip(a.pixels.iter().zip(b.pixels.iter()))
    {
        let expected = (*pa as f32 + *pb as f32) / 2.0;
        assert!(
            (*m as f32 - expected).abs() <= 1.0,
            "cross dissolve channel {m} vs expected {expected}"
        );
    }
}

#[test]
fn film_dissolve_differs_from_naive_srgb_crossfade() {
    // Black to white is where linear light blending shows most: the
    // film dissolve midpoint is about 187, the naive crossfade 128.
    let black = Frame::solid(16, 16, [0, 0, 0, 255]);
    let white = Frame::solid(16, 16, [255, 255, 255, 255]);
    let params = TransitionParams::default();
    let film = composite_cpu(&black, &white, TransitionKind::FilmDissolve, 0.5, &params).unwrap();
    let naive = composite_cpu(&black, &white, TransitionKind::CrossDissolve, 0.5, &params).unwrap();
    let diff = mean_abs_diff(&film, &naive);
    assert!(diff > 20.0, "film dissolve vs naive crossfade diff {diff}");
    assert!(film.pixel(0, 0)[0] > 170 && film.pixel(0, 0)[0] < 205);
}

// ------------------------------------------------------------ effects

fn mean_luma(frame: &Frame) -> f64 {
    let mut total = 0f64;
    let mut count = 0usize;
    for pixel in frame.pixels.as_chunks::<4>().0 {
        total += 0.2126 * pixel[0] as f64 + 0.7152 * pixel[1] as f64 + 0.0722 * pixel[2] as f64;
        count += 1;
    }
    total / count as f64
}

#[test]
fn brightness_raises_mean_luma() {
    let frame = pattern_a(32, 24);
    let brighter = apply_effects(&frame, &[EffectOp::Brightness { amount: 0.2 }]);
    assert!(mean_luma(&brighter) > mean_luma(&frame) + 30.0);
    let darker = apply_effects(&frame, &[EffectOp::Brightness { amount: -0.2 }]);
    assert!(mean_luma(&darker) < mean_luma(&frame) - 30.0);
}

#[test]
fn saturation_leaves_grey_and_moves_colour_away() {
    let grey = Frame::solid(8, 8, [100, 100, 100, 255]);
    let saturated_grey = apply_effects(&grey, &[EffectOp::Saturation { factor: 2.0 }]);
    assert_eq!(saturated_grey.pixels, grey.pixels, "grey must not move");

    let colour = Frame::solid(8, 8, [200, 50, 50, 255]);
    let spread = |f: &Frame| -> i32 {
        let p = f.pixel(0, 0);
        (p[0] as i32 - p[1] as i32).abs()
    };
    let saturated = apply_effects(&colour, &[EffectOp::Saturation { factor: 1.8 }]);
    assert!(
        spread(&saturated) > spread(&colour),
        "coloured pixel must move away from grey"
    );
    let drained = apply_effects(&colour, &[EffectOp::Saturation { factor: 0.0 }]);
    let p = drained.pixel(0, 0);
    assert!(p[0] == p[1] && p[1] == p[2], "zero saturation is pure grey");
}

#[test]
fn blur_reduces_and_sharpen_increases_local_contrast() {
    // A sharp vertical edge: left half black, right half white.
    let (w, h) = (32, 16);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let offset = ((y * w + x) * 4) as usize;
            let v = if x < w / 2 { 0 } else { 255 };
            pixels[offset] = v;
            pixels[offset + 1] = v;
            pixels[offset + 2] = v;
            pixels[offset + 3] = 255;
        }
    }
    let edge = Frame::new(w, h, pixels).unwrap();
    let adjacent_diff = |f: &Frame| -> f64 {
        // Difference across the edge in the middle row.
        let y = h / 2;
        (f.pixel(w / 2, y)[0] as f64 - f.pixel(w / 2 - 1, y)[0] as f64).abs()
    };
    let base = adjacent_diff(&edge);
    let blurred = apply_effects(&edge, &[EffectOp::Blur { radius: 2 }]);
    assert!(
        adjacent_diff(&blurred) < base * 0.6,
        "blur must soften the edge: {} vs {base}",
        adjacent_diff(&blurred)
    );
    let sharpened = apply_effects(&edge, &[EffectOp::Sharpen { amount: 1.0 }]);
    // Unsharp masking overshoots at edges: the total variation across
    // a small window around the edge grows even when the single step
    // at the boundary saturates.
    let variation = |f: &Frame| -> f64 {
        let y = h / 2;
        (f.pixel(w / 2 + 1, y)[0] as f64 - f.pixel(w / 2 - 2, y)[0] as f64).abs()
            + (f.pixel(w / 2, y)[0] as f64 - f.pixel(w / 2 - 1, y)[0] as f64).abs()
    };
    assert!(
        variation(&sharpened) >= variation(&edge),
        "sharpen must not soften the edge"
    );
    // An impulse shows the sharpen direction without clipping: a
    // lone mid grey line on black is brighter than its own blur, so
    // the unsharp mask must push its centre up. (A linear gradient
    // cannot show this: a symmetric blur leaves a straight ramp
    // unchanged, so there is no mask to add.)
    let (iw, ih) = (16, 8);
    let mut impulse_pixels = vec![0u8; (iw * ih * 4) as usize];
    for y in 0..ih {
        let offset = ((y * iw + iw / 2) * 4) as usize;
        impulse_pixels[offset] = 128;
        impulse_pixels[offset + 1] = 128;
        impulse_pixels[offset + 2] = 128;
        impulse_pixels[offset + 3] = 255;
    }
    // Alpha everywhere else must be opaque too, or the frame is
    // mostly transparent black; set it across the whole frame.
    for pixel in impulse_pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    let impulse = Frame::new(iw, ih, impulse_pixels).unwrap();
    let sharpened_impulse = apply_effects(&impulse, &[EffectOp::Sharpen { amount: 1.0 }]);
    assert!(
        sharpened_impulse.pixel(iw / 2, ih / 2)[0] > impulse.pixel(iw / 2, ih / 2)[0],
        "sharpen must raise the impulse centre: {} vs {}",
        sharpened_impulse.pixel(iw / 2, ih / 2)[0],
        impulse.pixel(iw / 2, ih / 2)[0]
    );
}

// ------------------------------------------------------ GPU vs CPU

#[test]
fn gpu_agrees_with_cpu_oracle_on_the_software_adapter() {
    let Some(gpu) = GpuCompositor::new() else {
        eprintln!("no wgpu adapter on this machine; GPU agreement test skipped");
        return;
    };
    println!("GPU adapter: {}", gpu.adapter_name());
    let a = pattern_a(96, 64);
    let b = pattern_b(96, 64);
    // Measured on llvmpipe and Intel UHD 620: max per channel
    // difference 1, mean below 0.55 across the transitions, dip colours
    // and progress samples. A small tolerance allows hardware
    // interpolation precision while catching shader or sampling drift.
    const MAX_TOLERANCE: u8 = 2;
    const MEAN_TOLERANCE: f64 = 0.75;
    for kind in [
        TransitionKind::CrossDissolve,
        TransitionKind::DipToColor,
        TransitionKind::CrashZoom,
    ] {
        for params in [TransitionParams::default(), TransitionParams::dip_white()] {
            for progress in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                let cpu = composite_cpu(&a, &b, kind, progress, &params).unwrap();
                let gpu_frame = gpu.composite(&a, &b, kind, progress, &params).unwrap();
                let (max, mean) = frame_difference(&cpu, &gpu_frame).unwrap();
                println!(
                    "{kind:?} dip={:?} p={progress}: max diff {max}, mean diff {mean:.4}",
                    params.dip_color
                );
                assert!(
                    max <= MAX_TOLERANCE && mean <= MEAN_TOLERANCE,
                    "{kind:?} p={progress}: GPU vs CPU max {max} mean {mean}"
                );
            }
        }
    }
}

// ------------------------------------------------------- render job

fn two_clip_project(a: &Path, b: &Path) -> Project {
    let mut project = Project::new();
    project.assets.push(Asset {
        id: 1,
        fingerprint: Fingerprint::new(1001, 11, 22, 33),
        path_hint: a.to_string_lossy().into_owned(),
        kind: AssetKind::Video,
        duration: Tick::from_seconds(2.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
    });
    project.assets.push(Asset {
        id: 2,
        fingerprint: Fingerprint::new(2002, 44, 55, 66),
        path_hint: b.to_string_lossy().into_owned(),
        kind: AssetKind::Video,
        duration: Tick::from_seconds(2.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
    });
    project.add_track(Track::new(1, TrackKind::Video, "V1"));
    let items = project.items_mut(1).unwrap();
    items.push(Item::new(1, 1, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO));
    items.push(Item::new(
        2,
        2,
        Tick::from_seconds(2.0),
        Tick::from_seconds(2.0),
        Tick::ZERO,
    ));
    project.export_settings.width = 160;
    project.export_settings.height = 120;
    project.export_settings.frame_rate_num = 10;
    project.export_settings.frame_rate_den = 1;
    project.export_settings.video_bitrate_kbps = 800;
    project
}

fn crash_zoom_request(project: Project, output: PathBuf) -> RenderRequest {
    RenderRequest {
        project,
        track_id: 1,
        transition: TransitionSpec {
            kind: TransitionKind::CrashZoom,
            params: TransitionParams::default(),
            duration: Tick::from_seconds(1.0),
        },
        effects: Vec::new(),
        output_path: output,
    }
}

/// Decode every frame of a file at small size and return them.
fn decode_all(path: &Path) -> Vec<(Tick, Vec<u8>, u32, u32)> {
    let mut decoder = hyperdex_platform::VideoDecoder::open(path).unwrap();
    let mut frames = Vec::new();
    while let Some(rgb) = decoder.next_rgb_frame(160).unwrap() {
        frames.push((rgb.time, rgb.pixels, rgb.width, rgb.height));
    }
    frames
}

#[test]
fn render_job_produces_a_verifiable_mp4() {
    let dir = test_dir("job");
    let (a, b) = make_clips(&dir);
    let output = dir.join("out.mp4");
    let request = crash_zoom_request(two_clip_project(&a, &b), output.clone());

    let progress_calls = RefCell::new(Vec::new());
    let outcome = render_project(&request, None, &mut |p| {
        progress_calls.borrow_mut().push(p);
    })
    .unwrap();
    println!(
        "render: {} frames, encoder {}, gpu {}",
        outcome.frames_written, outcome.encoder_name, outcome.used_gpu
    );

    // Progress fired repeatedly, monotonically, ending at exactly 1.
    let calls = progress_calls.borrow();
    assert!(calls.len() >= 10, "progress fired {} times", calls.len());
    assert!(calls.windows(2).all(|w| w[1] >= w[0]), "progress must not go backwards");
    assert_eq!(*calls.last().unwrap(), 1.0);
    drop(calls);

    // Shape: 2 s + 2 s minus the 1 s transition is 3 s at 10 fps.
    assert_eq!(outcome.frames_written, 30);
    let info = hyperdex_platform::probe(&output).unwrap();
    println!("output probe: duration {:.3} s, {}x{}", info.duration.to_seconds(), info.width, info.height);
    assert!(
        (info.duration.to_seconds() - 3.0).abs() <= 0.5,
        "duration {} s",
        info.duration.to_seconds()
    );
    assert_eq!((info.width, info.height), (160, 120));

    let frames = decode_all(&output);
    println!("decoded {} frames back", frames.len());
    assert!(
        (28..=32).contains(&frames.len()),
        "expected about 30 frames, decoded {}",
        frames.len()
    );

    // The mid transition frame (t = 1.5 s, frame index 15) differs
    // from both a pure A frame (index 5) and a pure B frame
    // (index 25).
    let to_frame = |idx: usize| {
        let (_, pixels, w, h) = &frames[idx];
        Frame::from_rgb24(*w, *h, pixels).unwrap()
    };
    let mid = to_frame(15);
    let pure_a = to_frame(5);
    let pure_b = to_frame(25);
    println!(
        "mid vs A mean diff {:.2}, mid vs B mean diff {:.2}",
        mean_abs_diff(&mid, &pure_a),
        mean_abs_diff(&mid, &pure_b)
    );
    assert!(mean_abs_diff(&mid, &pure_a) > 3.0);
    assert!(mean_abs_diff(&mid, &pure_b) > 3.0);
}

#[test]
fn timeline_export_renders_a_two_clip_transition_to_mp4() {
    let dir = test_dir("timeline-transition");
    let (a, b) = make_clips(&dir);
    let mut project = two_clip_project(&a, &b);
    project.items_mut(1).unwrap()[1].transition_in = Some(ItemTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(0.5),
    });
    let output = dir.join("transition.mp4");

    let outcome = render_timeline(&project, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 35);
    assert_eq!(outcome.duration, Tick::from_seconds(3.5));

    let info = hyperdex_platform::probe(&output).unwrap();
    assert!(
        (info.duration.to_seconds() - 3.5).abs() < 0.15,
        "transition-compressed MP4 duration was {} seconds",
        info.duration.to_seconds()
    );

    let rendered = decode_all(&output);
    let source_a = decode_all(&a);
    let source_b = decode_all(&b);
    assert!((34..=36).contains(&rendered.len()), "decoded {} output frames", rendered.len());

    // Output frame 17 is 1.7 s: 0.2 s into the 1.5..2.0 s
    // transition, so it should blend A at 1.7 s with B at 0.2 s.
    let as_frame = |frames: &Vec<(Tick, Vec<u8>, u32, u32)>, index: usize| {
        let (_, pixels, width, height) = &frames[index];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    let actual = as_frame(&rendered, 17);
    let outgoing = as_frame(&source_a, 17);
    let incoming = as_frame(&source_b, 2);
    let expected = composite_cpu(
        &outgoing,
        &incoming,
        TransitionKind::CrossDissolve,
        0.4,
        &TransitionParams::default(),
    ).unwrap();
    assert!(
        mean_abs_diff(&actual, &expected) < 16.0,
        "transition frame should match the CPU dissolve oracle, difference {}",
        mean_abs_diff(&actual, &expected)
    );

    // After the transition, incoming source time is shifted by the consumed
    // outgoing/incoming head region: output 2.5 s maps to B source time 1.0 s.
    let post_transition = as_frame(&rendered, 25);
    let expected_post = as_frame(&source_b, 10);
    assert!(
        mean_abs_diff(&post_transition, &expected_post) < 14.0,
        "incoming clip should resume at the compressed source position"
    );
}

#[test]
fn timeline_transition_is_composited_between_lower_and_upper_tracks() {
    let dir = test_dir("transition-track-order");
    let (a, b) = make_clips(&dir);
    let mut project = two_clip_project(&a, &b);

    // Track 2 transitions from B into A over the cut at 2 seconds.
    project.add_track(Track::new(2, TrackKind::Video, "V2"));
    project.items_mut(2).unwrap().push(Item::new(
        10, 2, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO,
    ));
    let mut incoming = Item::new(
        11, 1, Tick::from_seconds(2.0), Tick::from_seconds(2.0), Tick::ZERO,
    );
    incoming.transition_in = Some(ItemTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(0.5),
    });
    project.items_mut(2).unwrap().push(incoming);

    // Track 3 sits above the transition track for one frame inside the
    // transition region. It must remain the topmost layer after blending.
    project.add_track(Track::new(3, TrackKind::Video, "V3"));
    project.items_mut(3).unwrap().push(Item::new(
        12,
        2,
        Tick::from_seconds(1.6),
        Tick::from_seconds(0.2),
        Tick::ZERO,
    ));

    let output = dir.join("transition-stack.mp4");
    let outcome = render_timeline(&project, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 35);
    assert_eq!(outcome.duration, Tick::from_seconds(3.5));

    let rendered = decode_all(&output);
    let source_b = decode_all(&b);

    // At the first transition frame (output 1.5 s), progress is zero.
    // The transition track must cover its lower track with the outgoing B frame.
    let first_transition = {
        let (_, pixels, width, height) = &rendered[15];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    let expected_outgoing = {
        let (_, pixels, width, height) = &source_b[15];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    assert!(
        mean_abs_diff(&first_transition, &expected_outgoing) < 14.0,
        "transition track must composite above the lower track at its start"
    );

    let actual = {
        let (_, pixels, width, height) = &rendered[16];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    let expected_upper = {
        let (_, pixels, width, height) = &source_b[1];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    assert!(
        mean_abs_diff(&actual, &expected_upper) < 14.0,
        "upper video track must composite above the active transition, difference {}",
        mean_abs_diff(&actual, &expected_upper)
    );
}

#[test]
fn timeline_export_mixes_audio_track_items_to_aac_without_a_song_bed() {
    let dir = test_dir("audio-track-mix");
    let (a, b) = make_clips(&dir);
    let source_rate = 44_100u32;
    let mut source_samples = Vec::with_capacity((source_rate * 2 * 2) as usize);
    for index in 0..source_rate * 2 {
        let time = index as f32 / source_rate as f32;
        source_samples.push((2.0 * std::f32::consts::PI * 440.0 * time).sin() * 0.5);
        source_samples.push((2.0 * std::f32::consts::PI * 880.0 * time).sin() * 0.25);
    }

    let mut project = two_clip_project(&a, &b);
    project.assets.push(Asset {
        id: 3,
        fingerprint: Fingerprint::new(3003, 77, 88, 99),
        path_hint: "placed-audio.wav".into(),
        kind: AssetKind::Audio,
        duration: Tick::from_seconds(2.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
    });
    project.add_track(Track::new(2, TrackKind::Audio, "A1"));
    project.items_mut(2).unwrap().push(Item::new(10, 3, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO));

    let output = dir.join("audio-track.mp4");
    let audio_asset = StereoAudioAssetInput {
        asset_id: 3,
        interleaved_samples: &source_samples,
        sample_rate: source_rate,
    };
    let outcome = render_timeline_with_audio_items(&project, &output, None, &[audio_asset], &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 40);
    assert_eq!(outcome.duration, Tick::from_seconds(4.0));

    let exported = hyperdex_audio::decode_stereo_file(&output)
        .expect("a placed audio item must produce AAC even without a selected-song bed");
    assert_eq!(exported.sample_rate, 48_000);
    assert!((exported.duration_seconds() - 4.0).abs() < 0.06);

    let left = positive_zero_crossings(&exported.samples, 0, 48_000 / 5, 48_000 * 9 / 10);
    let right = positive_zero_crossings(&exported.samples, 1, 48_000 / 5, 48_000 * 9 / 10);
    assert!((250..=360).contains(&left), "440 Hz left channel crossings={left}");
    assert!((520..=720).contains(&right), "880 Hz right channel crossings={right}");

    let silent_start = 48_000 * 5 / 2;
    let silent_end = 48_000 * 7 / 2;
    let tail_rms = (silent_start * 2..silent_end * 2)
        .map(|index| exported.samples[index] * exported.samples[index])
        .sum::<f32>() / ((silent_end - silent_start) * 2) as f32;
    assert!(tail_rms.sqrt() < 0.02, "audio should be silent after item end, RMS={}", tail_rms.sqrt());
}

#[test]
fn muted_hidden_and_disabled_audio_items_do_not_create_aac_audio() {
    let dir = test_dir("audio-track-muted");
    let (a, b) = make_clips(&dir);
    let source_samples = vec![0.25f32; (48_000 * 2 * 2) as usize];

    let mut project = two_clip_project(&a, &b);
    project.assets.push(Asset {
        id: 3,
        fingerprint: Fingerprint::new(3004, 71, 81, 91),
        path_hint: "muted.wav".into(),
        kind: AssetKind::Audio,
        duration: Tick::from_seconds(2.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
    });
    project.add_track(Track::new(2, TrackKind::Audio, "Muted"));
    project.tracks.iter_mut().find(|track| track.id == 2).unwrap().muted = true;
    project.items_mut(2).unwrap().push(Item::new(20, 3, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO));
    project.add_track(Track::new(3, TrackKind::Audio, "Hidden"));
    project.tracks.iter_mut().find(|track| track.id == 3).unwrap().hidden = true;
    project.items_mut(3).unwrap().push(Item::new(21, 3, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO));
    project.add_track(Track::new(4, TrackKind::Audio, "Disabled"));
    let mut disabled = Item::new(22, 3, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO);
    disabled.enabled = false;
    project.items_mut(4).unwrap().push(disabled);

    let output = dir.join("muted-audio-items.mp4");
    let audio_asset = StereoAudioAssetInput { asset_id: 3, interleaved_samples: &source_samples, sample_rate: 48_000 };
    let outcome = render_timeline_with_audio_items(&project, &output, None, &[audio_asset], &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 40);
    assert!(
        hyperdex_audio::decode_stereo_file(&output).is_err(),
        "muted, hidden and disabled items must not create an audio stream"
    );
}

#[test]
fn render_cache_serves_the_second_identical_job() {
    let dir = test_dir("cache");
    let (a, b) = make_clips(&dir);
    let cache = RenderCache::new(dir.join("cache-home"));

    let first = crash_zoom_request(two_clip_project(&a, &b), dir.join("first.mp4"));
    let outcome1 = render_project(&first, Some(&cache), &mut |_| {}).unwrap();
    assert!(!outcome1.cache_hit);
    assert_eq!(cache.renders_performed(), 1, "first job renders the unit once");

    let second = crash_zoom_request(two_clip_project(&a, &b), dir.join("second.mp4"));
    let outcome2 = render_project(&second, Some(&cache), &mut |_| {}).unwrap();
    assert!(outcome2.cache_hit, "second identical job must hit the cache");
    assert_eq!(
        cache.renders_performed(),
        1,
        "a cache hit must not render again"
    );

    // A different transition duration is a different unit and misses.
    let mut third = crash_zoom_request(two_clip_project(&a, &b), dir.join("third.mp4"));
    third.transition.duration = Tick::from_seconds(0.5);
    let outcome3 = render_project(&third, Some(&cache), &mut |_| {}).unwrap();
    assert!(!outcome3.cache_hit);
    assert_eq!(cache.renders_performed(), 2);
}

// ------------------------------------------------------- sequential export



fn two_track_project(a: &Path, b: &Path) -> Project {
    let mut project = two_clip_project(a, b);
    project.items_mut(1).unwrap().clear();
    project.items_mut(1).unwrap().push(Item::new(
        11, 1, Tick::ZERO, Tick::from_seconds(2.0), Tick::ZERO,
    ));
    project.add_track(Track::new(2, TrackKind::Video, "V2"));
    project.items_mut(2).unwrap().push(Item::new(
        21, 2, Tick::ZERO, Tick::from_seconds(1.0), Tick::ZERO,
    ));
    project
}

#[test]
fn timeline_export_composites_upper_tracks_and_reveals_lower_tracks_in_gaps() {
    let dir = test_dir("multitrack-composite");
    let (a, b) = make_clips(&dir);
    let project = two_track_project(&a, &b);
    let output = dir.join("multitrack.mp4");

    let outcome = render_timeline(&project, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 20);
    assert_eq!(outcome.duration, Tick::from_seconds(2.0));

    let rendered = decode_all(&output);
    let source_a = decode_all(&a);
    let source_b = decode_all(&b);
    let to_frame = |frames: &Vec<(Tick, Vec<u8>, u32, u32)>, index: usize| {
        let (_, pixels, width, height) = &frames[index];
        Frame::from_rgb24(*width, *height, pixels).unwrap()
    };
    let upper_track_frame = to_frame(&rendered, 5);
    let expected_upper = to_frame(&source_b, 5);
    let lower_track_frame = to_frame(&rendered, 15);
    let expected_lower = to_frame(&source_a, 15);
    assert!(
        mean_abs_diff(&upper_track_frame, &expected_upper) < 12.0,
        "upper track should cover lower track, diff {}",
        mean_abs_diff(&upper_track_frame, &expected_upper)
    );
    assert!(
        mean_abs_diff(&lower_track_frame, &expected_lower) < 12.0,
        "a gap on the upper track should reveal the lower track, diff {}",
        mean_abs_diff(&lower_track_frame, &expected_lower)
    );
}

#[test]
fn timeline_export_ignores_hidden_video_tracks() {
    let dir = test_dir("multitrack-hidden");
    let (a, b) = make_clips(&dir);
    let mut project = two_track_project(&a, &b);
    project.track_mut(2).unwrap().hidden = true;
    let output = dir.join("hidden-track.mp4");

    let outcome = render_timeline(&project, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 20);
    let rendered = decode_all(&output);
    let source_a = decode_all(&a);
    let (_, rendered_pixels, width, height) = &rendered[5];
    let (_, source_pixels, _, _) = &source_a[5];
    let rendered_frame = Frame::from_rgb24(*width, *height, rendered_pixels).unwrap();
    let source_frame = Frame::from_rgb24(*width, *height, source_pixels).unwrap();
    assert!(
        mean_abs_diff(&rendered_frame, &source_frame) < 12.0,
        "hidden upper track must not contribute"
    );
}

#[test]
fn disabled_upper_video_clip_reveals_the_lower_track() {
    let dir = test_dir("multitrack-disabled");
    let (a, b) = make_clips(&dir);
    let mut project = two_track_project(&a, &b);
    project.items_mut(2).unwrap()[0].enabled = false;
    let output = dir.join("disabled-upper.mp4");

    let outcome = render_timeline(&project, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 20);
    let rendered = decode_all(&output);
    let source_a = decode_all(&a);
    for index in [5usize, 15usize] {
        let (_, rendered_pixels, width, height) = &rendered[index];
        let (_, source_pixels, _, _) = &source_a[index];
        let rendered_frame = Frame::from_rgb24(*width, *height, rendered_pixels).unwrap();
        let source_frame = Frame::from_rgb24(*width, *height, source_pixels).unwrap();
        assert!(
            mean_abs_diff(&rendered_frame, &source_frame) < 12.0,
            "disabled upper clip must reveal the lower track at frame {index}"
        );
    }
}

/// Three 1 second clips back to back: A, B, then A again.
fn three_clip_project(a: &Path, b: &Path) -> Project {
    let mut project = two_clip_project(a, b);
    let items = project.items_mut(1).unwrap();
    items.clear();
    items.push(Item::new(1, 1, Tick::ZERO, Tick::from_seconds(1.0), Tick::ZERO));
    items.push(Item::new(
        2,
        2,
        Tick::from_seconds(1.0),
        Tick::from_seconds(1.0),
        Tick::ZERO,
    ));
    items.push(Item::new(
        3,
        1,
        Tick::from_seconds(2.0),
        Tick::from_seconds(1.0),
        Tick::from_seconds(0.5),
    ));
    project
}

#[test]
fn sequential_export_renders_a_three_clip_timeline() {
    let dir = test_dir("sequential");
    let (a, b) = make_clips(&dir);
    let output = dir.join("seq.mp4");
    let project = three_clip_project(&a, &b);

    let progress_calls = RefCell::new(Vec::new());
    let outcome = hyperdex_render::render_sequential(&project, 1, &output, &mut |p| {
        progress_calls.borrow_mut().push(p);
    })
    .unwrap();
    println!(
        "sequential: {} frames, encoder {}",
        outcome.frames_written, outcome.encoder_name
    );
    assert_eq!(outcome.frames_written, 30);
    assert_eq!(outcome.duration, Tick::from_seconds(3.0));

    let calls = progress_calls.borrow();
    assert!(calls.len() >= 10, "progress fired {} times", calls.len());
    assert!(calls.windows(2).all(|w| w[1] >= w[0]));
    assert_eq!(*calls.last().unwrap(), 1.0);
    drop(calls);

    let info = hyperdex_platform::probe(&output).unwrap();
    assert_eq!((info.width, info.height), (160, 120));
    assert!(
        (info.duration.to_seconds() - 3.0).abs() <= 0.5,
        "duration {} s",
        info.duration.to_seconds()
    );

    // First clip frames look like A (testsrc), middle like B (bars):
    // compare a frame from each second against the others.
    let frames = decode_all(&output);
    assert!(
        (28..=32).contains(&frames.len()),
        "expected about 30 frames, decoded {}",
        frames.len()
    );
    let to_frame = |idx: usize| {
        let (_, pixels, w, h) = &frames[idx];
        Frame::from_rgb24(*w, *h, pixels).unwrap()
    };
    let from_a = to_frame(5);
    let from_b = to_frame(15);
    let from_a_again = to_frame(25);
    println!(
        "seq diffs: a-b {:.1}, a-a2 {:.1}",
        mean_abs_diff(&from_a, &from_b),
        mean_abs_diff(&from_a, &from_a_again)
    );
    assert!(
        mean_abs_diff(&from_a, &from_b) > 5.0,
        "the B second should look different from the A second"
    );
}

#[test]
fn sequential_export_renders_timeline_gaps_as_black_frames() {
    let dir = test_dir("sequential-gap");
    let (a, b) = make_clips(&dir);
    let mut project = two_clip_project(&a, &b);
    // Leave a one-second gap between two two-second clips.
    project.items_mut(1).unwrap()[1].start = Tick::from_seconds(3.0);

    let output = dir.join("gap.mp4");
    let outcome = hyperdex_render::render_sequential(&project, 1, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 50);
    assert_eq!(outcome.duration, Tick::from_seconds(5.0));

    let frames = decode_all(&output);
    assert!((48..=52).contains(&frames.len()), "decoded {} frames", frames.len());
    let (_, pixels, width, height) = &frames[25];
    let gap_frame = Frame::from_rgb24(*width, *height, pixels).unwrap();
    let black = Frame::solid(*width, *height, [0, 0, 0, 255]);
    assert!(
        mean_abs_diff(&gap_frame, &black) < 8.0,
        "the middle gap should be black, mean difference {}",
        mean_abs_diff(&gap_frame, &black)
    );
}

#[test]
fn sequential_export_preserves_disabled_clip_as_black_gap() {
    let dir = test_dir("sequential-disabled-gap");
    let (a, b) = make_clips(&dir);
    let mut project = three_clip_project(&a, &b);
    project.items_mut(1).unwrap()[1].enabled = false;

    let output = dir.join("disabled-gap.mp4");
    let outcome = hyperdex_render::render_sequential(&project, 1, &output, &mut |_| {}).unwrap();
    assert_eq!(outcome.frames_written, 30);
    assert_eq!(outcome.duration, Tick::from_seconds(3.0));

    let frames = decode_all(&output);
    assert!((28..=32).contains(&frames.len()), "decoded {} frames", frames.len());
    let (_, pixels, width, height) = &frames[15];
    let gap_frame = Frame::from_rgb24(*width, *height, pixels).unwrap();
    let black = Frame::solid(*width, *height, [0, 0, 0, 255]);
    assert!(
        mean_abs_diff(&gap_frame, &black) < 8.0,
        "disabled clip interval should be black, mean difference {}",
        mean_abs_diff(&gap_frame, &black)
    );
}


#[test]
fn sequential_export_muxes_selected_stereo_song_in_sync_with_video() {
    let dir = test_dir("sequential-audio");
    let (a, b) = make_clips(&dir);
    let song_path = dir.join("song.wav");
    write_stereo_wav(&song_path, 3.0, 44_100);
    let source_audio = hyperdex_audio::decode_stereo_file(&song_path).unwrap();
    assert_eq!(source_audio.sample_rate, 44_100);

    let mut project = three_clip_project(&a, &b);
    let transition = ItemTransition {
        kind: TransitionKind::CrossDissolve,
        params: TransitionParams::default(),
        duration: Tick::from_seconds(0.25),
    };
    project.items_mut(1).unwrap()[1].transition_in = Some(transition);
    project.items_mut(1).unwrap()[2].transition_in = Some(transition);
    project.add_track(Track::new(2, TrackKind::Video, "V2"));
    project.items_mut(2).unwrap().push(Item::new(
        4, 2, Tick::ZERO, Tick::from_seconds(1.0), Tick::ZERO,
    ));
    project.assets.push(Asset {
        id: 3,
        fingerprint: Fingerprint::new(3003, 77, 88, 99),
        path_hint: song_path.to_string_lossy().into_owned(),
        kind: AssetKind::Audio,
        duration: Tick::from_seconds(3.0),
        intro_trim: Tick::ZERO,
        outro_trim: Tick::ZERO,
    });
    project.song_asset_id = Some(3);

    let output = dir.join("with-song.mp4");
    let progress_calls = RefCell::new(Vec::new());
    let outcome = render_timeline_with_audio(
        &project,
        &output,
        Some(StereoAudioInput {
            interleaved_samples: &source_audio.samples,
            sample_rate: source_audio.sample_rate,
        }),
        &mut |fraction| progress_calls.borrow_mut().push(fraction),
    )
    .unwrap();

    assert_eq!(outcome.frames_written, 25);
    assert_eq!(outcome.duration, Tick::from_seconds(2.5));
    assert!(progress_calls.borrow().windows(2).all(|pair| pair[1] >= pair[0]));
    assert_eq!(*progress_calls.borrow().last().unwrap(), 1.0);

    let exported = hyperdex_audio::decode_stereo_file(&output)
        .expect("exported MP4 must contain decodable AAC audio");
    assert_eq!(exported.sample_rate, 48_000);
    assert!(exported.sample_frames() > 48_000 * 2);
    assert!(
        (exported.duration_seconds() - 2.5).abs() < 0.06,
        "audio duration {} should match 2.5 second transition-compressed video",
        exported.duration_seconds()
    );

    // Different tones in the two input channels prove that the
    // soundtrack remained stereo through decode, resample and AAC mux.
    let start = 48_000 / 5;
    let end = 48_000 * 9 / 10;
    let left_crossings = positive_zero_crossings(&exported.samples, 0, start, end);
    let right_crossings = positive_zero_crossings(&exported.samples, 1, start, end);
    assert!(
        (250..=360).contains(&left_crossings),
        "left channel should remain near 440 Hz, crossings={left_crossings}"
    );
    assert!(
        (520..=720).contains(&right_crossings),
        "right channel should remain near 880 Hz, crossings={right_crossings}"
    );

    let video_info = hyperdex_platform::probe(&output).unwrap();
    assert!(
        (video_info.duration.to_seconds() - exported.duration_seconds()).abs() < 0.06,
        "audio and video durations diverged: video={} audio={}",
        video_info.duration.to_seconds(),
        exported.duration_seconds()
    );
}

#[test]
fn video_layer_compositor_uses_later_tracks_as_the_top_layer() {
    let bottom = Frame::solid(2, 2, [220, 20, 10, 255]);
    let top = Frame::solid(2, 2, [10, 30, 230, 255]);
    let output = composite_video_layers(2, 2, &[bottom, top]).unwrap();
    assert_eq!(output.pixel(0, 0), [10, 30, 230, 255]);
}

#[test]
fn video_layer_compositor_blends_transparency_over_lower_tracks() {
    let bottom = Frame::solid(1, 1, [200, 0, 0, 255]);
    let top = Frame::solid(1, 1, [0, 0, 200, 128]);
    let output = composite_video_layers(1, 1, &[bottom, top]).unwrap();
    let pixel = output.pixel(0, 0);
    assert!((pixel[0] as i16 - 100).abs() <= 1, "red channel: {pixel:?}");
    assert!((pixel[2] as i16 - 100).abs() <= 1, "blue channel: {pixel:?}");
    assert_eq!(pixel[3], 255);
}

#[test]
fn video_layer_compositor_returns_black_when_no_video_layer_is_active() {
    let output = composite_video_layers(2, 2, &[]).unwrap();
    assert_eq!(output.pixel(1, 1), [0, 0, 0, 255]);
}
