//! ricodex-platform: the sole home of hardware and operating system media
//! access. Hardware decode and encode selection, capability probing by
//! real trial decode and encode, and platform media frameworks live here
//! and nowhere else.
//!
//! Everything FFmpeg shaped in the workspace goes through this crate:
//! it wraps the rsmpeg binding (rsmpeg 0.18.0 against FFmpeg 8.1.2, the
//! triple proved by the binding spike) and hands the rest of the
//! workspace plain data: a [`CapabilityReport`], container
//! [`ProbeInfo`], decoded [`GrayFrame`]s and RGB frames, and
//! [`TrialDecodeReport`]s. Higher crates never name an FFmpeg type.
//!
//! Building this crate on a machine whose FFmpeg is a runtime only
//! install needs the dev overlay environment documented in
//! crates/ricodex-media/README.md.

pub mod capability;
pub mod decode;
pub mod encode;
pub mod error;
pub mod probe;

pub use capability::{capability_report, CapabilityReport, HwAccelMethod};
pub use decode::{trial_decode, GrayFrame, RgbFrame, TrialDecodeReport, VideoDecoder};
pub use encode::VideoEncoder;
// VideoDecoder::next_frame_pair is the paired gray plus RGB decode used
// by the analysis pipeline.
pub use error::PlatformError;
pub use probe::{probe, ProbeInfo};
