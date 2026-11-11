//! The render error type: every failure of compositing, effects,
//! caching and the render job, as plain variants at the crate border.

use std::fmt;

#[derive(Debug)]
pub enum RenderError {
    /// The two frames handed to the compositor disagree in size.
    SizeMismatch {
        a: (u32, u32),
        b: (u32, u32),
    },
    /// A frame buffer does not match its stated dimensions.
    BadFrame { reason: String },
    /// The GPU compositor ran but failed mid render.
    Gpu { reason: String },
    /// The project handed to the render job is not the shape this
    /// slice renders (see the job module docs).
    BadProject { reason: String },
    /// A source file could not be decoded or probed.
    Platform(pmv_platform::PlatformError),
    /// Filesystem failure in the cache or at the output path.
    Io(std::io::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::SizeMismatch { a, b } => {
                write!(f, "frame sizes differ: {}x{} vs {}x{}", a.0, a.1, b.0, b.1)
            }
            RenderError::BadFrame { reason } => write!(f, "bad frame: {reason}"),
            RenderError::Gpu { reason } => write!(f, "gpu compositor failed: {reason}"),
            RenderError::BadProject { reason } => write!(f, "cannot render project: {reason}"),
            RenderError::Platform(e) => write!(f, "{e}"),
            RenderError::Io(e) => write!(f, "io failed: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<pmv_platform::PlatformError> for RenderError {
    fn from(error: pmv_platform::PlatformError) -> RenderError {
        RenderError::Platform(error)
    }
}

impl From<std::io::Error> for RenderError {
    fn from(error: std::io::Error) -> RenderError {
        RenderError::Io(error)
    }
}
