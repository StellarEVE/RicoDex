//! ricodex-engine: generation and planning. The montage planner that builds a
//! first cut from song structure and analysed footage, including the
//! provisional mode used while staged analysis is still refining.
//!
//! The planner is a pure function of its inputs plus an internal seed:
//! same grid, same sources, same settings, same seed, same plan. It takes
//! a beat grid (bridged from ricodex-audio by [`BeatGrid::from_analysis`]), a
//! pool of analysed sources as plain data, and the generation settings
//! from ricodex-core, extended locally with the placement controls that are
//! the planner's own business. Its output is a [`Plan`]: the chosen clips
//! as data, each marked provisional or settled, plus the ricodex-core
//! commands that build exactly those clips on a track.
//!
//! Provisional mode is the staged pipeline contract. While analysis is
//! still coarse (Stage 1 sampling rather than the full Stage 2 pass),
//! sources report partial data and every clip drawn from them is
//! provisional. [`Planner::refine`] then takes updated analysis and
//! improves the plan under two rules: pinned items never change, byte
//! for byte, and only unpinned provisional clips may be replaced.

use std::collections::{HashMap, HashSet};

use ricodex_audio::SongAnalysis;
use ricodex_core::{
    AssetId, Command, CommandError, GenerationSettings, Item, ItemId, Project, Tick, TrackId,
};

// ------------------------------------------------------------------- grid

/// A beat grid as the planner consumes it: beat times and strengths in
/// tick time, the song duration, and the estimated tempo.
#[derive(Clone, Debug, PartialEq)]
pub struct BeatGrid {
    pub bpm: f64,
    pub duration: Tick,
    pub beats: Vec<Tick>,
    pub strengths: Vec<f32>,
}

impl BeatGrid {
    /// Bridge a ricodex-audio analysis into a planner grid.
    pub fn from_analysis(analysis: &SongAnalysis) -> BeatGrid {
        BeatGrid {
            bpm: analysis.bpm,
            duration: analysis.duration,
            beats: analysis.beats.iter().map(|b| b.time).collect(),
            strengths: analysis.beats.iter().map(|b| b.strength).collect(),
        }
    }

    /// Typical spacing between neighbouring beats: the median gap, or
    /// the tempo period when the grid has fewer than two beats.
    pub fn beat_spacing(&self) -> Tick {
        if self.beats.len() >= 2 {
            let mut gaps: Vec<i64> = self
                .beats
                .windows(2)
                .map(|w| (w[1] - w[0]).as_ticks())
                .collect();
            gaps.sort_unstable();
            let median = gaps[gaps.len() / 2];
            if median > 0 {
                return Tick::from_ticks(median);
            }
        }
        if self.bpm > 0.0 {
            return Tick::from_seconds(60.0 / self.bpm);
        }
        Tick::from_seconds(0.5)
    }
}

/// Stitch two grids end to end into one continuous timeline: the second
/// song's beats are offset by the first song's duration, and the result
/// lasts the sum of both durations. The tempo reported is the first
/// song's, as the opening tempo of the combined timeline.
pub fn stitch_grids(first: &BeatGrid, second: &BeatGrid) -> BeatGrid {
    let mut beats = first.beats.clone();
    let mut strengths = first.strengths.clone();
    beats.extend(second.beats.iter().map(|t| *t + first.duration));
    strengths.extend(second.strengths.iter().copied());
    BeatGrid {
        bpm: first.bpm,
        duration: first.duration + second.duration,
        beats,
        strengths,
    }
}

/// Stitch two song analyses into one continuous planner grid.
pub fn stitch_analyses(first: &SongAnalysis, second: &SongAnalysis) -> BeatGrid {
    stitch_grids(&BeatGrid::from_analysis(first), &BeatGrid::from_analysis(second))
}

// ---------------------------------------------------------------- sources

/// One analysed scene of a source: its range, a motion score when the
/// analysis stage that measures motion has run, whether it holds a best
/// moment, and whether its boundaries are still coarse Stage 1 values.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneInfo {
    pub start: Tick,
    pub duration: Tick,
    pub motion: Option<f32>,
    pub best_moment: bool,
    pub coarse: bool,
}

