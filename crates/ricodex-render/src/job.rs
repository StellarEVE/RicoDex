//! The render job: one project, one video track, two clips, one
//! transition, one MP4 out. This is the first render path of the
//! new foundation, deliberately narrow:
//!
//! Scope of this slice. The project must hold exactly one video
//! track with exactly two enabled items placed back to back, from
//! two source files. The transition overlaps the cut in the usual
//! editor way: it consumes its duration from the combined length,
//! so the output runs item A plus item B minus the transition
//! duration, with the incoming clip starting under the outgoing
//! one. The request's effect stack applies to every frame. Audio
//! remains out of scope for this two-clip transition renderer; the
//! sequential application export has a separate selected-song AAC path.
//! Wider projects (more tracks, more items, per clip effects) are later slices
//! building on this path, not changes to it.
//!
//! Frames are decoded through ricodex-platform, composited through
//! the GPU compositor where it implements the transition and the
//! CPU oracle otherwise, and encoded through ricodex-platform's
//! VideoEncoder. The transition region goes through the content
//! addressed render cache when one is supplied.

use std::cell::Cell;
use std::path::PathBuf;

use ricodex_core::{AssetKind, Item, Project, Tick, TrackId, TrackKind, TICKS_PER_SECOND};
use ricodex_platform::VideoDecoder;

use crate::cache::{RenderCache, TransitionUnitKey};
use crate::composite::{composite_cpu, composite_video_layers};
use crate::effects::{apply_effects, describe_stack, EffectOp};
use crate::error::RenderError;
use crate::frame::Frame;
use crate::gpu::GpuCompositor;
use crate::transition::{TransitionKind, TransitionParams};
use crate::timeline::{TimelineTransitionMap, TimelineTransitionSegment};

/// Stereo source audio supplied to the sequential export. Samples are
/// interleaved left/right f32 values at the source sample rate.
#[derive(Clone, Copy, Debug)]
pub struct StereoAudioInput<'a> {
    pub interleaved_samples: &'a [f32],
    pub sample_rate: u32,
}

/// Decoded stereo PCM for one audio asset used by an Audio-track item.
#[derive(Clone, Copy, Debug)]
pub struct StereoAudioAssetInput<'a> {
    pub asset_id: u64,
    pub interleaved_samples: &'a [f32],
    pub sample_rate: u32,
}

struct PreparedAudioTrackItem<'a> {
    item: Item,
    interleaved_samples: &'a [f32],
    sample_rate: u32,
}

fn sample_stereo_at_tick(samples: &[f32], sample_rate: u32, time: Tick) -> [f32; 2] {
    if sample_rate == 0 || time < Tick::ZERO || samples.len() < 2 || samples.len() % 2 != 0 {
        return [0.0, 0.0];
    }
    let numerator = time.as_ticks() as i128 * sample_rate as i128;
    let denominator = TICKS_PER_SECOND as i128;
    let source_index = (numerator / denominator) as usize;
    let fraction = (numerator % denominator) as f32 / denominator as f32;
    let frame_count = samples.len() / 2;
    let next_index = source_index.saturating_add(1);
    let sample = |index: usize, channel: usize| -> f32 {
        if index < frame_count { samples[index * 2 + channel] } else { 0.0 }
    };
    [
        sample(source_index, 0) + (sample(next_index, 0) - sample(source_index, 0)) * fraction,
        sample(source_index, 1) + (sample(next_index, 1) - sample(source_index, 1)) * fraction,
    ]
}

/// The transition to render between the two items: its form, its
/// parameters, and its duration on the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionSpec {
    pub kind: TransitionKind,
    pub params: TransitionParams,
    pub duration: Tick,
}

/// A render request bridging from the core project: the project
/// supplies the assets, the items and the export settings; the
/// request adds the track, the transition and the output path.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub project: Project,
    pub track_id: TrackId,
    pub transition: TransitionSpec,
    /// Applied to every frame of the output, in stack order.
    pub effects: Vec<EffectOp>,
    pub output_path: PathBuf,
}

