//! ricodex-media: footage analysis. Probes media files, computes content
//! fingerprints, and runs the staged analysis passes behind the media
//! pipeline.
//!
//! The staged pipeline is the locked design: Stage 0 probes container
//! metadata without decoding, Stage 1 samples sparsely for a rough
//! cut (filmstrips, coarse scenes, black ranges, rough motion, all
//! marked partial), and Stage 2 makes one full decode for settled
//! results (exact scenes, motion scores, best moments), reporting
//! progress as it goes. Analysis results are cached by content
//! fingerprint in one cache home, outside any project file, and are
//! always disposable.
//!
//! The analysis data types here mirror ricodex-engine's `SceneInfo`,
//! `BlackRange` and `SourceInfo` field for field, with the same
//! `is_partial` meaning, so the engine (which depends on this crate)
//! converts with a plain field copy. All FFmpeg access lives in
//! ricodex-platform; this crate never names an FFmpeg type.

pub mod analysis;
pub mod cache;
pub mod error;
pub mod fingerprint;
pub mod scoring;

pub use analysis::{
    analyse_stage1, analyse_stage2, probe, MediaBlackRange, MediaProbe, MediaScene,
    SourceAnalysis, Stage1Output,
};
pub use cache::{AnalysisCache, CachedStage1};
pub use error::MediaError;
pub use fingerprint::{compute_fingerprint, fingerprint_key};