impl SceneInfo {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// A range of a source that is black, or near enough to black that the
/// planner should not cut into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlackRange {
    pub start: Tick,
    pub duration: Tick,
}

impl BlackRange {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// One analysed source in the pool the planner draws from.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    pub asset_id: AssetId,
    pub duration: Tick,
    pub scenes: Vec<SceneInfo>,
    pub black_ranges: Vec<BlackRange>,
}

impl SourceInfo {
    /// Partial analysis is what the staged pipeline hands the planner
    /// before Stage 2 lands: no scenes yet, any scene still coarse, or
    /// motion not yet measured. Clips drawn from a partial source are
    /// provisional.
    pub fn is_partial(&self) -> bool {
        self.scenes.is_empty()
            || self
                .scenes
                .iter()
                .any(|s| s.coarse || s.motion.is_none())
    }
}

// --------------------------------------------------------------- settings

/// How the planner walks the sources when it places clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementMode {
    /// Pure scored montage: the best scoring window wins each slot.
    Scored,
    /// Each beat searches around the matching relative position in its
    /// source, the Beats2Fun style windowed walk.
    ProportionalWalk,
    /// Sources are divided into evenly spaced windows, one per slot,
    /// the ricodex_Maker style placement.
    EvenSpaced,
}

/// Planner controls: the ricodex-core generation settings plus the
/// placement controls that belong to the planner itself.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannerSettings {
    pub generation: GenerationSettings,
    pub mode: PlacementMode,
    pub beats_per_bar: u32,
    /// Prefer windows far from sections of a source already used.
    pub spread: bool,
    /// Snap cut starts shifted by the cut offset to the nearest bar
    /// boundary when one is within half a beat.
    pub snap_cut_offset_to_bar: bool,
}

impl Default for PlannerSettings {
    fn default() -> Self {
        PlannerSettings {
            generation: GenerationSettings::default(),
            mode: PlacementMode::Scored,
            beats_per_bar: 4,
            spread: true,
            snap_cut_offset_to_bar: true,
        }
    }
}

// ----------------------------------------------------------------- output

/// Whether a planned clip rests on finished analysis or is still a
/// Stage 1 approximation that refinement may replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipState {
    Provisional,
    Settled,
}

/// One chosen clip, as data. Mirrors the ricodex-core [`Item`] it becomes,
/// plus its provisional state, which lives only in the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedClip {
    pub item_id: ItemId,
    pub asset_id: AssetId,
    pub start: Tick,
    pub duration: Tick,
    pub source_in: Tick,
    pub state: ClipState,
}

/// A planner output: the clips, and the ricodex-core commands that build
/// exactly those clips, in order, on `track_id`.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub track_id: TrackId,
    pub clips: Vec<PlannedClip>,
    pub commands: Vec<Command>,
}

impl Plan {
    /// Apply the plan's commands to a project, through the core
    /// clone, validate, commit path.
    pub fn apply(&self, project: Project) -> Result<Project, EngineError> {
        let mut project = project;
        for command in &self.commands {
            project = project
                .apply(command)
                .map_err(|(_, e)| EngineError::Command(e))?;
        }
        Ok(project)
    }

    fn from_clips(track_id: TrackId, clips: Vec<PlannedClip>) -> Plan {
        let commands = clips
            .iter()
            .map(|clip| Command::InsertItem {
                track_id,
                item: Item::new(
                    clip.item_id,
                    clip.asset_id,
                    clip.start,
                    clip.duration,
                    clip.source_in,
                ),
            })
            .collect();
        Plan {
            track_id,
            clips,
            commands,
        }
    }
}

/// Why applying a plan failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    Command(CommandError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Command(e) => write!(f, "plan command rejected: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

// -------------------------------------------------------------------- rng

/// SplitMix64: a small deterministic generator. The planner draws all
/// its randomness from one of these, seeded once per operation, so a
/// plan is a pure function of its inputs and the planner seed.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in 0.0..1.0.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------- planner

/// The montage planner. Holds only the internal seed; every operation
/// reseeds from it, so repeated calls with the same inputs agree.
#[derive(Clone, Debug)]
pub struct Planner {
    seed: u64,
}

