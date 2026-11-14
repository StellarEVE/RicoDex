//! The media error type, flattening platform, IO and cache failures
//! at the crate border.

use std::fmt;

use hyperdex_platform::PlatformError;

#[derive(Debug)]
pub enum MediaError {
    Platform(PlatformError),
    Io(std::io::Error),
    /// Writing a poster or filmstrip PNG failed.
    Image { reason: String },
    /// The file analysed cleanly at the container level but cannot
    /// feed an analysis stage (for example a zero duration probe).
    Analysis { reason: String },
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::Platform(e) => write!(f, "{e}"),
            MediaError::Io(e) => write!(f, "io error: {e}"),
            MediaError::Image { reason } => write!(f, "image write failed: {reason}"),
            MediaError::Analysis { reason } => write!(f, "analysis failed: {reason}"),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<PlatformError> for MediaError {
    fn from(error: PlatformError) -> MediaError {
        MediaError::Platform(error)
    }
}

impl From<std::io::Error> for MediaError {
    fn from(error: std::io::Error) -> MediaError {
        MediaError::Io(error)
    }
}