/// What a finished render job can report about itself.
#[derive(Clone, Debug)]
pub struct RenderOutcome {
    pub output_path: PathBuf,
    pub frames_written: u64,
    /// Output duration on the timeline (A plus B minus transition).
    pub duration: Tick,
    /// True when at least one transition frame came from the GPU
    /// compositor (false on a full cache hit, where nothing is
    /// composited at all).
    pub used_gpu: bool,
    /// True when the transition region came from the render cache.
    pub cache_hit: bool,
    /// The H.264 encoder the platform resolved to.
    pub encoder_name: String,
}

/// Whole output frames in a duration at a rational frame rate,
/// rounded to the nearest frame.
fn frames_in(duration: Tick, fps_num: u32, fps_den: u32) -> usize {
    let numerator = duration.as_ticks() as i128 * fps_num as i128;
    let denominator = TICKS_PER_SECOND as i128 * fps_den as i128;
    ((numerator + denominator / 2) / denominator).max(0) as usize
}

/// The tick at the start of output frame `index`.
fn frame_start(index: usize, fps_num: u32, fps_den: u32) -> Tick {
    let numerator = index as i128 * TICKS_PER_SECOND as i128 * fps_den as i128;
    let denominator = fps_num as i128;
    Tick::from_ticks(((numerator + denominator / 2) / denominator) as i64)
}

/// Decode the frames of one item range at the export size and frame
/// rate: one output frame per slot, each holding the latest decoded
/// frame at or before the slot's source time.
fn decode_item_frames(
    path: &std::path::Path,
    source_in: Tick,
    duration: Tick,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
) -> Result<Vec<Frame>, RenderError> {
    let slots = frames_in(duration, fps_num, fps_den);
    if slots == 0 {
        return Err(RenderError::BadProject {
            reason: "item duration rounds to zero output frames".to_string(),
        });
    }
    let mut decoder = VideoDecoder::open(path)?;
    decoder.seek(source_in)?;
    let end = source_in + duration;
    let mut decoded: Vec<(Tick, Frame)> = Vec::new();
    while let Some(rgb) = decoder.next_rgb_frame(width)? {
        let frame = Frame::from_rgb24(rgb.width, rgb.height, &rgb.pixels)?.resized(width, height);
        let time = rgb.time;
        decoded.push((time, frame));
        if time >= end {
            break;
        }
    }
    if decoded.is_empty() {
        return Err(RenderError::BadProject {
            reason: format!("decoder produced no frames for {}", path.display()),
        });
    }
    let mut out = Vec::with_capacity(slots);
    for index in 0..slots {
        let target = source_in + frame_start(index, fps_num, fps_den);
        let mut chosen = &decoded[0].1;
        for (time, frame) in &decoded {
            if *time <= target {
                chosen = frame;
            } else {
                break;
            }
        }
        out.push(chosen.clone());
    }
    Ok(out)
}

