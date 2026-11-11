//! The staged analysis passes.
//!
//! Stage 0 is [`probe`]: container metadata only, no decoding.
//! Stage 1 is [`analyse_stage1`]: a seek based sparse sampling pass
//! (2 frames per second at most 160 px wide) that produces a poster
//! frame, a filmstrip, coarse scene boundaries, black ranges and a
//! rough motion estimate per scene. Everything it returns is coarse,
//! so the planner treats clips drawn from it as provisional.
//! Stage 2 is [`analyse_stage2`]: one full decode at most 320 px
//! wide, scoring every frame, emitting progress through a callback as
//! frames complete, and producing exact scene boundaries, black
//! ranges, a motion score per scene and best moment flags. Its
//! output is settled.
//!
//! The data types mirror pmv-engine's `SceneInfo`, `BlackRange` and
//! `SourceInfo` field for field, with the same partial test, so the
//! engine converts with a plain field copy.

use std::path::{Path, PathBuf};

use pmv_core::Tick;
use pmv_platform::{GrayFrame, RgbFrame, VideoDecoder};
use serde::{Deserialize, Serialize};

use crate::error::MediaError;
use crate::scoring::{score_frame, FrameScore, SCENE_CUT_DIFF};

/// Stage 1 sampling rate: frames per second.
pub const STAGE1_SAMPLE_FPS: f64 = 2.0;
/// Stage 1 analysis width cap, in pixels.
pub const STAGE1_MAX_WIDTH: u32 = 160;
/// Stage 2 analysis width cap, in pixels.
pub const STAGE2_MAX_WIDTH: u32 = 320;
/// Filmstrip tile width, in pixels.
pub const FILMSTRIP_TILE_WIDTH: u32 = 80;
/// Most tiles a filmstrip holds; extra samples are still scored, they
/// just do not all become tiles.
pub const FILMSTRIP_MAX_TILES: usize = 240;

// ------------------------------------------------------------------ data

/// One analysed scene. Field for field the shape pmv-engine consumes
/// as `SceneInfo`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaScene {
    pub start: Tick,
    pub duration: Tick,
    /// Motion score, 0.0 to 1.0, when the stage that ran measures
    /// motion. Stage 1 fills this with a rough estimate, Stage 2 with
    /// the measured score.
    pub motion: Option<f32>,
    pub best_moment: bool,
    /// True when the boundaries are Stage 1 approximations.
    pub coarse: bool,
}

impl MediaScene {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// A range of a source that is black. Field for field the shape
/// pmv-engine consumes as `BlackRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaBlackRange {
    pub start: Tick,
    pub duration: Tick,
}

impl MediaBlackRange {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// The analysis of one source from one stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceAnalysis {
    pub duration: Tick,
    pub scenes: Vec<MediaScene>,
    pub black_ranges: Vec<MediaBlackRange>,
    /// Which stage produced this: 1 for the sparse sampling pass, 2
    /// for the full decode.
    pub stage: u8,
}

impl SourceAnalysis {
    /// The same partial test pmv-engine's `SourceInfo::is_partial`
    /// runs: no scenes yet, any scene still coarse, or motion not yet
    /// measured. Partial analysis makes provisional clips.
    pub fn is_partial(&self) -> bool {
        self.scenes.is_empty()
            || self
                .scenes
                .iter()
                .any(|s| s.coarse || s.motion.is_none())
    }
}

// ----------------------------------------------------------------- probe

/// Stage 0 output: container metadata as plain data, with the
/// duration in ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaProbe {
    pub duration: Tick,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub codec_name: String,
    pub rotation: Option<i32>,
}

/// Stage 0: probe a file's container metadata without decoding.
pub fn probe(path: &Path) -> Result<MediaProbe, MediaError> {
    let info = pmv_platform::probe(path)?;
    Ok(MediaProbe {
        duration: info.duration,
        width: info.width,
        height: info.height,
        frame_rate: info.frame_rate,
        codec_name: info.codec_name,
        rotation: info.rotation,
    })
}

// ------------------------------------------------------------- internals

/// One scored frame of an analysis pass: when it sits, and its score.
type ScoredFrame = (Tick, FrameScore);

/// A scene span over the scored frames: the half open sample index
/// range it covers and its time range.
struct SceneSpan {
    start: Tick,
    end: Tick,
    first: usize,
    last: usize,
}

fn scene_spans(scored: &[ScoredFrame], duration: Tick) -> Vec<SceneSpan> {
    if scored.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut first = 0usize;
    for i in 1..scored.len() {
        if scored[i].1.diff_from_prev.unwrap_or(0.0) > SCENE_CUT_DIFF {
            spans.push(SceneSpan {
                start: scored[first].0,
                end: scored[i].0,
                first,
                last: i,
            });
            first = i;
        }
    }
    spans.push(SceneSpan {
        start: scored[first].0,
        end: duration,
        first,
        last: scored.len(),
    });
    spans
}

