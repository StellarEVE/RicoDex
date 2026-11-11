//! The frame type the compositor works on: one owned RGBA image,
//! tightly packed rows of four bytes per pixel, alpha included so the
//! format is exactly what wgpu uploads and what the cache stores.

use crate::error::RenderError;

/// One RGBA frame, owned. `pixels.len() == width * height * 4`.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Frame, RenderError> {
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(RenderError::BadFrame {
                reason: format!(
                    "pixel buffer has {} bytes, expected {expected} for {width}x{height}",
                    pixels.len()
                ),
            });
        }
        Ok(Frame {
            width,
            height,
            pixels,
        })
    }

    /// A frame of one solid colour, for tests and dip colours alike.
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&rgba);
        }
        Frame {
            width,
            height,
            pixels,
        }
    }

    /// Build from a tightly packed RGB24 buffer (the pmv-platform
    /// decode shape), adding a fully opaque alpha channel.
    pub fn from_rgb24(width: u32, height: u32, rgb: &[u8]) -> Result<Frame, RenderError> {
        let expected = width as usize * height as usize * 3;
        if rgb.len() != expected {
            return Err(RenderError::BadFrame {
                reason: format!(
                    "RGB buffer has {} bytes, expected {expected} for {width}x{height}",
                    rgb.len()
                ),
            });
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for (dst, src) in pixels.chunks_exact_mut(4).zip(rgb.chunks_exact(3)) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = 255;
        }
        Ok(Frame {
            width,
            height,
            pixels,
        })
    }

    /// Tightly packed RGB24 bytes, the shape the platform encoder
    /// consumes. Alpha is dropped.
    pub fn to_rgb24(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width as usize * self.height as usize * 3];
        for (dst, src) in out.chunks_exact_mut(3).zip(self.pixels.chunks_exact(4)) {
            dst.copy_from_slice(&src[0..3]);
        }
        out
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let mut out = [0u8; 4];
        out.copy_from_slice(&self.pixels[offset..offset + 4]);
        out
    }

    /// Bilinear resize to exactly `width` by `height`. Used to bring
    /// decoded frames to the export size; this resamples (it can
    /// stretch) rather than cropping, which is the documented
    /// behaviour of this render slice.
    pub fn resized(&self, width: u32, height: u32) -> Frame {
        if self.width == width && self.height == height {
            return self.clone();
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            // Map the destination pixel centre into source texel
            // space, matching the sampler in the composite module.
            let sy = (y as f32 + 0.5) * (self.height as f32 / height as f32) - 0.5;
            for x in 0..width {
                let sx = (x as f32 + 0.5) * (self.width as f32 / width as f32) - 0.5;
                let sample = sample_bilinear(self, sx, sy);
                let offset = (y as usize * width as usize + x as usize) * 4;
                for channel in 0..4 {
                    pixels[offset + channel] = round_u8(sample[channel]);
                }
            }
        }
        Frame {
            width,
            height,
            pixels,
        }
    }
}

/// Round a float channel value to the nearest byte, clamped.
pub(crate) fn round_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Bilinear sample of a frame at texel space coordinates `sx`, `sy`,
/// where texel centres sit on integers and coordinates outside the
/// frame clamp to the edge texels. This is exactly the convention
/// wgpu linear sampling uses with clamp to edge addressing, which is
/// what keeps the CPU oracle and the GPU compositor in agreement.
pub(crate) fn sample_bilinear(frame: &Frame, sx: f32, sy: f32) -> [f32; 4] {
    let max_x = frame.width.saturating_sub(1) as f32;
    let max_y = frame.height.saturating_sub(1) as f32;
    let sx = sx.clamp(0.0, max_x);
    let sy = sy.clamp(0.0, max_y);
    let x0 = sx.floor() as u32;
    let y0 = sy.floor() as u32;
    let x1 = (x0 + 1).min(frame.width - 1);
    let y1 = (y0 + 1).min(frame.height - 1);
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let mut out = [0f32; 4];
    for channel in 0..4 {
        let top = lerp(
            texel(frame, x0, y0, channel),
            texel(frame, x1, y0, channel),
            fx,
        );
        let bottom = lerp(
            texel(frame, x0, y1, channel),
            texel(frame, x1, y1, channel),
            fx,
        );
        out[channel] = lerp(top, bottom, fy);
    }
    out
}

fn texel(frame: &Frame, x: u32, y: u32, channel: usize) -> f32 {
    frame.pixels[(y as usize * frame.width as usize + x as usize) * 4 + channel] as f32
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
