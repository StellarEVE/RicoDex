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

use ricodex_core::{
    Asset, AssetKind, Fingerprint, Item, Project, Tick, Track, TrackKind,
};
use ricodex_render::{
    apply_effects, composite_cpu, frame_difference, render_project, EffectOp, Frame,
    GpuCompositor, RenderCache, RenderRequest, TransitionKind, TransitionParams,
    TransitionSpec,
};

// ------------------------------------------------------------ helpers

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "music video-render-test-{}-{}-{:?}",
        name,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
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
            let on = ((x / cell) + (y / cell)) % 2 == 0;
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
    for pixel in frame.pixels.chunks_exact(4) {
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
    for pixel in impulse_pixels.chunks_exact_mut(4) {
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
    // Documented tolerance for the software adapter. Measured on
    // llvmpipe (2026-11-11, the run recorded in the slice report):
    // max per channel difference 1, mean at most 0.155, across all
    // three transitions, both dip colours and progress 0 to 1. The
    // bounds below sit just above the measurements: the two paths
    // share the maths and the sampling convention, so anything
    // larger is a real divergence, not float noise.
    const MAX_TOLERANCE: u8 = 2;
    const MEAN_TOLERANCE: f64 = 0.5;
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
    });
    project.assets.push(Asset {
        id: 2,
        fingerprint: Fingerprint::new(2002, 44, 55, 66),
        path_hint: b.to_string_lossy().into_owned(),
        kind: AssetKind::Video,
        duration: Tick::from_seconds(2.0),
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
    let mut decoder = ricodex_platform::VideoDecoder::open(path).unwrap();
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
    let info = ricodex_platform::probe(&output).unwrap();
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
    let outcome = ricodex_render::render_sequential(&project, 1, &output, &mut |p| {
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

    let info = ricodex_platform::probe(&output).unwrap();
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
fn sequential_export_rejects_a_gap() {
    let dir = test_dir("sequential-gap");
    let (a, b) = make_clips(&dir);
    let mut project = two_clip_project(&a, &b);
    // Move the second clip one second later, opening a gap.
    let items = project.items_mut(1).unwrap();
    items[1].start = Tick::from_seconds(3.0);

    let output = dir.join("gap.mp4");
    let error = ricodex_render::render_sequential(&project, 1, &output, &mut |_| {}).unwrap_err();
    assert!(
        error.to_string().contains("back to back"),
        "unexpected error: {error}"
    );
    assert!(!output.exists(), "a rejected export writes no file");
}
