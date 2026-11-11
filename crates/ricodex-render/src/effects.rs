//! Effects on the CPU path: a small first set of single frame
//! operations, expressed as data so a project's effect stack is a
//! plain list that can be stored, described for cache keys, and
//! applied in stack order.
//!
//! Approximations, documented where they live: Blur is a separable
//! box blur, not a gaussian; Sharpen is an unsharp mask built on
//! that box blur. Colour effects leave the alpha channel untouched.

use serde::{Deserialize, Serialize};

use crate::frame::{lerp, round_u8, Frame};

/// One effect operation with its parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EffectOp {
    /// Add `amount` (in -1.0 to 1.0) of the full scale to every
    /// colour channel.
    Brightness { amount: f32 },
    /// Scale colour channels around middle grey by `factor`
    /// (1.0 is identity).
    Contrast { factor: f32 },
    /// Scale colour saturation by `factor` (0.0 is grey, 1.0 is
    /// identity).
    Saturation { factor: f32 },
    /// Box blur with the given radius in pixels (0 is identity).
    Blur { radius: u32 },
    /// Unsharp mask with the given strength (0.0 is identity).
    Sharpen { amount: f32 },
}

impl EffectOp {
    /// A canonical description for cache keys: equal stacks describe
    /// equal, byte for byte.
    pub fn describe(&self) -> String {
        match self {
            EffectOp::Brightness { amount } => format!("brightness({amount:.4})"),
            EffectOp::Contrast { factor } => format!("contrast({factor:.4})"),
            EffectOp::Saturation { factor } => format!("saturation({factor:.4})"),
            EffectOp::Blur { radius } => format!("blur({radius})"),
            EffectOp::Sharpen { amount } => format!("sharpen({amount:.4})"),
        }
    }
}

/// The canonical description of a whole stack, in application
/// order, for cache keys.
pub fn describe_stack(effects: &[EffectOp]) -> String {
    if effects.is_empty() {
        return "none".to_string();
    }
    effects
        .iter()
        .map(|e| e.describe())
        .collect::<Vec<_>>()
        .join(";")
}

/// Apply a stack of effects to a frame, in order, returning the
/// resulting frame. An empty stack returns the frame unchanged.
pub fn apply_effects(frame: &Frame, effects: &[EffectOp]) -> Frame {
    let mut current = frame.clone();
    for effect in effects {
        current = apply_one(&current, effect);
    }
    current
}

fn apply_one(frame: &Frame, effect: &EffectOp) -> Frame {
    match *effect {
        EffectOp::Brightness { amount } => per_channel(frame, |v, _| v + amount * 255.0),
        EffectOp::Contrast { factor } => {
            per_channel(frame, |v, _| (v - 127.5) * factor + 127.5)
        }
        EffectOp::Saturation { factor } => saturation(frame, factor),
        EffectOp::Blur { radius } => box_blur(frame, radius),
        EffectOp::Sharpen { amount } => sharpen(frame, amount),
    }
}

/// Apply `f` to each colour channel, with the pixel's other
/// channels available for context; alpha passes through.
fn per_channel(frame: &Frame, f: impl Fn(f32, &[u8]) -> f32) -> Frame {
    let mut pixels = frame.pixels.clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let original = [pixel[0], pixel[1], pixel[2], pixel[3]];
        for channel in 0..3 {
            pixel[channel] = round_u8(f(original[channel] as f32, &original));
        }
    }
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

fn saturation(frame: &Frame, factor: f32) -> Frame {
    let mut pixels = frame.pixels.clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let (r, g, b) = (
            pixel[0] as f32,
            pixel[1] as f32,
            pixel[2] as f32,
        );
        // Rec. 709 luma, the workspace's display standard.
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        pixel[0] = round_u8(lerp(luma, r, factor));
        pixel[1] = round_u8(lerp(luma, g, factor));
        pixel[2] = round_u8(lerp(luma, b, factor));
    }
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

/// Separable box blur: one horizontal pass, one vertical, each
/// averaging a window of `2 * radius + 1` texels with edge clamping.
/// Documented approximation of a gaussian blur: two box passes read
/// close enough for a first set, and the render cache makes trying
/// it cheap.
pub fn box_blur(frame: &Frame, radius: u32) -> Frame {
    if radius == 0 || frame.width == 0 || frame.height == 0 {
        return frame.clone();
    }
    let horizontal = blur_pass(frame, radius, true);
    blur_pass(&horizontal, radius, false)
}

fn blur_pass(frame: &Frame, radius: u32, horizontal: bool) -> Frame {
    let (w, h) = (frame.width as i64, frame.height as i64);
    let r = radius as i64;
    let count = (2 * r + 1) as f32;
    let mut pixels = vec![0u8; frame.pixels.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0f32; 4];
            for k in -r..=r {
                let (sx, sy) = if horizontal {
                    ((x + k).clamp(0, w - 1), y)
                } else {
                    (x, (y + k).clamp(0, h - 1))
                };
                let offset = (sy as usize * frame.width as usize + sx as usize) * 4;
                for channel in 0..4 {
                    sum[channel] += frame.pixels[offset + channel] as f32;
                }
            }
            let offset = (y as usize * frame.width as usize + x as usize) * 4;
            for channel in 0..4 {
                pixels[offset + channel] = round_u8(sum[channel] / count);
            }
        }
    }
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

/// Unsharp mask: the frame plus `amount` times its difference from
/// a small box blur of itself. Documented approximation of a
/// convolution sharpen: the blur radius is fixed at 1, so the mask
/// catches single pixel edges.
fn sharpen(frame: &Frame, amount: f32) -> Frame {
    if amount == 0.0 {
        return frame.clone();
    }
    let blurred = box_blur(frame, 1);
    let mut pixels = frame.pixels.clone();
    for (pixel, soft) in pixels
        .chunks_exact_mut(4)
        .zip(blurred.pixels.chunks_exact(4))
    {
        for channel in 0..3 {
            let value = pixel[channel] as f32;
            pixel[channel] = round_u8(value + amount * (value - soft[channel] as f32));
        }
    }
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}
