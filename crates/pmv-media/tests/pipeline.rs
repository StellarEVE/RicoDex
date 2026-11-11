//! Media pipeline tests over generated fixtures: fingerprinting,
//! stage 0 probe, the stage 1 sparse pass, the stage 2 full pass,
//! and the fingerprint keyed analysis cache.

mod common;

use std::cell::RefCell;
use std::path::PathBuf;

use pmv_media::{
    analyse_stage1, analyse_stage2, compute_fingerprint, probe, AnalysisCache,
};

use common::{make_black_fixture, make_cut_fixture, test_dir};

/// The hard cut in the cut fixture sits at frame 55 of 24 fps.
const CUT_SECONDS: f64 = 55.0 / 24.0;

#[test]
fn fingerprint_is_stable_and_content_based() {
    let dir = test_dir("fingerprint");
    let cut = make_cut_fixture(&dir);
    let first = compute_fingerprint(&cut).unwrap();
    let second = compute_fingerprint(&cut).unwrap();
    assert_eq!(first, second, "same file must fingerprint equal");

    // A byte for byte copy at a different path fingerprints equal:
    // identity is content, not location.
    let copy_path = dir.join("copy.mp4");
    std::fs::copy(&cut, &copy_path).unwrap();
    assert_eq!(compute_fingerprint(&copy_path).unwrap(), first);

    // Different content differs.
    let black = make_black_fixture(&dir);
    assert_ne!(compute_fingerprint(&black).unwrap(), first);

    // Appending changes the fingerprint (size and tail sample move).
    let mut grown = std::fs::read(&cut).unwrap();
    grown.push(0xAB);
    let grown_path = dir.join("grown.mp4");
    std::fs::write(&grown_path, grown).unwrap();
    assert_ne!(compute_fingerprint(&grown_path).unwrap(), first);
}

#[test]
fn probe_returns_metadata() {
    let dir = test_dir("probe");
    let cut = make_cut_fixture(&dir);
    let info = probe(&cut).unwrap();
    assert_eq!(info.width, 320);
    assert_eq!(info.height, 240);
    assert_eq!(info.codec_name, "h264");
    let seconds = info.duration.to_seconds();
    assert!(
        (4.3..4.7).contains(&seconds),
        "duration {seconds} not near 4.5 seconds"
    );
    assert!(
        (info.frame_rate - 24.0).abs() < 0.5,
        "frame rate {} not near 24",
        info.frame_rate
    );
    assert!(
        matches!(info.rotation, None | Some(0)),
        "unexpected rotation {:?}",
        info.rotation
    );
}

#[test]
fn stage1_finds_the_cut_approximately_and_marks_partial() {
    let dir = test_dir("stage1-cut");
    let cut = make_cut_fixture(&dir);
    let images = dir.join("images");
    let output = analyse_stage1(&cut, &images).unwrap();
    let analysis = &output.analysis;

    assert!(analysis.is_partial(), "stage 1 output must be partial");
    assert_eq!(analysis.stage, 1);
    assert!(
        analysis.scenes.len() >= 2,
        "expected at least 2 scenes, got {}",
        analysis.scenes.len()
    );
    assert!(
        analysis.scenes.iter().all(|s| s.coarse),
        "stage 1 scenes must all be coarse"
    );
    assert!(
        analysis.scenes.iter().all(|s| s.motion.is_some()),
        "stage 1 scenes carry a rough motion estimate"
    );

    // The first boundary lands within the sampling tolerance of the
    // true cut: stage 1 samples at 2 fps, so half a second plus a
    // frame of slack.
    let boundary = analysis.scenes[1].start.to_seconds();
    assert!(
        (boundary - CUT_SECONDS).abs() <= 0.6,
        "stage 1 boundary {boundary} too far from the cut at {CUT_SECONDS}"
    );

    // Poster and filmstrip are real PNG files in the image dir.
    for path in [&output.poster_path, &output.filmstrip_path] {
        assert!(path.is_file(), "{} missing", path.display());
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG: {path:?}");
    }
    assert_eq!(output.poster_path, images.join("poster.png"));
    assert_eq!(output.filmstrip_path, images.join("filmstrip.png"));
}

#[test]
fn stage1_detects_the_black_range() {
    let dir = test_dir("stage1-black");
    let black = make_black_fixture(&dir);
    let output = analyse_stage1(&black, &dir.join("images")).unwrap();
    let ranges = &output.analysis.black_ranges;
    assert!(!ranges.is_empty(), "expected a black range");
    // The black segment runs 1.5 to 3.0 seconds; the sampled range
    // must cover the heart of it within sampling tolerance.
    let covers = ranges.iter().any(|r| {
        let start = r.start.to_seconds();
        let end = r.end().to_seconds();
        start <= 2.1 && end >= 2.4 && start >= 0.8 && end <= 3.7
    });
    assert!(covers, "no black range covering the segment: {ranges:?}");
}