/// Run the render job. `progress` fires after every encoded frame
/// with the fraction complete, ending at exactly 1.0.
pub fn render_project(
    request: &RenderRequest,
    cache: Option<&RenderCache>,
    progress: &mut dyn FnMut(f32),
) -> Result<RenderOutcome, RenderError> {
    let project = &request.project;
    project.validate().map_err(|e| RenderError::BadProject {
        reason: e.to_string(),
    })?;
    let track = project
        .track(request.track_id)
        .ok_or_else(|| RenderError::BadProject {
            reason: format!("no track {}", request.track_id),
        })?;
    if track.kind != TrackKind::Video {
        return Err(RenderError::BadProject {
            reason: "the render job renders a video track".to_string(),
        });
    }
    let items = project
        .items(request.track_id)
        .ok_or_else(|| RenderError::BadProject {
            reason: "track has no item group".to_string(),
        })?;
    if items.len() != 2 {
        return Err(RenderError::BadProject {
            reason: format!(
                "this render slice takes exactly two items, found {}",
                items.len()
            ),
        });
    }
    let (item_a, item_b) = (&items[0], &items[1]);
    if !item_a.enabled || !item_b.enabled {
        return Err(RenderError::BadProject {
            reason: "both items must be enabled".to_string(),
        });
    }
    if item_b.start != item_a.end() {
        return Err(RenderError::BadProject {
            reason: "the two items must sit back to back".to_string(),
        });
    }
    let tdur = request.transition.duration;
    if tdur <= Tick::ZERO || tdur >= item_a.duration || tdur >= item_b.duration {
        return Err(RenderError::BadProject {
            reason: "transition duration must be positive and shorter than both items"
                .to_string(),
        });
    }
    let asset_path = |asset_id: u64| -> Result<PathBuf, RenderError> {
        project
            .assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| PathBuf::from(&a.path_hint))
            .ok_or_else(|| RenderError::BadProject {
                reason: format!("item points at unknown asset {asset_id}"),
            })
    };
    let path_a = asset_path(item_a.asset_id)?;
    let path_b = asset_path(item_b.asset_id)?;

    let settings = &project.export_settings;
    let (width, height) = (settings.width, settings.height);
    let (fps_num, fps_den) = (settings.frame_rate_num, settings.frame_rate_den);

    let frames_a = decode_item_frames(
        &path_a, item_a.source_in, item_a.duration, width, height, fps_num, fps_den,
    )?;
    let frames_b = decode_item_frames(
        &path_b, item_b.source_in, item_b.duration, width, height, fps_num, fps_den,
    )?;
    // The effect stack applies to the source frames once, before
    // compositing, so cached transition units already carry it.
    let frames_a: Vec<Frame> = frames_a
        .iter()
        .map(|f| apply_effects(f, &request.effects))
        .collect();
    let frames_b: Vec<Frame> = frames_b
        .iter()
        .map(|f| apply_effects(f, &request.effects))
        .collect();

    let n_a = frames_a.len();
    let n_b = frames_b.len();
    let n_t = frames_in(tdur, fps_num, fps_den);
    if n_t == 0 || n_t >= n_a || n_t >= n_b {
        return Err(RenderError::BadProject {
            reason: "transition duration rounds to an unusable frame count".to_string(),
        });
    }

    let asset_fingerprint = |asset_id: u64| {
        project
            .assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| a.fingerprint)
    };
    let key = TransitionUnitKey {
        fingerprint_a: asset_fingerprint(item_a.asset_id).unwrap(),
        fingerprint_b: asset_fingerprint(item_b.asset_id).unwrap(),
        range_a: (item_a.source_in + (item_a.duration - tdur), tdur),
        range_b: (item_b.source_in, tdur),
        transition: request.transition.kind,
        params: request.transition.params,
        transition_duration: tdur,
        width,
        height,
        frame_rate_num: fps_num,
        frame_rate_den: fps_den,
        effects: describe_stack(&request.effects),
    };

    let used_gpu = Cell::new(false);
    let build_unit = |gpu: &Option<GpuCompositor>| -> Result<Vec<Frame>, RenderError> {
        let mut unit = Vec::with_capacity(n_t);
        for i in 0..n_t {
            let fa = &frames_a[n_a - n_t + i];
            let fb = &frames_b[i];
            let p = (i as f32 + 0.5) / n_t as f32;
            let frame = match gpu {
                Some(gpu) if GpuCompositor::supports(request.transition.kind) => {
                    let rendered =
                        gpu.composite(fa, fb, request.transition.kind, p, &request.transition.params)?;
                    used_gpu.set(true);
                    rendered
                }
                _ => composite_cpu(fa, fb, request.transition.kind, p, &request.transition.params)?,
            };
            unit.push(frame);
        }
        Ok(unit)
    };

    let gpu = GpuCompositor::new();
    let (unit, cache_hit) = match cache {
        Some(cache) => cache.render_unit(&key, || build_unit(&gpu))?,
        None => (build_unit(&gpu)?, false),
    };

    let total = n_a + n_b - n_t;
    let mut encoder = ricodex_platform::VideoEncoder::create(
        &request.output_path,
        width,
        height,
        fps_num,
        fps_den,
        settings.video_bitrate_kbps,
    )?;
    let encoder_name = encoder.encoder_name().to_string();
    let mut written = 0usize;
    let mut emit = |frame: &Frame,
                    encoder: &mut ricodex_platform::VideoEncoder,
                    written: &mut usize|
     -> Result<(), RenderError> {
        encoder.encode_rgb24(&frame.to_rgb24())?;
        *written += 1;
        progress(*written as f32 / total as f32);
        Ok(())
    };
    for frame in &frames_a[..n_a - n_t] {
        emit(frame, &mut encoder, &mut written)?;
    }
    for frame in &unit {
        emit(frame, &mut encoder, &mut written)?;
    }
    for frame in &frames_b[n_t..] {
        emit(frame, &mut encoder, &mut written)?;
    }
    encoder.finish()?;

    Ok(RenderOutcome {
        output_path: request.output_path.clone(),
        frames_written: written as u64,
        duration: item_a.duration + item_b.duration - tdur,
        used_gpu: used_gpu.get(),
        cache_hit,
        encoder_name,
    })
}

