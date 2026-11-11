//! Integration tests for music video-engine: planner determinism, the named
//! generation controls changing outcomes in the expected direction, the
//! provisional refine contract of the staged pipeline, and multi song
//! stitching.

use std::collections::HashSet;

use ricodex_core::{
    Asset, AssetKind, Fingerprint, GenerationSettings, Project, Tick, Track, TrackKind,
};
use ricodex_engine::*;

// ------------------------------------------------------------- fixtures

fn grid_120bpm(seconds: f64) -> BeatGrid {
    let count = (seconds * 2.0) as usize;
    let beats: Vec<Tick> = (0..count).map(|i| Tick::from_seconds(i as f64 * 0.5)).collect();
    BeatGrid {
        bpm: 120.0,
        duration: Tick::from_seconds(seconds),
        strengths: vec![0.8; beats.len()],
        beats,
    }
}

fn scene(start: f64, duration: f64, motion: Option<f32>, best: bool, coarse: bool) -> SceneInfo {
    SceneInfo {
        start: Tick::from_seconds(start),
        duration: Tick::from_seconds(duration),
        motion,
        best_moment: best,
        coarse,
    }
}

fn source(id: u64, duration: f64, scenes: Vec<SceneInfo>) -> SourceInfo {
    SourceInfo {
        asset_id: id,
        duration: Tick::from_seconds(duration),
        scenes,
        black_ranges: Vec::new(),
    }
}

fn settings(variance: f64, reuse: bool) -> PlannerSettings {
    PlannerSettings {
        generation: GenerationSettings {
            min_clip: Tick::from_seconds(1.5),
            max_clip: Tick::from_seconds(2.5),
            clip_variance: variance,
            reuse_protection: reuse,
            ..GenerationSettings::default()
        },
        ..PlannerSettings::default()
    }
}

fn project_for(sources: &[SourceInfo]) -> Project {
    let mut project = Project::new();
    for s in sources {
        project.assets.push(Asset {
            id: s.asset_id,
            fingerprint: Fingerprint::new(1, 2, 3, 4),
            path_hint: format!("source{}.mp4", s.asset_id),
            kind: AssetKind::Video,
            duration: s.duration,
        });
    }
    project.add_track(Track::new(1, TrackKind::Video, "V1"));
    project
}

fn settled_source(id: u64) -> SourceInfo {
    source(
        id,
        60.0,
        vec![
            scene(0.0, 20.0, Some(0.9), true, false),
            scene(20.0, 20.0, Some(0.5), false, false),
            scene(40.0, 20.0, Some(0.3), false, false),
        ],
    )
}

// ---------------------------------------------------------------- tests

#[test]
fn planner_is_deterministic_and_builds_a_valid_project() {
    let grid = grid_120bpm(16.0);
    let sources = vec![settled_source(1), settled_source(2)];
    let planner = Planner::new(42);
    let settings = PlannerSettings::default();

    let first = planner.plan(&grid, &sources, &settings, 1, 1);
    let second = planner.plan(&grid, &sources, &settings, 1, 1);
    assert_eq!(first, second, "same inputs and seed must give the same plan");
    assert!(first.clips.len() >= 4, "plan too thin: {}", first.clips.len());
    assert!(first.clips.iter().all(|c| c.state == ClipState::Settled));

    // Clips tile the song without gaps or overlaps.
    let mut cursor = Tick::ZERO;
    for clip in &first.clips {
        assert_eq!(clip.start, cursor, "gap or overlap before clip {:?}", clip.item_id);
        cursor = clip.start + clip.duration;
    }
    assert_eq!(cursor, grid.duration);

    let project = first
        .apply(project_for(&sources))
        .expect("plan applies cleanly");
    project.validate().expect("planned project is valid");
    assert_eq!(project.items(1).unwrap().len(), first.clips.len());
}

