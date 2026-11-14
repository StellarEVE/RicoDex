//! The CPU reference compositor: the oracle. Every transition the
//! product names is implemented here in plain per pixel maths, and
//! the GPU compositor is tested against this module. Preview and
//! export both render through these functions (or their GPU twins),
//! so the two can never drift into different pictures.
//!
//! Conventions: `progress` runs 0.0 to 1.0 across the transition.
//! Progress at or below 0.0 returns frame A pixel exact and at or
//! above 1.0 returns frame B pixel exact, for every form; the
//! endpoint shortcut is deliberate, so a transition region joins its
//! neighbours seamlessly by construction. Warped forms sample with
//! the bilinear sampler in the frame module, in texel space with
//! clamp to edge, the same convention the GPU shaders use.

use crate::error::RenderError;
use crate::frame::{lerp, round_u8, sample_bilinear, Frame};
use crate::transition::{TransitionKind, TransitionParams};


/// Composite video layers from bottom to top over an opaque black canvas.
///
/// Later entries are visually above earlier entries. Source pixels use
/// straight alpha; the output is always opaque because it is composited
/// over the black export canvas.
pub fn composite_video_layers(
    width: u32,
    height: u32,
    layers: &[Frame],
) -> Result<Frame, RenderError> {
    let mut output = Frame::solid(width, height, [0, 0, 0, 255]);
    for layer in layers {
        if layer.width != width || layer.height != height {
            return Err(RenderError::SizeMismatch {
                a: (width, height),
                b: (layer.width, layer.height),
            });
        }
        for (dst, src) in output
            .pixels
            .as_chunks_mut::<4>().0
            .iter_mut()
            .zip(layer.pixels.as_chunks::<4>().0)
        {
            let alpha = src[3] as f32 / 255.0;
            let inverse = 1.0 - alpha;
            for channel in 0..3 {
                dst[channel] = round_u8(src[channel] as f32 * alpha + dst[channel] as f32 * inverse);
            }
            dst[3] = 255;
        }
    }
    Ok(output)
}

/// Composite one output frame from input frames `a` (outgoing) and
/// `b` (incoming) at `progress`. Both frames must share dimensions.
pub fn composite_cpu(
    a: &Frame,
    b: &Frame,
    kind: TransitionKind,
    progress: f32,
    params: &TransitionParams,
) -> Result<Frame, RenderError> {
    if a.width != b.width || a.height != b.height {
        return Err(RenderError::SizeMismatch {
            a: (a.width, a.height),
            b: (b.width, b.height),
        });
    }
    let p = progress.clamp(0.0, 1.0);
    if p <= 0.0 {
        return Ok(a.clone());
    }
    if p >= 1.0 {
        return Ok(b.clone());
    }
    let out = match kind {
        TransitionKind::CrossDissolve => dissolve(a, b, p, false),
        TransitionKind::FilmDissolve => dissolve(a, b, p, true),
        TransitionKind::DipToColor => dip_to_color(a, b, p, params),
        TransitionKind::DipToDissolve => dip_to_dissolve(a, b, p),
        TransitionKind::CrashZoom => crash_zoom(a, b, p),
        TransitionKind::DirectionalZoom => directional_zoom(a, b, p, params),
        TransitionKind::TwistWheel => twist_wheel(a, b, p),
        TransitionKind::RadialTwist => radial_twist(a, b, p),
        TransitionKind::Push => push(a, b, p, params),
        TransitionKind::Slide => slide(a, b, p, params),
        TransitionKind::Split => split(a, b, p),
    };
    Ok(out)
}

fn empty_like(frame: &Frame) -> Vec<u8> {
    vec![0u8; frame.pixels.len()]
}

fn put(out: &mut [u8], frame: &Frame, x: u32, y: u32, rgba: [f32; 4]) {
    let offset = (y as usize * frame.width as usize + x as usize) * 4;
    for channel in 0..4 {
        out[offset + channel] = round_u8(rgba[channel]);
    }
}