// ------------------------------------------------------- sequential export

/// Maintains one video track's decoder and advances it in timeline order.
struct TrackVideoReader {
    track_id: TrackId,
    items: Vec<(ricodex_core::Item, PathBuf)>,
    item_index: usize,
    decoder: Option<VideoDecoder>,
    last: Option<Frame>,
    pending: Option<(Tick, Frame)>,
    exhausted: bool,
}

impl TrackVideoReader {
    fn new(project: &Project, track_id: TrackId) -> Result<Self, RenderError> {
        let track = project.track(track_id).ok_or_else(|| RenderError::BadProject {
            reason: format!("no track {track_id}"),
        })?;
        if track.kind != TrackKind::Video {
            return Err(RenderError::BadProject {
                reason: format!("track {track_id} is not a video track"),
            });
        }
        let all_items = project.items(track_id).ok_or_else(|| RenderError::BadProject {
            reason: format!("track {track_id} has no item group"),
        })?;
        let mut items = Vec::new();
        for item in all_items.iter().filter(|item| item.enabled) {
            let asset = project.assets.iter().find(|asset| asset.id == item.asset_id)
                .ok_or_else(|| RenderError::BadProject {
                    reason: format!("item {} points at unknown asset {}", item.id, item.asset_id),
                })?;
            if asset.kind != AssetKind::Video {
                return Err(RenderError::BadProject {
                    reason: format!("video item {} references a non-video asset", item.id),
                });
            }
            items.push((item.clone(), PathBuf::from(&asset.path_hint)));
        }
        Ok(Self {
            track_id,
            items,
            item_index: 0,
            decoder: None,
            last: None,
            pending: None,
            exhausted: false,
        })
    }

    /// Construct an independent reader for one enabled item. Transition
    /// rendering needs two simultaneous source timestamps at the same output
    /// instant, so it must not seek one stateful track reader backwards.
    fn for_item(project: &Project, track_id: TrackId, item_id: u64) -> Result<Self, RenderError> {
        let mut reader = Self::new(project, track_id)?;
        reader.items.retain(|(item, _)| item.id == item_id);
        if reader.items.len() != 1 {
            return Err(RenderError::BadProject {
                reason: format!("transition item {item_id} is not an enabled video item on track {track_id}"),
            });
        }
        Ok(reader)
    }

    fn bounds(&self) -> Option<(Tick, Tick)> {
        Some((self.items.first()?.0.start, self.items.last()?.0.end()))
    }