#[test]
fn stage2_finds_the_cut_precisely_with_progress_and_motion() {
    let dir = test_dir("stage2-cut");
    let cut = make_cut_fixture(&dir);

    // Stage 1 boundary, for the precision comparison.
    let stage1 = analyse_stage1(&cut, &dir.join("images")).unwrap();
    let stage1_boundary = stage1.analysis.scenes[1].start.to_seconds();
    let stage1_error = (stage1_boundary - CUT_SECONDS).abs();

    let calls: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    let analysis = analyse_stage2(&cut, &mut |fraction| {
        calls.borrow_mut().push(fraction);
    })
    .unwrap();

    // Progress: many invocations, all in range, nondecreasing, and
    // the final value is exactly 1.0.
    let calls = calls.borrow();
    assert!(
        calls.len() >= 10,
        "progress invoked {} times, expected at least 10",
        calls.len()
    );
    assert!(calls.iter().all(|f| (0.0..=1.0).contains(f)));
    assert!(
        calls.windows(2).all(|w| w[1] >= w[0]),
        "progress must not go backwards: {calls:?}"
    );
    assert_eq!(*calls.last().unwrap(), 1.0);
    drop(calls);

    assert!(!analysis.is_partial(), "stage 2 output must be settled");
    assert_eq!(analysis.stage, 2);
    assert!(analysis.scenes.len() >= 2);
    assert!(analysis.scenes.iter().all(|s| !s.coarse));
    assert!(
        analysis.scenes.iter().all(|s| s.motion.is_some()),
        "stage 2 scenes must carry motion scores"
    );

    // More precise than stage 1, and close to the true cut in
    // absolute terms (a frame is 1/24 s).
    let boundary = analysis.scenes[1].start.to_seconds();
    let error = (boundary - CUT_SECONDS).abs();
    assert!(
        error < 0.1,
        "stage 2 boundary {boundary} too far from the cut at {CUT_SECONDS}"
    );
    assert!(
        error < stage1_error,
        "stage 2 error {error} not better than stage 1 error {stage1_error}"
    );

    // Motion: the moving box segment scores higher than the solid
    // segment.
    let first_motion = analysis.scenes[0].motion.unwrap();
    let second_motion = analysis.scenes[1].motion.unwrap();
    assert!(
        first_motion > second_motion,
        "moving segment motion {first_motion} should exceed solid segment motion {second_motion}"
    );
    assert!(first_motion > 0.001, "moving segment motion too low: {first_motion}");
}

#[test]
fn cache_round_trip_serves_results_without_the_source_file() {
    let dir = test_dir("cache");
    let cut = make_cut_fixture(&dir);
    let cache = AnalysisCache::new(dir.join("cache-home"));

    let stage1 = cache.stage1(&cut).unwrap();
    let stage2 = cache.stage2(&cut, &mut |_| {}).unwrap();
    let fingerprint = compute_fingerprint(&cut).unwrap();

    // A second run over the same file hits the cache and agrees.
    let stage1_again = cache.stage1(&cut).unwrap();
    assert_eq!(stage1_again.analysis, stage1.analysis);

    // Rename the source away: lookups by fingerprint must still
    // serve both stages, which proves no decode happens on a hit,
    // because there is nothing left to decode.
    let moved: PathBuf = dir.join("cut-moved.mp4");
    std::fs::rename(&cut, &moved).unwrap();
    assert!(!cut.exists());
    let cached1 = cache
        .get_stage1(&fingerprint)
        .expect("stage 1 must be cached");
    assert_eq!(cached1.analysis, stage1.analysis);
    assert!(cached1.poster_path.is_file());
    assert!(cached1.filmstrip_path.is_file());
    let cached2 = cache
        .get_stage2(&fingerprint)
        .expect("stage 2 must be cached");
    assert_eq!(cached2, stage2);

    // Purge empties the cache home.
    cache.purge().unwrap();
    assert!(cache.get_stage1(&fingerprint).is_none());
    assert!(cache.get_stage2(&fingerprint).is_none());
    let remaining = std::fs::read_dir(cache.home()).unwrap().count();
    assert_eq!(remaining, 0, "cache home must be empty after purge");
}