impl Planner {
    pub fn new(seed: u64) -> Planner {
        Planner { seed }
    }

    /// Build a first cut: slots on the beat grid filled with the best
    /// scoring source windows, honouring every generation control.
    pub fn plan(
        &self,
        grid: &BeatGrid,
        sources: &[SourceInfo],
        settings: &PlannerSettings,
        track_id: TrackId,
        first_item_id: ItemId,
    ) -> Plan {
        let mut rng = Rng::new(self.seed);
        let slots = compute_slots(grid, settings, &mut rng);
        let mut used: HashMap<AssetId, Vec<(Tick, Tick)>> = HashMap::new();
        let mut last_end: HashMap<AssetId, Tick> = HashMap::new();
        let partial: HashMap<AssetId, bool> =
            sources.iter().map(|s| (s.asset_id, s.is_partial())).collect();

        let mut clips = Vec::new();
        for (index, &(start, end)) in slots.iter().enumerate() {
            let duration = end - start;
            let ctx = ScoreCtx {
                settings,
                grid,
                slot_start: start,
                slot_index: index,
                slot_count: slots.len(),
            };
            let Some((asset_id, source_in, _)) =
                choose_window(&ctx, duration, sources, &used, &last_end, &mut rng, true)
            else {
                continue;
            };
            used.entry(asset_id).or_default().push((source_in, source_in + duration));
            last_end.insert(asset_id, source_in + duration);
            clips.push(PlannedClip {
                item_id: first_item_id + clips.len() as ItemId,
                asset_id,
                start,
                duration,
                source_in,
                state: if partial.get(&asset_id).copied().unwrap_or(true) {
                    ClipState::Provisional
                } else {
                    ClipState::Settled
                },
            });
        }
        Plan::from_clips(track_id, clips)
    }

    /// Refine a plan with updated analysis. Pinned clips and settled
    /// clips are carried over byte for byte; an unpinned provisional
    /// clip is replaced only when the updated data scores a different
    /// window strictly better for its slot, and settles when the source
    /// it rests on is no longer partial. Spans held by kept clips count
    /// as used, so replacements respect reuse protection against them.
    pub fn refine(
        &self,
        previous: &Plan,
        grid: &BeatGrid,
        sources: &[SourceInfo],
        settings: &PlannerSettings,
        pinned: &HashSet<ItemId>,
    ) -> Plan {
        let mut rng = Rng::new(self.seed);
        let by_id: HashMap<AssetId, &SourceInfo> =
            sources.iter().map(|s| (s.asset_id, s)).collect();

        // Spans of clips that cannot change are used from the start.
        let mut used: HashMap<AssetId, Vec<(Tick, Tick)>> = HashMap::new();
        let mut last_end: HashMap<AssetId, Tick> = HashMap::new();
        for clip in &previous.clips {
            if pinned.contains(&clip.item_id) || clip.state == ClipState::Settled {
                used.entry(clip.asset_id)
                    .or_default()
                    .push((clip.source_in, clip.source_in + clip.duration));
                last_end.insert(clip.asset_id, clip.source_in + clip.duration);
            }
        }

        let slot_count = previous.clips.len();
        let mut clips = Vec::with_capacity(slot_count);
        for (index, clip) in previous.clips.iter().enumerate() {
            if pinned.contains(&clip.item_id) || clip.state == ClipState::Settled {
                clips.push(clip.clone());
                continue;
            }
            let ctx = ScoreCtx {
                settings,
                grid,
                slot_start: clip.start,
                slot_index: index,
                slot_count,
            };
            // Refine compares exact scores, no variance wander, so a
            // clip is only replaced when the new data is genuinely
            // better for its slot, never on a dice roll.
            let current_score = score_window(
                &ctx,
                clip.asset_id,
                clip.source_in,
                clip.duration,
                sources,
                &used,
                &last_end,
                None,
            );
            let best = choose_window(&ctx, clip.duration, sources, &used, &last_end, &mut rng, false);
            let replace = match (best, current_score) {
                (Some((asset_id, source_in, best_score)), Some(score)) => {
                    (asset_id != clip.asset_id || source_in != clip.source_in)
                        && best_score > score + 1e-9
                }
                (Some(_), None) => true,
                _ => false,
            };
            let chosen = if replace {
                let (asset_id, source_in, _) = best.unwrap();
                PlannedClip {
                    item_id: clip.item_id,
                    asset_id,
                    start: clip.start,
                    duration: clip.duration,
                    source_in,
                    state: state_for(&by_id, asset_id),
                }
            } else {
                PlannedClip {
                    state: state_for(&by_id, clip.asset_id),
                    ..clip.clone()
                }
            };
            used.entry(chosen.asset_id)
                .or_default()
                .push((chosen.source_in, chosen.source_in + chosen.duration));
            last_end.insert(chosen.asset_id, chosen.source_in + chosen.duration);
            clips.push(chosen);
        }
        Plan::from_clips(previous.track_id, clips)
    }
}

