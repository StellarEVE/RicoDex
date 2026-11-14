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

use ricodex_core::{Project, Tick, TrackId, TrackKind, TICKS_PER_SECOND};
use ricodex_platform::VideoDecoder;

use crate::cache::{RenderCache, TransitionUnitKey};
use crate::composite::composite_cpu;
use crate::effects::{apply_effects, describe_stack, EffectOp};
use crate::error::RenderError;
use crate::frame::Frame;
use crate::gpu::GpuCompositor;
use crate::transition::{TransitionKind, TransitionParams};
use crate::timeline::TimelineFrameSchedule;

/// Stereo source audio supplied to the sequential export. Samples are
/// interleaved left/right f32 values at the source sample rate.
#[derive(Clone, Copy, Debug)]
pub struct StereoAudioInput<'a> {
    pub interleaved_samples: &'a [f32],
    pub sample_rate: u32,
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

/// What a finished sequential export can report about itself.
#[derive(Clone, Debug)]
pub struct SequentialOutcome {
    pub output_path: PathBuf,
    pub frames_written: u64,
    /// Total duration covered by the enabled items, first start to
    /// last end.
    pub duration: Tick,
    /// The H.264 encoder the platform resolved to.
    pub encoder_name: String,
}

/// The plain sequential export: every enabled item on one video
/// track, in timeline order, decoded and encoded back to back with
/// hard cuts and no transitions or effects. The optional soundtrack
/// is supplied by the project-selected song and is mixed as stereo AAC.
///
/// The enabled items must form one contiguous run: each item must
/// start exactly where the previous one ends. A gap, including the
/// gap a disabled item leaves, is rejected rather than silently
/// filled, so the output duration is always the run's duration.
/// progress fires after every encoded video frame with the fraction
/// complete, ending at exactly 1.0.
pub fn render_sequential(
    project: &Project,
    track_id: TrackId,
    output_path: &std::path::Path,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    render_sequential_with_audio(project, track_id, output_path, None, progress)
}

