//! Per frame scoring on the CPU: mean luma, frame to frame difference
//! and black detection over the small grayscale frames the platform
//! decoder produces.
//!
//! This module is deliberately the seam for the future GPU analysis
//! pass: its functions take plain gray frames and return plain
//! scores, so a WGSL shader implementation can replace the loops here
//! without anything outside this file changing shape.

use ricodex_platform::GrayFrame;

/// A frame whose mean luma sits below this (0.0 to 1.0) counts as
/// black for black range detection.
pub const BLACK_MEAN_LUMA: f32 = 0.06;

/// A frame to frame difference above this (0.0 to 1.0, mean absolute
/// luma change) counts as a scene cut.
pub const SCENE_CUT_DIFF: f32 = 0.30;

/// The score of one analysed frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameScore {
    /// Mean luma, 0.0 to 1.0.
    pub mean_luma: f32,
    /// Mean absolute luma difference against the previous frame, 0.0
    /// to 1.0, or None for the first frame scored. A change of frame
    /// size also reports a full difference of 1.0.
    pub diff_from_prev: Option<f32>,
    pub is_black: bool,
}

/// Mean luma of packed grayscale pixels, 0.0 to 1.0.
pub fn mean_luma(pixels: &[u8]) -> f32 {
    if pixels.is_empty() {
        return 0.0;
    }
    let sum: u64 = pixels.iter().map(|&p| p as u64).sum();
    sum as f32 / (pixels.len() as f32 * 255.0)
}

/// Mean absolute difference between two packed grayscale planes, 0.0
/// to 1.0. Different lengths report a full difference.
pub fn mean_abs_diff(a: &[u8], b: &[u8]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| x.abs_diff(y) as u64)
        .sum();
    sum as f32 / (a.len() as f32 * 255.0)
}

/// Score one frame, against the previously scored frame when there is
/// one.
pub fn score_frame(frame: &GrayFrame, previous: Option<&GrayFrame>) -> FrameScore {
    let luma = mean_luma(&frame.pixels);
    let diff = previous.map(|prev| {
        if prev.width != frame.width || prev.height != frame.height {
            1.0
        } else {
            mean_abs_diff(&prev.pixels, &frame.pixels)
        }
    });
    FrameScore {
        mean_luma: luma,
        diff_from_prev: diff,
        is_black: luma < BLACK_MEAN_LUMA,
    }
}
