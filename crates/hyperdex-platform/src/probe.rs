//! Stage 0 probing: container metadata only. Opening an input and
//! reading its stream headers is all that happens here; no frame is
//! ever decoded by this module.

use std::ffi::{CStr, CString};
use std::path::Path;

use hyperdex_core::{Tick, TICKS_PER_SECOND};
use rsmpeg::avcodec::AVCodecRef;
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::ffi;

use crate::decode::open_input;
use crate::error::PlatformError;

/// Container metadata for one video file, as plain data. Duration is in
/// ticks, the only time unit the rest of the workspace stores.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeInfo {
    pub duration: Tick,
    pub width: u32,
    pub height: u32,
    /// Frames per second when the container states a rate, else 0.0.
    pub frame_rate: f64,
    pub codec_name: String,
    /// Clockwise rotation in degrees (0, 90, 180, 270) when the
    /// container carries one, from stream metadata or the display
    /// matrix side data.
    pub rotation: Option<i32>,
}

/// Probe a file without decoding any frame.
pub fn probe(path: &Path) -> Result<ProbeInfo, PlatformError> {
    let (input, video_index, codec) = open_input(path)?;
    Ok(probe_of(&input, video_index, &codec))
}

pub(crate) fn probe_of(
    input: &AVFormatContextInput,
    video_index: usize,
    codec: &AVCodecRef<'static>,
) -> ProbeInfo {
    let streams = input.streams();
    let stream = &streams[video_index];
    let codecpar = stream.codecpar();

    let frame_rate = {
        let avg = stream.avg_frame_rate;
        if avg.num > 0 && avg.den > 0 {
            avg.num as f64 / avg.den as f64
        } else if let Some(guessed) = stream.guess_framerate() {
            if guessed.num > 0 && guessed.den > 0 {
                guessed.num as f64 / guessed.den as f64
            } else {
                0.0
            }
        } else {
            0.0
        }
    };

    // The format context duration is in AV_TIME_BASE (microsecond)
    // units. Fall back to the stream duration in stream time base when
    // the container does not state one.
    let duration = if input.duration > 0 {
        Tick::from_ticks(
            (input.duration as i128 * TICKS_PER_SECOND as i128 / 1_000_000) as i64,
        )
    } else if stream.duration > 0 {
        crate::decode::ticks_from_stream_ts(stream.duration, stream.time_base)
    } else {
        Tick::ZERO
    };

    ProbeInfo {
        duration,
        width: codecpar.width.max(0) as u32,
        height: codecpar.height.max(0) as u32,
        frame_rate,
        codec_name: codec.name().to_string_lossy().into_owned(),
        rotation: stream_rotation(stream),
    }
}

/// Rotation of a stream, when the container carries one: the legacy
/// `rotate` metadata key first, then the display matrix side data on
/// the codec parameters.
fn stream_rotation(stream: &rsmpeg::avformat::AVStreamRef<'_>) -> Option<i32> {
    if let Some(metadata) = stream.metadata() {
        let key = CString::new("rotate").unwrap();
        if let Some(entry) = metadata.get(&key, None, 0) {
            if !entry.value.is_null() {
                let value = unsafe { CStr::from_ptr(entry.value) }
                    .to_string_lossy()
                    .into_owned();
                if let Ok(degrees) = value.trim().parse::<i32>() {
                    return Some(normalize_degrees(degrees));
                }
            }
        }
    }

    let codecpar = stream.codecpar();
    let count = codecpar.nb_coded_side_data.max(0) as usize;
    if !codecpar.coded_side_data.is_null() && count > 0 {
        let entries =
            unsafe { std::slice::from_raw_parts(codecpar.coded_side_data, count) };
        for entry in entries {
            if entry.type_ == ffi::AV_PKT_DATA_DISPLAYMATRIX && entry.size >= 9 * 4 {
                let matrix = entry.data as *const i32;
                let angle = unsafe { ffi::av_display_rotation_get(matrix) };
                // av_display_rotation_get reports the counterclockwise
                // angle the frame must be rotated by for display in the
                // stored orientation convention; containers express the
                // correction as a clockwise rotation.
                let degrees = (-angle).round() as i32;
                return Some(normalize_degrees(degrees));
            }
        }
    }
    None
}

fn normalize_degrees(degrees: i32) -> i32 {
    ((degrees % 360) + 360) % 360
}