/// Plain dissolve. With `linear_light` set, the blend happens in
/// linear light: channels are run through the sRGB transfer function,
/// mixed, and converted back. That is the film dissolve: real film
/// and light mix linearly, so mid blends come out brighter than a
/// naive stored value crossfade. (True film dissolve also models
/// film stock response; the linear light blend is the documented
/// approximation used here.)
fn dissolve(a: &Frame, b: &Frame, p: f32, linear_light: bool) -> Frame {
    let mut pixels = empty_like(a);
    for (index, (pa, pb)) in a
        .pixels
        .as_chunks::<4>().0
        .iter()
        .zip(b.pixels.as_chunks::<4>().0)
        .enumerate()
    {
        for channel in 0..3 {
            let va = pa[channel] as f32;
            let vb = pb[channel] as f32;
            let mixed = if linear_light {
                let la = srgb_to_linear(va / 255.0);
                let lb = srgb_to_linear(vb / 255.0);
                linear_to_srgb(lerp(la, lb, p)) * 255.0
            } else {
                lerp(va, vb, p)
            };
            pixels[index * 4 + channel] = round_u8(mixed);
        }
        pixels[index * 4 + 3] = round_u8(lerp(pa[3] as f32, pb[3] as f32, p));
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Dip to Color: the outgoing frame fades to the dip colour over the
/// first half, the incoming frame fades in from it over the second.
fn dip_to_color(a: &Frame, b: &Frame, p: f32, params: &TransitionParams) -> Frame {
    let dip = [
        params.dip_color[0] as f32,
        params.dip_color[1] as f32,
        params.dip_color[2] as f32,
        255.0,
    ];
    let mut pixels = empty_like(a);
    for (index, (pa, pb)) in a
        .pixels
        .as_chunks::<4>().0
        .iter()
        .zip(b.pixels.as_chunks::<4>().0)
        .enumerate()
    {
        for channel in 0..4 {
            let va = pa[channel] as f32;
            let vb = pb[channel] as f32;
            let mixed = if p < 0.5 {
                lerp(va, dip[channel], p * 2.0)
            } else {
                lerp(dip[channel], vb, (p - 0.5) * 2.0)
            };
            pixels[index * 4 + channel] = round_u8(mixed);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Dip to Dissolve: a cross dissolve whose brightness dips through
/// the middle, as if the blend passed partway through black.
/// Approximation: the dissolved frame is multiplied by a factor that
/// falls to 1 minus DIP_DEPTH at the midpoint and recovers at both
/// ends. Alpha is a plain dissolve.
const DIP_DEPTH: f32 = 0.85;

fn dip_to_dissolve(a: &Frame, b: &Frame, p: f32) -> Frame {
    let factor = 1.0 - DIP_DEPTH * (1.0 - (2.0 * p - 1.0).abs());
    let mut pixels = empty_like(a);
    for (index, (pa, pb)) in a
        .pixels
        .as_chunks::<4>().0
        .iter()
        .zip(b.pixels.as_chunks::<4>().0)
        .enumerate()
    {
        for channel in 0..3 {
            let mixed = lerp(pa[channel] as f32, pb[channel] as f32, p) * factor;
            pixels[index * 4 + channel] = round_u8(mixed);
        }
        pixels[index * 4 + 3] = round_u8(lerp(pa[3] as f32, pb[3] as f32, p));
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sample `frame` at uv coordinates, returning float RGBA. The uv
/// space origin is the top left, (0.5, 0.5) the centre.
fn sample_uv(frame: &Frame, uv: (f32, f32)) -> [f32; 4] {
    let sx = uv.0 * frame.width as f32 - 0.5;
    let sy = uv.1 * frame.height as f32 - 0.5;
    sample_bilinear(frame, sx, sy)
}

fn zoom_uv(uv: (f32, f32), centre: (f32, f32), zoom: f32) -> (f32, f32) {
    (
        centre.0 + (uv.0 - centre.0) / zoom,
        centre.1 + (uv.1 - centre.1) / zoom,
    )
}

/// Crash Zoom: the outgoing frame punches in towards the centre
/// while the incoming frame settles back from the same punch, and
/// the cut over happens across the middle of the move. This is the
/// same maths as the WGSL shader the compositor spike proved.
fn crash_zoom(a: &Frame, b: &Frame, p: f32) -> Frame {
    warped_pair(a, b, p, (0.5, 0.5), (0.5, 0.5), 3.0)
}

/// Directional Zoom: a gentler zoom whose focal point sits off
/// centre, drifted along the travel direction, so the frame rushes
/// towards a point rather than the middle.
fn directional_zoom(a: &Frame, b: &Frame, p: f32, params: &TransitionParams) -> Frame {
    let (dx, dy) = params.direction;
    let len = (dx * dx + dy * dy).sqrt();
    let (dx, dy) = if len > f32::EPSILON {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let focal = (0.5 + 0.30 * dx, 0.5 + 0.30 * dy);
    warped_pair(a, b, p, focal, focal, 1.5)
}

/// The shared body of the zoom forms: frame A zooms in with
/// strength `power` (zoom factor 1 plus power times progress
/// squared) around `focal_a`, frame B starts at the mirrored zoom
/// around `focal_b` and settles, and the mix crosses over on a
/// smoothstep across the middle.
fn warped_pair(
    a: &Frame,
    b: &Frame,
    p: f32,
    focal_a: (f32, f32),
    focal_b: (f32, f32),
    power: f32,
) -> Frame {
    let zoom_a = 1.0 + power * p * p;
    let zoom_b = 1.0 + power * (1.0 - p) * (1.0 - p);
    let cut = smoothstep(0.35, 0.65, p);
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let uv = (
                (x as f32 + 0.5) / a.width as f32,
                (y as f32 + 0.5) / a.height as f32,
            );
            let sa = sample_uv(a, zoom_uv(uv, focal_a, zoom_a));
            let sb = sample_uv(b, zoom_uv(uv, focal_b, zoom_b));
            let mut out = [0f32; 4];
            for channel in 0..4 {
                out[channel] = lerp(sa[channel], sb[channel], cut);
            }
            put(&mut pixels, a, x, y, out);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Rotate a uv offset by `angle` radians (counterclockwise in a
/// y-down frame, which reads as clockwise on screen; the sign only
/// sets the twist direction).
fn rotate(offset: (f32, f32), angle: f32) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    (
        offset.0 * c - offset.1 * s,
        offset.0 * s + offset.1 * c,
    )
}

/// Twist Wheel: the two frames counter rotate around the centre
/// like a wheel being spun, swelling slightly at the midpoint,
/// crossing over across the middle of the move.
fn twist_wheel(a: &Frame, b: &Frame, p: f32) -> Frame {
    const MAX_TWIST: f32 = 0.9;
    let swell = 1.0 + 0.25 * (std::f32::consts::PI * p).sin();
    let angle_a = -MAX_TWIST * p;
    let angle_b = MAX_TWIST * (1.0 - p);
    let cut = smoothstep(0.35, 0.65, p);
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let uv = (
                (x as f32 + 0.5) / a.width as f32,
                (y as f32 + 0.5) / a.height as f32,
            );
            let offset = (uv.0 - 0.5, uv.1 - 0.5);
            // The displayed frame is rotated by angle; sampling runs
            // the rotation backwards and applies the swell as a zoom
            // out (offsets grow).
            let back_a = rotate(offset, -angle_a);
            let back_b = rotate(offset, -angle_b);
            let sa = sample_uv(
                a,
                (0.5 + back_a.0 * swell, 0.5 + back_a.1 * swell),
            );
            let sb = sample_uv(
                b,
                (0.5 + back_b.0 * swell, 0.5 + back_b.1 * swell),
            );
            let mut out = [0f32; 4];
            for channel in 0..4 {
                out[channel] = lerp(sa[channel], sb[channel], cut);
            }
            put(&mut pixels, a, x, y, out);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Radial Twist: a vortex. Both frames are sampled through a swirl
/// whose angle falls with distance from the centre and peaks at the
/// midpoint of the transition, A swirled one way and B the other,
/// under a plain cross dissolve. Unlike Twist Wheel nothing rotates
/// rigidly and nothing scales: the edges stay put while the middle
/// wrings.
fn radial_twist(a: &Frame, b: &Frame, p: f32) -> Frame {
    const MAX_SWIRL: f32 = 2.4;
    let strength = MAX_SWIRL * (std::f32::consts::PI * p).sin();
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let uv = (
                (x as f32 + 0.5) / a.width as f32,
                (y as f32 + 0.5) / a.height as f32,
            );
            let offset = (uv.0 - 0.5, uv.1 - 0.5);
            let r = (offset.0 * offset.0 + offset.1 * offset.1).sqrt() / std::f32::consts::FRAC_1_SQRT_2;
            let falloff = (1.0 - r.clamp(0.0, 1.0)).powi(2);
            let theta = strength * falloff;
            let back_a = rotate(offset, -theta);
            let back_b = rotate(offset, theta);
            let sa = sample_uv(a, (0.5 + back_a.0, 0.5 + back_a.1));
            let sb = sample_uv(b, (0.5 + back_b.0, 0.5 + back_b.1));
            let mut out = [0f32; 4];
            for channel in 0..4 {
                out[channel] = lerp(sa[channel], sb[channel], p);
            }
            put(&mut pixels, a, x, y, out);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Push: the incoming frame shoves the outgoing one out. The two
/// frames sit side by side on a strip and the window slides across
/// it, so the outgoing frame genuinely moves. With direction
/// (1, 0) the new frame enters from the right.
fn push(a: &Frame, b: &Frame, p: f32, params: &TransitionParams) -> Frame {
    let w = a.width as f32;
    let forward = params.direction.0 >= 0.0;
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let (frame, sx) = if forward {
                let strip = x as f32 + p * w;
                if strip < w {
                    (a, strip)
                } else {
                    (b, strip - w)
                }
            } else {
                let strip = x as f32 + (1.0 - p) * w;
                if strip < w {
                    (b, strip)
                } else {
                    (a, strip - w)
                }
            };
            let sample = sample_bilinear(frame, sx, y as f32);
            put(&mut pixels, a, x, y, sample);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Slide: the incoming frame slides over the outgoing one, which
/// stays exactly where it was (the difference from Push). With
/// direction (1, 0) the new frame enters from the right.
fn slide(a: &Frame, b: &Frame, p: f32, params: &TransitionParams) -> Frame {
    let w = a.width as f32;
    let forward = params.direction.0 >= 0.0;
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let xf = x as f32;
            let sample = if forward {
                let edge = (1.0 - p) * w;
                if xf >= edge {
                    sample_bilinear(b, xf - edge, y as f32)
                } else {
                    sample_bilinear(a, xf, y as f32)
                }
            } else {
                let edge = p * w;
                if xf < edge {
                    sample_bilinear(b, xf + (1.0 - p) * w, y as f32)
                } else {
                    sample_bilinear(a, xf, y as f32)
                }
            };
            put(&mut pixels, a, x, y, sample);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}

/// Split: the outgoing frame parts down the middle, its two halves
/// sliding apart horizontally to reveal the incoming frame beneath.
fn split(a: &Frame, b: &Frame, p: f32) -> Frame {
    let w = a.width as f32;
    let half = w / 2.0;
    let open = p * half;
    let mut pixels = empty_like(a);
    for y in 0..a.height {
        for x in 0..a.width {
            let xf = x as f32;
            let sample = if xf < half - open {
                sample_bilinear(a, xf + open, y as f32)
            } else if xf >= half + open {
                sample_bilinear(a, xf - open, y as f32)
            } else {
                sample_bilinear(b, xf, y as f32)
            };
            put(&mut pixels, a, x, y, sample);
        }
    }
    Frame {
        width: a.width,
        height: a.height,
        pixels,
    }
}