    fn reset_item(&mut self) {
        self.decoder = None;
        self.last = None;
        self.pending = None;
        self.exhausted = false;
    }

    /// Return the active clip frame for this track at an absolute timeline time.
    /// Empty intervals return None so lower video tracks remain visible.
    fn frame_at(
        &mut self,
        timeline_time: Tick,
        width: u32,
        height: u32,
    ) -> Result<Option<Frame>, RenderError> {
        while self.item_index < self.items.len()
            && timeline_time >= self.items[self.item_index].0.end()
        {
            self.item_index += 1;
            self.reset_item();
        }
        if self.item_index >= self.items.len() {
            return Ok(None);
        }

        let (item, path) = self.items[self.item_index].clone();
        if timeline_time < item.start {
            return Ok(None);
        }

        if self.decoder.is_none() {
            let mut decoder = VideoDecoder::open(&path)?;
            decoder.seek(item.source_in)?;
            self.decoder = Some(decoder);
        }

        let target = item.source_in + (timeline_time - item.start);
        loop {
            if self.pending.as_ref().map(|(time, _)| *time > target).unwrap_or(false) {
                break;
            }
            let next = if self.pending.is_some() {
                self.pending.take()
            } else if !self.exhausted {
                match self.decoder.as_mut().expect("active item has a decoder").next_rgb_frame(width)? {
                    Some(rgb) => {
                        let time = rgb.time;
                        let frame = Frame::from_rgb24(rgb.width, rgb.height, &rgb.pixels)?
                            .resized(width, height);
                        Some((time, frame))
                    }
                    None => {
                        self.exhausted = true;
                        None
                    }
                }
            } else {
                None
            };

            match next {
                Some((time, frame)) if time <= target => self.last = Some(frame),
                Some((time, frame)) => {
                    self.pending = Some((time, frame));
                    break;
                }
                None => break,
            }
        }

        if self.last.is_none() {
            self.last = self.pending.as_ref().map(|(_, frame)| frame.clone());
        }
        self.last.clone().map(Some).ok_or_else(|| RenderError::BadProject {
            reason: format!(
                "decoder produced no frames for {} on video track {}",
                path.display(), self.track_id
            ),
        })
    }
}

/// What a finished sequential export can report about itself.
#[derive(Clone, Debug)]
pub struct SequentialOutcome {
    pub output_path: PathBuf,
    pub frames_written: u64,
    /// Output duration after compressing each rendered transition region once.
    pub duration: Tick,
    /// The H.264 encoder the platform resolved to.
    pub encoder_name: String,
}

/// Export one video track as a hard-cut sequence. This lower-level entry
/// point is retained for callers and tests that intentionally select a track.
pub fn render_sequential(
    project: &Project,
    track_id: TrackId,
    output_path: &std::path::Path,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    render_sequential_with_audio(project, track_id, output_path, None, progress)
}

/// Export one video track with an optional stereo music bed.
pub fn render_sequential_with_audio(
    project: &Project,
    track_id: TrackId,
    output_path: &std::path::Path,
    audio: Option<StereoAudioInput<'_>>,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    render_video_tracks_with_audio(project, &[track_id], output_path, audio, None, progress)
}

/// Export every visible video track in project stacking order.
pub fn render_timeline(
    project: &Project,
    output_path: &std::path::Path,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    render_timeline_with_audio(project, output_path, None, progress)
}

/// Export every visible video track in project stacking order, optionally
/// adding the selected stereo song as a synchronized AAC music bed.
///
/// Track order is bottom-to-top: later entries in Project::tracks are
/// composited above earlier entries. A gap on an upper track reveals lower
/// tracks; black is emitted only when no visible video track has an active clip.
pub fn render_timeline_with_audio(
    project: &Project,
    output_path: &std::path::Path,
    audio: Option<StereoAudioInput<'_>>,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    render_timeline_with_audio_items(project, output_path, audio, &[], progress)
}

