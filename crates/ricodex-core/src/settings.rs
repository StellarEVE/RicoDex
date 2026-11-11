//! Plain data structs for settings and presets. These carry values only;
//! the behaviour that consumes them lives in the engine and render crates.

use serde::{Deserialize, Serialize};

use crate::time::Tick;

/// How a finished video is exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    /// Frame rate as a rational, numerator over denominator.
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    /// Target container, as a plain name the render crate maps to a muxer.
    pub container: String,
    /// Target video codec name, before hardware encoder selection.
    pub video_codec: String,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            width: 1920,
            height: 1080,
            frame_rate_num: 30,
            frame_rate_den: 1,
            container: "mp4".to_string(),
            video_codec: "h264".to_string(),
            video_bitrate_kbps: 16_000,
            audio_bitrate_kbps: 320,
        }
    }
}

/// How the planner generates a first cut. The numeric defaults preserve the
/// behaviour proven in the previous project, including the continue starts
/// bonus of 0.35, which was settled by testing after 0.45 broke round robin
/// routing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerationSettings {
    /// Shortest clip the planner may place.
    pub min_clip: Tick,
    /// Longest clip the planner may place.
    pub max_clip: Tick,
    /// How far a clip start may wander from its ideal beat position, as a
    /// fraction of the beat spacing.
    pub clip_variance: f64,
    /// Bonus for starting a clip where the same source last stopped.
    pub continue_starts_bonus: f64,
    /// Keep generated clips from reusing the same source span.
    pub reuse_protection: bool,
    /// Cut placement offset relative to the beat, in ticks; negative cuts
    /// land slightly ahead of the beat.
    pub cut_offset: Tick,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        GenerationSettings {
            min_clip: Tick::from_seconds(0.8),
            max_clip: Tick::from_seconds(4.0),
            clip_variance: 0.5,
            continue_starts_bonus: 0.35,
            reuse_protection: true,
            cut_offset: Tick::ZERO,
        }
    }
}