/// Mean in-scene frame difference: the average of the diffs of the
/// frames after the scene's first, so the cut that opens the scene
/// never inflates its motion.
fn scene_motion(scored: &[ScoredFrame], span: &SceneSpan) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0usize;
    for i in (span.first + 1)..span.last {
        sum += scored[i].1.diff_from_prev.unwrap_or(0.0);
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

/// The best moment heuristic: a scene holds a best moment when it
/// contains a frame whose motion (its diff against the previous
/// frame) is at least 0.01 and is followed by a frame of equal or
/// lower motion, a motion peak that settles rather than a cut or a
/// continuing ramp. The highest such peak is the moment; the flag is
/// what the scene reports.
fn scene_best_moment(scored: &[ScoredFrame], span: &SceneSpan) -> bool {
    let mut best: Option<f32> = None;
    for i in (span.first + 1)..span.last {
        let diff = scored[i].1.diff_from_prev.unwrap_or(0.0);
        if diff < 0.01 {
            continue;
        }
        let next = if i + 1 < span.last {
            scored[i + 1].1.diff_from_prev.unwrap_or(0.0)
        } else {
            0.0
        };
        if next <= diff && best.map(|b| diff > b).unwrap_or(true) {
            best = Some(diff);
        }
    }
    best.is_some()
}

fn black_ranges(scored: &[ScoredFrame], duration: Tick) -> Vec<MediaBlackRange> {
    let mut ranges = Vec::new();
    let mut run_start: Option<Tick> = None;
    for (time, score) in scored {
        if score.is_black {
            if run_start.is_none() {
                run_start = Some(*time);
            }
        } else if let Some(start) = run_start.take() {
            if *time > start {
                ranges.push(MediaBlackRange {
                    start,
                    duration: *time - start,
                });
            }
        }
    }
    if let Some(start) = run_start {
        if duration > start {
            ranges.push(MediaBlackRange {
                start,
                duration: duration - start,
            });
        }
    }
    ranges
}

fn build_analysis(
    scored: &[ScoredFrame],
    duration: Tick,
    stage: u8,
    coarse: bool,
    with_best_moments: bool,
) -> SourceAnalysis {
    let spans = scene_spans(scored, duration);
    let scenes = spans
        .iter()
        .map(|span| MediaScene {
            start: span.start,
            duration: span.end - span.start,
            motion: Some(scene_motion(scored, span)),
            best_moment: with_best_moments && scene_best_moment(scored, span),
            coarse,
        })
        .collect();
    SourceAnalysis {
        duration,
        scenes,
        black_ranges: black_ranges(scored, duration),
        stage,
    }
}

// --------------------------------------------------------------- stage 1

/// The output of Stage 1: the coarse analysis plus the poster and
/// filmstrip written into the image directory.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage1Output {
    pub analysis: SourceAnalysis,
    pub poster_path: PathBuf,
    pub filmstrip_path: PathBuf,
}

/// Resize packed RGB to a target width with nearest neighbour
/// sampling, keeping aspect. Used for filmstrip tiles.
fn resize_rgb_nearest(frame: &RgbFrame, target_width: u32) -> (u32, u32, Vec<u8>) {
    if frame.width == 0 || frame.height == 0 || frame.width == target_width {
        return (frame.width, frame.height, frame.pixels.clone());
    }
    let target_height =
        ((frame.height as u64 * target_width as u64) / frame.width as u64).max(1) as u32;
    let mut out = vec![0u8; (target_width * target_height * 3) as usize];
    for y in 0..target_height {
        let src_y = (y as u64 * frame.height as u64 / target_height as u64) as u32;
        for x in 0..target_width {
            let src_x = (x as u64 * frame.width as u64 / target_width as u64) as u32;
            let src = ((src_y * frame.width + src_x) * 3) as usize;
            let dst = ((y * target_width + x) * 3) as usize;
            out[dst..dst + 3].copy_from_slice(&frame.pixels[src..src + 3]);
        }
    }
    (target_width, target_height, out)
}

fn write_png_rgb(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<(), MediaError> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| MediaError::Image { reason: e.to_string() })?;
    writer
        .write_image_data(rgb)
        .map_err(|e| MediaError::Image { reason: e.to_string() })?;
    writer
        .finish()
        .map_err(|e| MediaError::Image { reason: e.to_string() })?;
    Ok(())
}

/// Assemble the filmstrip image from RGB samples: tiles side by side
/// on a transparent black strip, all resized to the tile width.
fn write_filmstrip(path: &Path, samples: &[&RgbFrame]) -> Result<(), MediaError> {
    if samples.is_empty() {
        return Err(MediaError::Image {
            reason: "no frames sampled for the filmstrip".to_string(),
        });
    }
    let tiles: Vec<(u32, u32, Vec<u8>)> = samples
        .iter()
        .map(|frame| resize_rgb_nearest(frame, FILMSTRIP_TILE_WIDTH))
        .collect();
    let tile_w = FILMSTRIP_TILE_WIDTH;
    let tile_h = tiles.iter().map(|t| t.1).max().unwrap_or(0).max(1);
    let width = tile_w * tiles.len() as u32;
    let mut canvas = vec![0u8; (width * tile_h * 3) as usize];
    for (index, (_, h, pixels)) in tiles.iter().enumerate() {
        for y in 0..*h {
            let src = (y * tile_w * 3) as usize;
            let dst = ((y * width + index as u32 * tile_w) * 3) as usize;
            canvas[dst..dst + (tile_w * 3) as usize]
                .copy_from_slice(&pixels[src..src + (tile_w * 3) as usize]);
        }
    }
    write_png_rgb(path, width, tile_h, &canvas)
}