/// Sequential hard-cut export with an optional stereo music bed.
/// The supplied audio is resampled to 48 kHz, starts at output time
/// zero, and is truncated or silence-padded to the video run duration.
pub fn render_sequential_with_audio(
    project: &Project,
    track_id: TrackId,
    output_path: &std::path::Path,
    audio: Option<StereoAudioInput<'_>>,
    progress: &mut dyn FnMut(f32),
) -> Result<SequentialOutcome, RenderError> {
    const OUTPUT_AUDIO_RATE: u32 = 48_000;

    project.validate().map_err(|e| RenderError::BadProject {
        reason: e.to_string(),
    })?;
    if let Some(input) = audio {
        if input.sample_rate == 0 || input.interleaved_samples.len() % 2 != 0 {
            return Err(RenderError::BadProject {
                reason: "selected song must contain interleaved stereo samples at a valid sample rate".to_string(),
            });
        }
        if input.interleaved_samples.is_empty() {
            return Err(RenderError::BadProject {
                reason: "selected song decoded to no audio samples".to_string(),
            });
        }
    }

    let track = project.track(track_id).ok_or_else(|| RenderError::BadProject {
        reason: format!("no track {track_id}"),
    })?;
    if track.kind != TrackKind::Video {
        return Err(RenderError::BadProject {
            reason: "the sequential export renders a video track".to_string(),
        });
    }
    let all_items = project.items(track_id).ok_or_else(|| RenderError::BadProject {
        reason: "track has no item group".to_string(),
    })?;
    let items: Vec<&ricodex_core::Item> = all_items.iter().filter(|i| i.enabled).collect();
    if items.is_empty() {
        return Err(RenderError::BadProject {
            reason: "the timeline has no enabled clips to export".to_string(),
        });
    }
    for pair in items.windows(2) {
        if pair[1].start != pair[0].end() {
            return Err(RenderError::BadProject {
                reason: format!(
                    "clips {} and {} are not back to back; this export renders one contiguous run",
                    pair[0].id, pair[1].id
                ),
            });
        }
    }

    let settings = &project.export_settings;
    let (width, height) = (settings.width, settings.height);
    let (fps_num, fps_den) = (settings.frame_rate_num, settings.frame_rate_den);
    let total_duration = items[items.len() - 1].end() - items[0].start;
    let schedule = TimelineFrameSchedule::for_range(
        items[0].start,
        items[items.len() - 1].end(),
        fps_num,
        fps_den,
    ).ok_or_else(|| RenderError::BadProject {
        reason: "invalid output frame rate or timeline range".to_string(),
    })?;
    let total_frames = schedule.frame_count() as usize;
    if total_frames == 0 {
        return Err(RenderError::BadProject {
            reason: "the timeline rounds to zero output frames".to_string(),
        });
    }

    // Audio must match the output frame schedule, not the unrounded
    // tick duration: the encoded video duration is total_frames / fps.
    let total_audio_samples = if audio.is_some() {
        ((total_frames as i128 * OUTPUT_AUDIO_RATE as i128 * fps_den as i128)
            / fps_num as i128)
            .max(0) as usize
    } else {
        0
    };
    let mut encoder = if audio.is_some() {
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
    let black_frame = Frame::solid(width, height, [0, 0, 0, 255]);
    let mut emit_frame = |frame: &Frame| -> Result<(), RenderError> {
        encoder.encode_rgb24(&frame.to_rgb24())?;
        written += 1;

        if let Some(input) = audio {
            // Derive audio boundaries from the shared output frame count.
            let next_sample = ((written as i128
                * OUTPUT_AUDIO_RATE as i128
                * fps_den as i128)
                / fps_num as i128)
                .max(0) as usize;
            let audio_end = next_sample.min(total_audio_samples);
            if audio_end > audio_cursor {
                let source_frames = input.interleaved_samples.len() / 2;
                let mut chunk = Vec::with_capacity((audio_end - audio_cursor) * 2);
                for output_sample in audio_cursor..audio_end {
                    let source_numerator =
                        output_sample as u128 * input.sample_rate as u128;
                    let source_index =
                        (source_numerator / OUTPUT_AUDIO_RATE as u128) as usize;
                    let remainder =
                        (source_numerator % OUTPUT_AUDIO_RATE as u128) as f32;
                    let fraction = remainder / OUTPUT_AUDIO_RATE as f32;
                    for channel in 0..2 {
                        let a = if source_index < source_frames {
                            input.interleaved_samples[source_index * 2 + channel]
                        } else {
                            0.0
                        };
                        let b = if source_index + 1 < source_frames {
                            input.interleaved_samples[(source_index + 1) * 2 + channel]
                        } else {
                            0.0
                        };
                        chunk.push(a + (b - a) * fraction);
                    }
                }
                encoder.encode_stereo_audio(&chunk)?;
                audio_cursor = audio_end;
            }
        }

        progress(written as f32 / total_frames as f32);
        Ok(())
    };

    // The shared schedule is consumed exactly once from the first enabled
    // clip to the last. Any uncovered interval, including the interval left
    // by a disabled clip, is encoded as black rather than collapsed.
    let mut next_output_index = 0usize;
    for item in &items {
        let asset = project
            .assets
            .iter()
            .find(|a| a.id == item.asset_id)
            .ok_or_else(|| RenderError::BadProject {
                reason: format!("item {} points at unknown asset {}", item.id, item.asset_id),
            })?;
        let path = PathBuf::from(&asset.path_hint);
        let mut decoder = VideoDecoder::open(&path)?;
        decoder.seek(item.source_in)?;

        let mut last: Option<Frame> = None;
        let mut pending: Option<(Tick, Frame)> = None;
        let mut exhausted = false;
        while next_output_index < total_frames {
            let timeline_time = schedule.frame_time(next_output_index as u64)
                .expect("frame index is bounded by the schedule")
                .time;

            if timeline_time < item.start {
                emit_frame(&black_frame)?;
                next_output_index += 1;
                continue;
            }
            if timeline_time >= item.end() {
                break;
            }

            let target = item.source_in + (timeline_time - item.start);
            loop {
                if let Some((time, _)) = &pending {
                    if *time > target {
                        break;
                    }
                }
                let next = if pending.is_some() {
                    pending.take()
                } else if !exhausted {
                    match decoder.next_rgb_frame(width)? {
                        Some(rgb) => {
                            let time = rgb.time;
                            let frame = Frame::from_rgb24(rgb.width, rgb.height, &rgb.pixels)?
                                .resized(width, height);
                            Some((time, frame))
                        }
                        None => {
                            exhausted = true;
                            None
                        }
                    }
                } else {
                    None
                };
                match next {
                    Some((time, frame)) => {
                        if time <= target {
                            last = Some(frame);
                        } else {
                            pending = Some((time, frame));
                            break;
                        }
                    }
                    None => break,
                }
            }
            if last.is_none() {
                if let Some((_, frame)) = &pending {
                    last = Some(frame.clone());
                }
            }
            let frame = last.clone().ok_or_else(|| RenderError::BadProject {
                reason: format!("decoder produced no frames for {}", path.display()),
            })?;
            emit_frame(&frame)?;
            next_output_index += 1;
        }
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