fn state_for(by_id: &HashMap<AssetId, &SourceInfo>, asset_id: AssetId) -> ClipState {
    match by_id.get(&asset_id) {
        Some(source) if !source.is_partial() => ClipState::Settled,
        _ => ClipState::Provisional,
    }
}

// ------------------------------------------------------------------ slots

/// Cut the song into slots on the beat grid. Slot boundaries fall on
/// beats, slot lengths land between min and max clip, steered by a
/// seeded target inside that range, and a short tail merges into the
/// last slot instead of becoming a stub. The cut offset then shifts the
/// starts, optionally snapped to bar boundaries, with the ends tracking
/// the next start so coverage never gaps.
fn compute_slots(grid: &BeatGrid, settings: &PlannerSettings, rng: &mut Rng) -> Vec<(Tick, Tick)> {
    if grid.beats.is_empty() || grid.duration <= Tick::ZERO {
        return Vec::new();
    }
    let gen = &settings.generation;
    let min_clip = if gen.min_clip > Tick::ZERO { gen.min_clip } else { Tick::from_seconds(0.1) };
    let max_clip = if gen.max_clip >= min_clip { gen.max_clip } else { min_clip };

    let mut points: Vec<Tick> = Vec::new();
    points.push(Tick::ZERO);
    for &beat in &grid.beats {
        if beat > Tick::ZERO && beat < grid.duration {
            points.push(beat);
        }
    }
    points.push(grid.duration);
    points.dedup();

    let mut slots = Vec::new();
    let mut i = 0usize;
    while i + 1 < points.len() {
        let start = points[i];
        let target = min_clip.as_ticks() as f64
            + rng.next_f64() * (max_clip.as_ticks() - min_clip.as_ticks()) as f64;
        let mut chosen = i + 1;
        let mut best_diff = f64::INFINITY;
        for j in i + 1..points.len() {
            let dur = points[j] - start;
            if dur > max_clip && j > i + 1 {
                break;
            }
            let diff = (dur.as_ticks() as f64 - target).abs();
            if dur >= min_clip && diff < best_diff {
                best_diff = diff;
                chosen = j;
            }
            if dur >= max_clip {
                break;
            }
        }
        let mut end = points[chosen];
        // A tail shorter than min_clip merges into this slot.
        if end < grid.duration && grid.duration - end < min_clip {
            end = grid.duration;
        }
        slots.push((start, end));
        if end >= grid.duration {
            break;
        }
        i = points.iter().position(|&p| p == end).unwrap_or(i + 1);
    }

    if gen.cut_offset != Tick::ZERO && slots.len() > 1 {
        let spacing = grid.beat_spacing();
        let bar_starts: Vec<Tick> = if settings.snap_cut_offset_to_bar && settings.beats_per_bar > 0 {
            grid.beats
                .iter()
                .step_by(settings.beats_per_bar as usize)
                .copied()
                .collect()
        } else {
            Vec::new()
        };
        let mut starts: Vec<Tick> = slots.iter().map(|&(s, _)| s).collect();
        for k in 1..starts.len() {
            let original = starts[k];
            let mut shifted = original + gen.cut_offset;
            if shifted <= starts[k - 1] {
                shifted = original;
            }
            if settings.snap_cut_offset_to_bar {
                if let Some(&bar) = bar_starts
                    .iter()
                    .min_by_key(|&&b| (b - shifted).as_ticks().abs())
                {
                    if (bar - shifted).as_ticks().abs() <= spacing.as_ticks() / 2 && bar > starts[k - 1] {
                        shifted = bar;
                    }
                }
            }
            starts[k] = shifted;
        }
        for k in 0..slots.len() {
            let end = if k + 1 < slots.len() { starts[k + 1] } else { grid.duration };
            slots[k] = (starts[k], end);
        }
    }
    slots
}