#[test]
fn variance_changes_start_point_choices() {
    let grid = grid_120bpm(16.0);
    // Uniform scenes: with no score differences, wander is the only
    // thing that can move the picks, which isolates the control.
    let uniform = |id: u64| {
        source(
            id,
            120.0,
            vec![
                scene(0.0, 40.0, Some(0.5), false, false),
                scene(40.0, 40.0, Some(0.5), false, false),
                scene(80.0, 40.0, Some(0.5), false, false),
            ],
        )
    };
    let sources = vec![uniform(1)];
    let planner = Planner::new(7);

    let still = planner.plan(&grid, &sources, &settings(0.0, true), 1, 1);
    let wandering = planner.plan(&grid, &sources, &settings(1.0, true), 1, 1);
    assert_eq!(still.clips.len(), wandering.clips.len());
    let still_ins: Vec<Tick> = still.clips.iter().map(|c| c.source_in).collect();
    let wander_ins: Vec<Tick> = wandering.clips.iter().map(|c| c.source_in).collect();
    assert_ne!(
        still_ins, wander_ins,
        "clip variance 1.0 must move at least one start point"
    );
}

#[test]
fn reuse_protection_prevents_source_span_reuse() {
    let grid = grid_120bpm(16.0);
    // One strong best moment scene, then a long weak tail: without
    // protection the planner keeps returning to the strong scene.
    let one = source(
        1,
        60.0,
        vec![
            scene(0.0, 4.0, Some(1.0), true, false),
            scene(4.0, 56.0, Some(0.1), false, false),
        ],
    );
    let sources = vec![one];
    let planner = Planner::new(3);

    let overlaps = |plan: &Plan| {
        let mut spans: Vec<(Tick, Tick)> = plan
            .clips
            .iter()
            .map(|c| (c.source_in, c.source_in + c.duration))
            .collect();
        spans.sort();
        spans.windows(2).any(|w| w[1].0 < w[0].1)
    };

    let unprotected = planner.plan(&grid, &sources, &settings(0.0, false), 1, 1);
    assert!(
        overlaps(&unprotected),
        "without reuse protection the strong scene should be reused: {:?}",
        unprotected.clips
    );

    let protected = planner.plan(&grid, &sources, &settings(0.0, true), 1, 1);
    assert!(
        !overlaps(&protected),
        "reuse protection must keep every source span distinct: {:?}",
        protected.clips
    );
    assert_ne!(unprotected.clips, protected.clips);
}

#[test]
fn continue_starts_resume_where_the_source_stopped() {
    let grid = grid_120bpm(16.0);
    let one = source(
        1,
        60.0,
        vec![
            scene(0.0, 20.0, Some(0.5), false, false),
            scene(20.0, 20.0, Some(0.5), false, false),
            scene(40.0, 20.0, Some(0.5), false, false),
        ],
    );
    let planner = Planner::new(5);
    let plan = planner.plan(&grid, &[one], &settings(0.0, true), 1, 1);
    assert!(plan.clips.len() >= 3, "clips: {:?}", plan.clips);
    for w in plan.clips.windows(2).take(2) {
        assert_eq!(
            w[1].source_in,
            w[0].source_in + w[0].duration,
            "clip {} should resume where clip {} stopped",
            w[1].item_id,
            w[0].item_id
        );
    }
}

