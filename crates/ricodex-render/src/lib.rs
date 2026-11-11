//! music video-render: export rendering. Turns a validated timeline into
//! finished video files through the owned compositor and the rented
//! encoders, with a render queue and honest progress.
//!
//! This slice lands the first render path:
//!
//! 1. [`Frame`], the owned RGBA frame everything works on.
//! 2. The CPU reference compositor ([`composite_cpu`]), the oracle:
//!    every named transition from the product specification as real
//!    per pixel maths.
//! 3. The GPU compositor ([`GpuCompositor`]), the fast path: Cross
//!    Dissolve, Dip to Color and Crash Zoom as WGSL shaders, tested
//!    against the oracle.
//! 4. CPU effects ([`EffectOp`], [`apply_effects`]): brightness,
//!    contrast, saturation, blur, sharpen, as data driven ops.
//! 5. The render job ([`render_project`]): a two clip project with
//!    one transition, decoded and encoded through music video-platform,
//!    video only in this slice.
//! 6. The content addressed render cache ([`RenderCache`]) for
//!    transition regions, keyed by everything that determines the
//!    pixels.
//! 7. The sequential export ([`render_sequential`]): any number of
//!    enabled items on one track, decoded and encoded back to back
//!    with hard cuts. This is the export the app drives; the two
//!    clip transition job stays the narrow, cache backed path.
//!
//! All FFmpeg access stays in music video-platform; this crate consumes its
//! plain data frames and its encoder, and never names an FFmpeg
//! type.

pub mod cache;
pub mod composite;
pub mod effects;
pub mod error;
pub mod frame;
pub mod gpu;
pub mod job;
pub mod transition;

pub use cache::{RenderCache, TransitionUnitKey};
pub use composite::composite_cpu;
pub use effects::{apply_effects, describe_stack, EffectOp};
pub use error::RenderError;
pub use frame::Frame;
pub use gpu::{frame_difference, GpuCompositor};
pub use job::{
    render_project, render_sequential, RenderOutcome, RenderRequest, SequentialOutcome,
    TransitionSpec,
};
pub use transition::{TransitionKind, TransitionParams};