// -------------------------------------------------------------- selection

/// Everything scoring needs to know about the slot being filled.
struct ScoreCtx<'a> {
    settings: &'a PlannerSettings,
    grid: &'a BeatGrid,
    slot_start: Tick,
    slot_index: usize,
    slot_count: usize,
}

fn overlap(a_start: Tick, a_end: Tick, b_start: Tick, b_end: Tick) -> Tick {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    if end > start {
        end - start
    } else {
        Tick::ZERO
    }
}

/// Score one candidate window. With `jitter` present, consumes one draw
/// and adds the variance wander; without, scores the window exactly,
/// which is what comparisons in refine use.
#[allow(clippy::too_many_arguments)]
fn score_window(
    ctx: &ScoreCtx,
    asset_id: AssetId,
    source_in: Tick,
    duration: Tick,
    sources: &[SourceInfo],
    used: &HashMap<AssetId, Vec<(Tick, Tick)>>,
    last_end: &HashMap<AssetId, Tick>,
    jitter: Option<f64>,
) -> Option<f64> {
    let source = sources.iter().find(|s| s.asset_id == asset_id)?;
    let win_start = source_in;
    let win_end = source_in + duration;
    if win_start < Tick::ZERO || win_end > source.duration {
        return None;
    }
    let gen = &ctx.settings.generation;

    // The scene this window mostly lives in drives alignment and motion.
    let mut best_scene: Option<&SceneInfo> = None;
    let mut best_overlap = Tick::ZERO;
    for scene in &source.scenes {
        let o = overlap(win_start, win_end, scene.start, scene.end());
        if o > best_overlap {
            best_overlap = o;
            best_scene = Some(scene);
        }
    }
    let coverage = if duration > Tick::ZERO {
        best_overlap.as_ticks() as f64 / duration.as_ticks() as f64
    } else {
        0.0
    };
    let mut score = 1.2 * coverage;
    match best_scene {
        Some(scene) => match scene.motion {
            Some(motion) => score += 0.9 * motion as f64,
            None => score += 0.25,
        },
        None => {}
    }
    let half = Tick::from_ticks(duration.as_ticks() / 2);
    let best_moment = source
        .scenes
        .iter()
        .filter(|s| s.best_moment)
        .any(|s| overlap(win_start, win_end, s.start, s.end()) >= half);
    if best_moment {
        score += 0.6;
    }

    // Continue starts: resuming exactly where this source last stopped.
    let continues = last_end.get(&asset_id).copied() == Some(win_start);
    if continues {
        score += gen.continue_starts_bonus;
    }

    // Placement modes steer toward an ideal position in the source.
    let ideal = match ctx.settings.mode {
        PlacementMode::Scored => None,
        PlacementMode::ProportionalWalk => {
            let frac = if ctx.grid.duration > Tick::ZERO {
                ctx.slot_start.as_ticks() as f64 / ctx.grid.duration.as_ticks() as f64
            } else {
                0.0
            };
            Some(Tick::from_ticks(
                (frac * (source.duration - duration).as_ticks() as f64) as i64,
            ))
        }
        PlacementMode::EvenSpaced => {
            let count = ctx.slot_count.max(1) as i64;
            let raw = source.duration.as_ticks() * ctx.slot_index as i64 / count;
            Some(Tick::from_ticks(raw.min((source.duration - duration).as_ticks().max(0))))
        }
    };
    if let Some(ideal) = ideal {
        let distance = (win_start - ideal).as_ticks().abs() as f64;
        let range = source.duration.as_ticks().max(1) as f64;
        score += 0.7 * (1.0 - distance / range).clamp(0.0, 1.0);
    }

    // Spread: windows close to, but not continuing from, a used span of
    // the same source are penalised, pushing picks into distant
    // sections. Adjacent windows are the continue case, not clustering.
    if ctx.settings.spread && !continues {
        if let Some(spans) = used.get(&asset_id) {
            let mut nearest: Option<i64> = None;
            for &(s, e) in spans {
                let distance = if win_end <= s {
                    (s - win_end).as_ticks()
                } else if win_start >= e {
                    (win_start - e).as_ticks()
                } else {
                    0
                };
                nearest = Some(nearest.map(|n| n.min(distance)).unwrap_or(distance));
            }
            if let Some(distance) = nearest {
                let dur = duration.as_ticks().max(1);
                if distance > 0 && distance < dur {
                    score -= 0.5 * (1.0 - distance as f64 / dur as f64);
                }
            }
        }
    }

    if let Some(draw) = jitter {
        score += gen.clip_variance * (draw * 2.0 - 1.0) * 0.5;
    }
    Some(score)
}