/// Stage 1: the sparse sampling pass. Seeks through the file at
/// [`STAGE1_SAMPLE_FPS`], scoring each sampled frame, and writes the
/// poster (`poster.png`, the first sampled frame) and the filmstrip
/// (`filmstrip.png`) into `image_dir`, which is created when needed.
/// The returned analysis is explicitly coarse: scene boundaries are
/// sample accurate, not frame accurate.
pub fn analyse_stage1(path: &Path, image_dir: &Path) -> Result<Stage1Output, MediaError> {
    let mut decoder = VideoDecoder::open(path)?;
    let duration = decoder.duration();
    if duration <= Tick::ZERO {
        return Err(MediaError::Analysis {
            reason: "probe reported a zero duration, cannot sample".to_string(),
        });
    }
    std::fs::create_dir_all(image_dir)?;

    let step = Tick::from_seconds(1.0 / STAGE1_SAMPLE_FPS);
    let mut scored: Vec<ScoredFrame> = Vec::new();
    let mut rgb_samples: Vec<RgbFrame> = Vec::new();
    let mut previous: Option<GrayFrame> = None;
    let mut target = Tick::ZERO;
    while target < duration {
        decoder.seek(target)?;
        // A seek lands on the keyframe at or before the target, so
        // decode forward and discard until the first frame at or
        // after the target: that frame is this target's sample.
        let mut sampled = None;
        while let Some(pair) = decoder.next_frame_pair(STAGE1_MAX_WIDTH)? {
            if pair.0.time >= target {
                sampled = Some(pair);
                break;
            }
        }
        let Some((gray, rgb)) = sampled else {
            break;
        };
        if scored.last().map(|(t, _)| *t == gray.time).unwrap_or(false) {
            // The seek landed on the same frame as the previous
            // sample (very short file): count it once.
            target = target + step;
            continue;
        }
        let score = score_frame(&gray, previous.as_ref());
        scored.push((gray.time, score));
        previous = Some(gray);
        rgb_samples.push(rgb);
        target = target + step;
    }
    if scored.is_empty() {
        return Err(MediaError::Analysis {
            reason: "stage 1 sampled no frames".to_string(),
        });
    }

    let poster_path = image_dir.join("poster.png");
    let first = &rgb_samples[0];
    write_png_rgb(&poster_path, first.width, first.height, &first.pixels)?;

    // Thin the tiles, not the samples, when a long file overshoots
    // the filmstrip cap.
    let stride = if rgb_samples.len() > FILMSTRIP_MAX_TILES {
        rgb_samples.len() / FILMSTRIP_MAX_TILES + 1
    } else {
        1
    };
    let tiles: Vec<&RgbFrame> = rgb_samples.iter().step_by(stride).collect();
    let filmstrip_path = image_dir.join("filmstrip.png");
    write_filmstrip(&filmstrip_path, &tiles)?;

    let analysis = build_analysis(&scored, duration, 1, true, false);
    Ok(Stage1Output {
        analysis,
        poster_path,
        filmstrip_path,
    })
}

// --------------------------------------------------------------- stage 2

/// Stage 2: the full pass. One sequential decode scoring every
/// frame. `progress` is called with the fraction complete (0.0 to
/// 1.0) whenever it advances by at least a hundredth, and once more
/// with exactly 1.0 at the end. The returned analysis is settled:
/// exact boundaries, measured motion, best moment flags.
pub fn analyse_stage2(
    path: &Path,
    progress: &mut dyn FnMut(f32),
) -> Result<SourceAnalysis, MediaError> {
    let mut decoder = VideoDecoder::open(path)?;
    let duration = decoder.duration();
    let mut scored: Vec<ScoredFrame> = Vec::new();
    let mut previous: Option<GrayFrame> = None;
    let mut last_reported = -1.0f32;
    while let Some(frame) = decoder.next_gray_frame(STAGE2_MAX_WIDTH)? {
        let time = frame.time;
        let score = score_frame(&frame, previous.as_ref());
        scored.push((time, score));
        previous = Some(frame);
        if duration > Tick::ZERO {
            let fraction =
                (time.as_ticks() as f32 / duration.as_ticks() as f32).clamp(0.0, 1.0);
            if fraction - last_reported >= 0.01 {
                progress(fraction);
                last_reported = fraction;
            }
        }
    }
    progress(1.0);
    Ok(build_analysis(&scored, duration, 2, false, true))
}