/// Export a timeline with an optional selected-song bed and decoded PCM for
/// placed Audio-track items. The song uses output time; placed items use the
/// transition-compressed source timeline.
pub fn render_timeline_with_audio_items(
    project: &Project,
    output_path: &std::path::Path,
    audio: Option<StereoAudioInput<'_>>,
    audio_assets: &[StereoAudioAssetInput<'_>],
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    let track_ids: Vec<TrackId> = project.tracks.iter()
        .filter(|track| track.kind == TrackKind::Video && !track.hidden)
        .map(|track| track.id)
        .collect();
    render_video_tracks_with_audio(project, &track_ids, output_path, audio, Some(audio_assets), progress)
}

fn render_video_tracks_with_audio(
    project: &Project,
    track_ids: &[TrackId],
    output_path: &std::path::Path,
    audio: Option<StereoAudioInput<'_>>,
    audio_assets: Option<&[StereoAudioAssetInput<'_>]>,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    const OUTPUT_AUDIO_RATE: u32 = 48_000;

    project.validate().map_err(|e| RenderError::BadProject {
        reason: e.to_string(),
    })?;
    if let Some(input) = audio {
        if input.sample_rate == 0 || input.interleaved_samples.len() % 2 != 0 || input.interleaved_samples.is_empty() {
            return Err(RenderError::BadProject {
                reason: "selected song must contain interleaved stereo samples at a valid sample rate".to_string(),
            });
        }
    }

    let mut audio_track_items = Vec::new();
    if let Some(audio_assets) = audio_assets {
        for track in project.tracks.iter()
            .filter(|track| track.kind == TrackKind::Audio && !track.hidden && !track.muted)
        {
            if let Some(items) = project.items(track.id) {
                for item in items.iter().filter(|item| item.enabled) {
                    let input = audio_assets.iter()
                        .find(|input| input.asset_id == item.asset_id)
                        .ok_or_else(|| RenderError::BadProject {
                            reason: format!("audio item {} has no decoded PCM asset {}", item.id, item.asset_id),
                        })?;
                    if input.sample_rate == 0 || input.interleaved_samples.is_empty() || input.interleaved_samples.len() % 2 != 0 {
                        return Err(RenderError::BadProject {
                            reason: format!("audio asset {} must contain interleaved stereo samples at a valid sample rate", item.asset_id),
                        });
                    }
                    audio_track_items.push(PreparedAudioTrackItem {
                        item: item.clone(),
                        interleaved_samples: input.interleaved_samples,
                        sample_rate: input.sample_rate,
                    });
                }
            }
        }
    }
    let has_audio_output = audio.is_some() || !audio_track_items.is_empty();

    let mut track_readers = Vec::new();
    let mut start_time: Option<Tick> = None;
    let mut end_time: Option<Tick> = None;
    for track_id in track_ids {
        let reader = TrackVideoReader::new(project, *track_id)?;
        if let Some((start, end)) = reader.bounds() {
            start_time = Some(start_time.map(|current| current.min(start)).unwrap_or(start));
            end_time = Some(end_time.map(|current| current.max(end)).unwrap_or(end));
            track_readers.push(reader);
        }
    }
    let start_time = start_time.ok_or_else(|| RenderError::BadProject {
        reason: "the timeline has no enabled clips on visible video tracks to export".to_string(),
    })?;
    let end_time = end_time.expect("a start time is set whenever a track is added");
    // Build one compressed output-time map for every selected video track.
    // A transition consumes D ticks from the combined timeline; the same map
    // drives all tracks so their relative timing cannot drift apart.
    let transition_map = TimelineTransitionMap::from_tracks(
        project,
        track_ids,
        start_time,
        end_time,
    ).map_err(|error| RenderError::BadProject {
        reason: error.to_string(),
    })?;
    let total_duration = transition_map.output_duration();

    let settings = &project.export_settings;
    let (width, height) = (settings.width, settings.height);
    let (fps_num, fps_den) = (settings.frame_rate_num, settings.frame_rate_den);
    let schedule = transition_map.frame_schedule(fps_num, fps_den)
        .ok_or_else(|| RenderError::BadProject {
            reason: "invalid output frame rate or compressed timeline range".to_string(),
        })?;
    let total_frames = schedule.frame_count() as usize;
    if total_frames == 0 {
        return Err(RenderError::BadProject {
            reason: "the compressed timeline rounds to zero output frames".to_string(),
        });
    }

    // A transition needs outgoing and incoming frames at once. Keep two
    // independent one-item readers per transition so decoder state remains
    // monotonic within each reader, including when a clip participates in
    // transitions at both its head and tail.
    let mut transition_readers: Vec<(
        TimelineTransitionSegment,
        TrackVideoReader,
        TrackVideoReader,
    )> = Vec::with_capacity(transition_map.segments().len());
    for segment in transition_map.segments() {
        transition_readers.push((
            *segment,
            TrackVideoReader::for_item(project, segment.track_id, segment.outgoing_item_id)?,
            TrackVideoReader::for_item(project, segment.track_id, segment.incoming_item_id)?,
        ));
    }

    // Audio duration follows the exact output frame count, not an independently
    // rounded Tick duration, so video and soundtrack share the same endpoint.
    let total_audio_samples = if has_audio_output {
        ((total_frames as i128 * OUTPUT_AUDIO_RATE as i128 * fps_den as i128)
            / fps_num as i128)
            .max(0) as usize
    } else {
        0
    };
    let mut encoder = if has_audio_output {
        ricodex_platform::VideoEncoder::create_with_audio(
            output_path,
            width,
            height,
            fps_num,
            fps_den,
            settings.video_bitrate_kbps,
            settings.audio_bitrate_kbps,
        )?
    } else {
        ricodex_platform::VideoEncoder::create(
            output_path,
            width,
            height,
            fps_num,
            fps_den,
            settings.video_bitrate_kbps,
        )?
    };
    let encoder_name = encoder.encoder_name().to_string();

    let mut written = 0usize;
    let mut audio_cursor = 0usize;
    let mut emit_frame = |frame: &Frame| -> Result<(), RenderError> {
        encoder.encode_rgb24(&frame.to_rgb24())?;
        written += 1;

        if has_audio_output {
            let next_sample = ((written as i128
                * OUTPUT_AUDIO_RATE as i128
                * fps_den as i128)
                / fps_num as i128)
                .max(0) as usize;
            let audio_end = next_sample.min(total_audio_samples);
            if audio_end > audio_cursor {
                let mut chunk = Vec::with_capacity((audio_end - audio_cursor) * 2);
                for output_sample in audio_cursor..audio_end {
                    let mut mixed = [0.0f32; 2];

                    // The selected song is output-time audio and remains at
                    // normal speed through transition-compressed video.
                    if let Some(input) = audio {
                        let numerator = output_sample as u128 * input.sample_rate as u128;
                        let source_index = (numerator / OUTPUT_AUDIO_RATE as u128) as usize;
                        let fraction = (numerator % OUTPUT_AUDIO_RATE as u128) as f32 / OUTPUT_AUDIO_RATE as f32;
                        let source_frames = input.interleaved_samples.len() / 2;
                        for channel in 0..2 {
                            let a = if source_index < source_frames { input.interleaved_samples[source_index * 2 + channel] } else { 0.0 };
                            let b = if source_index + 1 < source_frames { input.interleaved_samples[(source_index + 1) * 2 + channel] } else { 0.0 };
                            mixed[channel] += a + (b - a) * fraction;
                        }
                    }

                    // Placed clips sample the shared compressed project clock.
                    let sample_offset = Tick::from_ticks(
                        (output_sample as i128 * TICKS_PER_SECOND as i128 / OUTPUT_AUDIO_RATE as i128) as i64,
                    );
                    let output_time = start_time.checked_add(sample_offset)
                        .ok_or_else(|| RenderError::BadProject {
                            reason: "audio sample timestamp exceeded Tick range".to_string(),
                        })?;
                    if let Some(source_timeline_time) = transition_map.source_time_at(output_time) {
                        for source in &audio_track_items {
                            if source_timeline_time < source.item.start || source_timeline_time >= source.item.end() {
                                continue;
                            }
                            let source_offset = source_timeline_time.checked_sub(source.item.start)
                                .ok_or_else(|| RenderError::BadProject {
                                    reason: "audio source offset became negative".to_string(),
                                })?;
                            let source_time = source.item.source_in.checked_add(source_offset)
                                .ok_or_else(|| RenderError::BadProject {
                                    reason: "audio source timestamp exceeded Tick range".to_string(),
                                })?;
                            let sample = sample_stereo_at_tick(source.interleaved_samples, source.sample_rate, source_time);
                            mixed[0] += sample[0];
                            mixed[1] += sample[1];
                        }
                    }

                    // Apply a deterministic ceiling until gain/limiter controls exist.
                    chunk.push(mixed[0].clamp(-1.0, 1.0));
                    chunk.push(mixed[1].clamp(-1.0, 1.0));
                }
                encoder.encode_stereo_audio(&chunk)?;
                audio_cursor = audio_end;
            }
        }

        progress(written as f32 / total_frames as f32);
        Ok(())
    };

    // Track readers stay in project order, bottom to top. A transition is
    // rendered as one contribution in its owning track's slot: lower tracks
    // remain below it and upper tracks remain above it.
    for frame_index in 0..total_frames {
        let output_time = schedule.frame_time(frame_index as u64)
            .expect("frame index is bounded by the schedule")
            .time;
        let source_time = transition_map.source_time_at(output_time)
            .ok_or_else(|| RenderError::BadProject {
                reason: "output frame is outside the compressed source-time map".to_string(),
            })?;
        let active_transition = transition_map.transition_frame_at(output_time);
        let mut layers = Vec::with_capacity(track_readers.len());
        for reader in &mut track_readers {
            let frame = match active_transition.filter(|transition| transition.track_id == reader.track_id) {
                Some(transition) => {
                    let (_, outgoing_reader, incoming_reader) = transition_readers
                        .iter_mut()
                        .find(|(segment, _, _)| {
                            segment.track_id == transition.track_id
                                && segment.incoming_item_id == transition.incoming_item_id
                        })
                        .ok_or_else(|| RenderError::BadProject {
                            reason: format!(
                                "transition readers for incoming item {} are unavailable",
                                transition.incoming_item_id
                            ),
                        })?;

                    let outgoing = outgoing_reader
                        .frame_at(transition.outgoing_timeline_time, width, height)?
                        .ok_or_else(|| RenderError::BadProject {
                            reason: format!(
                                "outgoing clip {} produced no frame during its transition",
                                transition.outgoing_item_id
                            ),
                        })?;
                    let incoming = incoming_reader
                        .frame_at(transition.incoming_timeline_time, width, height)?
                        .ok_or_else(|| RenderError::BadProject {
                            reason: format!(
                                "incoming clip {} produced no frame during its transition",
                                transition.incoming_item_id
                            ),
                        })?;
                    Some(composite_cpu(
                        &outgoing,
                        &incoming,
                        transition.kind,
                        transition.progress,
                        &transition.params,
                    )?)
                }
                None => reader.frame_at(source_time, width, height)?,
            };
            if let Some(frame) = frame {
                layers.push(frame);
            }
        }
        let composite = composite_video_layers(width, height, &layers)?;
        emit_frame(&composite)?;
    }

    drop(emit_frame);
    encoder.finish()?;

    Ok(SequentialOutcome {
        output_path: output_path.to_path_buf(),
        frames_written: written as u64,
        duration: total_duration,
        encoder_name,
    })
}