/// Whether a candidate survives the hard filters at a cascade stage.
/// Stage 0 enforces black ranges and reuse protection, stage 1 drops
/// reuse protection, stage 2 drops both, so a slot is filled whenever
/// any window exists at all.
fn passes_filters(
    stage: u32,
    settings: &PlannerSettings,
    asset_id: AssetId,
    source_in: Tick,
    duration: Tick,
    source: &SourceInfo,
    used: &HashMap<AssetId, Vec<(Tick, Tick)>>,
) -> bool {
    let win_end = source_in + duration;
    if stage < 2 {
        let black: Tick = source
            .black_ranges
            .iter()
            .map(|b| overlap(source_in, win_end, b.start, b.end()))
            .fold(Tick::ZERO, |a, b| a + b);
        if black.as_ticks() * 10 > duration.as_ticks() {
            return false;
        }
    }
    if stage < 1 && settings.generation.reuse_protection {
        if let Some(spans) = used.get(&asset_id) {
            for &(s, e) in spans {
                if overlap(source_in, win_end, s, e) > Tick::ZERO {
                    return false;
                }
            }
        }
    }
    true
}

/// Choose the best window for one slot across the whole pool. Candidate
/// starts sit on scene starts, on a beat spaced lattice across each
/// source, and on each source's continue point, so every named control
/// has candidates to act on.
fn choose_window(
    ctx: &ScoreCtx,
    duration: Tick,
    sources: &[SourceInfo],
    used: &HashMap<AssetId, Vec<(Tick, Tick)>>,
    last_end: &HashMap<AssetId, Tick>,
    rng: &mut Rng,
    use_jitter: bool,
) -> Option<(AssetId, Tick, f64)> {
    let spacing = ctx.grid.beat_spacing();
    for stage in 0..3 {
        let mut best: Option<(f64, AssetId, Tick)> = None;
        for source in sources {
            if source.duration < duration {
                continue;
            }
            let mut starts: Vec<Tick> = Vec::new();
            for scene in &source.scenes {
                starts.push(scene.start);
            }
            let mut t = Tick::ZERO;
            while t + duration <= source.duration {
                starts.push(t);
                if spacing <= Tick::ZERO {
                    break;
                }
                t = t + spacing;
            }
            if let Some(&end) = last_end.get(&source.asset_id) {
                starts.push(end);
            }
            starts.sort_unstable();
            starts.dedup();
            for start in starts {
                if start < Tick::ZERO || start + duration > source.duration {
                    continue;
                }
                if !passes_filters(stage, ctx.settings, source.asset_id, start, duration, source, used) {
                    continue;
                }
                let jitter = if use_jitter { Some(rng.next_f64()) } else { None };
                if let Some(score) = score_window(
                    ctx,
                    source.asset_id,
                    start,
                    duration,
                    sources,
                    used,
                    last_end,
                    jitter,
                ) {
                    if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                        best = Some((score, source.asset_id, start));
                    }
                }
            }
        }
        if let Some((score, asset_id, start)) = best {
            return Some((asset_id, start, score));
        }
    }
    None
}
