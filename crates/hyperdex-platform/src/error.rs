//! The platform error type: every failure of the FFmpeg facing code,
//! flattened to plain messages at the crate border so no FFmpeg error
//! type leaks into the rest of the workspace.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum PlatformError {
    /// The path could not be turned into a C string (interior NUL).
    InvalidPath { path: PathBuf },
    /// FFmpeg refused to open the input, or found no streams in it.
    OpenFailed { path: PathBuf, reason: String },
    /// The input opened but holds no video stream.
    NoVideoStream { path: PathBuf },
    /// A decode, seek or scale operation failed mid stream.
    DecodeFailed { reason: String },
    /// An encode or mux operation failed.
    EncodeFailed { reason: String },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::InvalidPath { path } => {
                write!(f, "path {} contains an interior NUL byte", path.display())
            }
            PlatformError::OpenFailed { path, reason } => {
                write!(f, "cannot open {}: {reason}", path.display())
            }
            PlatformError::NoVideoStream { path } => {
                write!(f, "{} contains no video stream", path.display())
            }
            PlatformError::DecodeFailed { reason } => write!(f, "decode failed: {reason}"),
            PlatformError::EncodeFailed { reason } => write!(f, "encode failed: {reason}"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl From<rsmpeg::error::RsmpegError> for PlatformError {
    fn from(error: rsmpeg::error::RsmpegError) -> PlatformError {
        PlatformError::DecodeFailed {
            reason: error.to_string(),
        }
    }
}