#[test]
fn refine_replaces_unpinned_provisional_clips_and_never_touches_pinned() {
    let grid = grid_120bpm(16.0);
    // Stage 1 data: coarse boundaries, motion not yet measured.
    let partial = |id: u64| {
        source(
            id,
            60.0,
            vec![scene(0.0, 60.0, None, false, true)],
        )
    };
    let stage1 = vec![partial(1), partial(2)];
    let planner = Planner::new(11);
    let settings = settings(0.0, true);

    let rough = planner.plan(&grid, &stage1, &settings, 1, 1);
    assert!(rough.clips.len() >= 3, "rough clips: {:?}", rough.clips);
    assert!(
        rough.clips.iter().all(|c| c.state == ClipState::Provisional),
        "stage 1 data must mark every clip provisional"
    );

    // Stage 2 lands: source 2 turns out to hold the best material.
    let stage2 = vec![
        source(1, 60.0, vec![scene(0.0, 60.0, Some(0.1), false, false)]),
        source(2, 60.0, vec![scene(0.0, 60.0, Some(0.99), true, false)]),
    ];

    let pinned_id = rough.clips[1].item_id;
    let pinned: HashSet<u64> = [pinned_id].into_iter().collect();
    let refined = planner.refine(&rough, &grid, &stage2, &settings, &pinned);

    // The pinned clip is byte for byte the same planned clip.
    let before = rough.clips.iter().find(|c| c.item_id == pinned_id).unwrap();
    let after = refined.clips.iter().find(|c| c.item_id == pinned_id).unwrap();
    assert_eq!(before, after, "pinned clip changed during refine");

    // And byte for byte the same item in the built projects.
    let project_before = rough.apply(project_for(&stage1)).unwrap();
    let project_after = refined.apply(project_for(&stage2)).unwrap();
    assert_eq!(
        project_before.item(pinned_id).unwrap(),
        project_after.item(pinned_id).unwrap(),
        "pinned item changed during refine"
    );
    project_after.validate().unwrap();

    // At least one unpinned provisional clip was replaced by the better
    // source, and replacements resting on finished data settle.
    let replaced = refined
        .clips
        .iter()
        .filter(|c| c.item_id != pinned_id)
        .filter(|c| {
            let old = rough.clips.iter().find(|o| o.item_id == c.item_id).unwrap();
            c.asset_id != old.asset_id || c.source_in != old.source_in
        })
        .count();
    assert!(replaced >= 1, "no unpinned clip was replaced: {:?}", refined.clips);
    assert!(
        refined
            .clips
            .iter()
            .filter(|c| c.asset_id == 2)
            .all(|c| c.state == ClipState::Settled),
        "clips on finished data should settle"
    );
}

#[test]
fn two_song_stitching_produces_one_continuous_timeline() {
    let first = grid_120bpm(4.0);
    let second = grid_120bpm(4.0);
    let stitched = stitch_grids(&first, &second);
    assert_eq!(stitched.beats.len(), 16);
    assert_eq!(stitched.strengths.len(), 16);
    assert_eq!(stitched.duration, Tick::from_seconds(8.0));
    assert_eq!(stitched.beats[8], Tick::from_seconds(4.0));
    for w in stitched.beats.windows(2) {
        assert!(w[1] > w[0], "stitched beats must keep increasing");
        assert_eq!(w[1] - w[0], Tick::from_seconds(0.5), "beat spacing must stay continuous across the join");
    }
}

#[test]
fn analysis_bridges_into_a_plan() {
    // End to end: a real click train analysis feeds the grid, and the
    // planner cuts a settled source on it.
    let rate = 22_050u32;
    let n = (rate as f64 * 8.0) as usize;
    let mut samples = vec![0f32; n];
    let mut beat = 0.5f64;
    while beat < 8.0 {
        let pos = (beat * rate as f64) as usize;
        for j in 0..(rate as usize / 100) {
            if pos + j < n {
                let decay = 1.0 - j as f32 / (rate as f32 / 100.0);
                samples[pos + j] += 0.9 * decay * if j % 2 == 0 { 1.0 } else { -0.6 };
            }
        }
        beat += 0.5;
    }
    let analysis = ricodex_audio::analyse_samples(&samples, rate, &ricodex_audio::AnalysisSettings::default())
        .expect("analyse click train");
    let grid = BeatGrid::from_analysis(&analysis);
    assert!((grid.bpm - 120.0).abs() <= 1.0, "grid bpm {}", grid.bpm);
    assert!(!grid.beats.is_empty());

    let planner = Planner::new(1);
    let plan = planner.plan(
        &grid,
        &[settled_source(1)],
        &PlannerSettings::default(),
        1,
        1,
    );
    assert!(!plan.clips.is_empty());
    let project = plan.apply(project_for(&[settled_source(1)])).unwrap();
    project.validate().unwrap();
}
