//! Video decoding over rsmpeg. This module is the only place in the
//! workspace that drives a decoder: open an input once, then seek and
//! pull frames scaled down to small grayscale frames for analysis or
//! small RGB frames for posters and filmstrips. It also hosts the
//! trial decode used by capability selection: a real decode attempt
//! whose success or failure reason is reported as data, never assumed
//! from a probe.

use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};

use ricodex_core::{Tick, TICKS_PER_SECOND};
use rsmpeg::avcodec::{AVCodecContext, AVCodecRef};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;
use rsmpeg::swscale::SwsContext;

use crate::error::PlatformError;
use crate::probe::{probe_of, ProbeInfo};

/// One decoded frame scaled to small grayscale: tightly packed rows,
/// `pixels.len() == width * height`, values are luma 0 to 255.
#[derive(Clone, Debug, PartialEq)]
pub struct GrayFrame {
    pub time: Tick,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// One decoded frame scaled to small RGB: tightly packed rows of RGB
/// triples, `pixels.len() == width * height * 3`.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbFrame {
    pub time: Tick,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Open an input and find its best video stream. Shared by probing,
/// decoding and trial decode.
pub(crate) fn open_input(
    path: &Path,
) -> Result<(AVFormatContextInput, usize, AVCodecRef<'static>), PlatformError> {
    let path_str = path
        .to_str()
        .ok_or_else(|| PlatformError::InvalidPath {
            path: path.to_path_buf(),
        })?;
    let cpath = CString::new(path_str).map_err(|_| PlatformError::InvalidPath {
        path: path.to_path_buf(),
    })?;
    let input = AVFormatContextInput::open(&cpath).map_err(|e| PlatformError::OpenFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let (index, codec) = input
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .map_err(|e| PlatformError::OpenFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?
        .ok_or_else(|| PlatformError::NoVideoStream {
            path: path.to_path_buf(),
        })?;
    Ok((input, index, codec))
}

/// Convert a stream timestamp in stream time base units to ticks with
/// exact integer maths.
pub(crate) fn ticks_from_stream_ts(ts: i64, time_base: ffi::AVRational) -> Tick {
    if time_base.num <= 0 || time_base.den <= 0 {
        return Tick::ZERO;
    }
    let ticks =
        ts as i128 * time_base.num as i128 * TICKS_PER_SECOND as i128 / time_base.den as i128;
    Tick::from_ticks(ticks as i64)
}

fn stream_ts_from_ticks(time: Tick, time_base: ffi::AVRational) -> i64 {
    if time_base.num <= 0 || time_base.den <= 0 {
        return 0;
    }
    (time.as_ticks() as i128 * time_base.den as i128
        / (TICKS_PER_SECOND as i128 * time_base.num as i128)) as i64
}

/// The target size for a frame scaled to at most `max_width` wide,
/// keeping aspect. Frames already narrow enough keep their size.
fn scaled_size(src_w: u32, src_h: u32, max_width: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || src_w <= max_width {
        return (src_w, src_h);
    }
    let h = ((src_h as u64 * max_width as u64) / src_w as u64).max(1) as u32;
    (max_width, h)
}

/// A cached swscale context plus the shape it was built for.
struct Scaler {
    ctx: SwsContext,
    src_w: i32,
    src_h: i32,
    src_fmt: i32,
    dst_w: i32,
    dst_h: i32,
    dst_fmt: i32,
}

impl Scaler {
    fn matches(&self, frame: &AVFrame, dst_w: i32, dst_h: i32, dst_fmt: i32) -> bool {
        self.src_w == frame.width
            && self.src_h == frame.height
            && self.src_fmt == frame.format
            && self.dst_w == dst_w
            && self.dst_h == dst_h
            && self.dst_fmt == dst_fmt
    }
}

/// A video file opened for decoding, with its probe alongside.
pub struct VideoDecoder {
    input: AVFormatContextInput,
    decoder: AVCodecContext,
    video_index: usize,
    time_base: ffi::AVRational,
    probe: ProbeInfo,
    eof_sent: bool,
    finished: bool,
    frames_produced: u64,
    gray_scaler: Option<Scaler>,
    rgb_scaler: Option<Scaler>,
}

impl VideoDecoder {
    pub fn open(path: &Path) -> Result<VideoDecoder, PlatformError> {
        let (input, video_index, codec) = open_input(path)?;
        let probe = probe_of(&input, video_index, &codec);
        let mut decoder = AVCodecContext::new(&codec);
        {
            let streams = input.streams();
            decoder.apply_codecpar(&streams[video_index].codecpar())?;
        }
        decoder.open(None)?;
        let time_base = input.streams()[video_index].time_base;
        Ok(VideoDecoder {
            input,
            decoder,
            video_index,
            time_base,
            probe,
            eof_sent: false,
            finished: false,
            frames_produced: 0,
            gray_scaler: None,
            rgb_scaler: None,
        })
    }

    pub fn probe_info(&self) -> &ProbeInfo {
        &self.probe
    }

    pub fn duration(&self) -> Tick {
        self.probe.duration
    }

    /// Seek to the keyframe at or before `time` and reset the decoder,
    /// so the next frames produced are the first ones at or after it.
    pub fn seek(&mut self, time: Tick) -> Result<(), PlatformError> {
        let ts = stream_ts_from_ticks(time, self.time_base);
        self.input.seek(
            self.video_index as i32,
            ts,
            ffi::AVSEEK_FLAG_BACKWARD as i32,
        )?;
        self.decoder.flush_buffers();
        self.eof_sent = false;
        self.finished = false;
        Ok(())
    }

    fn is_drain(error: &RsmpegError) -> bool {
        matches!(error, RsmpegError::DecoderDrainError) || error.raw_error() == Some(-11)
    }

    fn is_flushed(error: &RsmpegError) -> bool {
        matches!(error, RsmpegError::DecoderFlushedError)
            || error.raw_error() == Some(ffi::AVERROR_EOF)
    }

    /// Pull the next decoded frame in presentation order.
    fn next_frame(&mut self) -> Result<Option<AVFrame>, PlatformError> {
        if self.finished {
            return Ok(None);
        }
        loop {
            match self.decoder.receive_frame() {
                Ok(frame) => {
                    self.frames_produced += 1;
                    return Ok(Some(frame));
                }
                Err(e) if Self::is_drain(&e) => {
                    if self.eof_sent {
                        self.finished = true;
                        return Ok(None);
                    }
                    match self.input.read_packet()? {
                        Some(packet) => {
                            if packet.stream_index == self.video_index as i32 {
                                match self.decoder.send_packet(Some(&packet)) {
                                    Ok(()) => {}
                                    Err(se)
                                        if matches!(
                                            se,
                                            RsmpegError::DecoderFullError
                                        ) => {}
                                    Err(se) => return Err(se.into()),
                                }
                            }
                        }
                        None => {
                            self.decoder.send_packet(None)?;
                            self.eof_sent = true;
                        }
                    }
                }
                Err(e) if Self::is_flushed(&e) => {
                    self.finished = true;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn frame_time(&self, frame: &AVFrame) -> Tick {
        let ts = frame.best_effort_timestamp;
        if ts != ffi::AV_NOPTS_VALUE {
            ticks_from_stream_ts(ts, self.time_base)
        } else {
            // No timestamp on the frame: estimate from the count of
            // frames produced so far and the probed frame rate.
            let fps = if self.probe.frame_rate > 0.0 {
                self.probe.frame_rate
            } else {
                25.0
            };
            Tick::from_seconds(self.frames_produced.saturating_sub(1) as f64 / fps)
        }
    }

    fn scaler<'a>(
        slot: &'a mut Option<Scaler>,
        frame: &AVFrame,
        dst_w: u32,
        dst_h: u32,
        dst_fmt: i32,
    ) -> Result<&'a mut SwsContext, PlatformError> {
        let rebuild = match slot {
            Some(scaler) => !scaler.matches(frame, dst_w as i32, dst_h as i32, dst_fmt),
            None => true,
        };
        if rebuild {
            let ctx = SwsContext::get_context(
                frame.width,
                frame.height,
                frame.format,
                dst_w as i32,
                dst_h as i32,
                dst_fmt,
                ffi::SWS_BILINEAR as ffi::SwsFlags,
                None,
                None,
                None,
            )
            .ok_or_else(|| PlatformError::DecodeFailed {
                reason: "cannot create swscale context".to_string(),
            })?;
            *slot = Some(Scaler {
                ctx,
                src_w: frame.width,
                src_h: frame.height,
                src_fmt: frame.format,
                dst_w: dst_w as i32,
                dst_h: dst_h as i32,
                dst_fmt,
            });
        }
        Ok(&mut slot.as_mut().unwrap().ctx)
    }

    fn scale_to(
        &mut self,
        frame: &AVFrame,
        max_width: u32,
        dst_fmt: i32,
        rgb: bool,
    ) -> Result<(Tick, u32, u32, Vec<u8>), PlatformError> {
        let time = self.frame_time(frame);
        let (dst_w, dst_h) = scaled_size(frame.width as u32, frame.height as u32, max_width);
        let mut dst = AVFrame::new();
        dst.set_width(dst_w as i32);
        dst.set_height(dst_h as i32);
        dst.set_format(dst_fmt);
        dst.alloc_buffer()?;
        {
            let slot = if rgb {
                &mut self.rgb_scaler
            } else {
                &mut self.gray_scaler
            };
            let ctx = Self::scaler(slot, frame, dst_w, dst_h, dst_fmt)?;
            ctx.scale_frame(frame, 0, frame.height, &mut dst)?;
        }
        let channels = if rgb { 3usize } else { 1usize };
        let row_bytes = dst_w as usize * channels;
        let mut pixels = vec![0u8; row_bytes * dst_h as usize];
        let stride = dst.linesize[0] as usize;
        let data = dst.data[0];
        if data.is_null() {
            return Err(PlatformError::DecodeFailed {
                reason: "scaled frame has no data".to_string(),
            });
        }
        for y in 0..dst_h as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.add(y * stride),
                    pixels.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }
        }
        Ok((time, dst_w, dst_h, pixels))
    }

    /// The next frame as small grayscale, or None at end of stream.
    pub fn next_gray_frame(&mut self, max_width: u32) -> Result<Option<GrayFrame>, PlatformError> {
        let Some(frame) = self.next_frame()? else {
            return Ok(None);
        };
        let (time, width, height, pixels) =
            self.scale_to(&frame, max_width, ffi::AV_PIX_FMT_GRAY8, false)?;
        Ok(Some(GrayFrame {
            time,
            width,
            height,
            pixels,
        }))
    }

    /// The next frame scaled twice from one decode: small grayscale
    /// for analysis and small RGB for display, sharing the frame's
    /// timestamp. Or None at end of stream.
    pub fn next_frame_pair(
        &mut self,
        max_width: u32,
    ) -> Result<Option<(GrayFrame, RgbFrame)>, PlatformError> {
        let Some(frame) = self.next_frame()? else {
            return Ok(None);
        };
        let (time, width, height, pixels) =
            self.scale_to(&frame, max_width, ffi::AV_PIX_FMT_GRAY8, false)?;
        let gray = GrayFrame {
            time,
            width,
            height,
            pixels,
        };
        let (time, width, height, pixels) =
            self.scale_to(&frame, max_width, ffi::AV_PIX_FMT_RGB24, true)?;
        let rgb = RgbFrame {
            time,
            width,
            height,
            pixels,
        };
        Ok(Some((gray, rgb)))
    }

    /// The next frame as small RGB, or None at end of stream.
    pub fn next_rgb_frame(&mut self, max_width: u32) -> Result<Option<RgbFrame>, PlatformError> {
        let Some(frame) = self.next_frame()? else {
            return Ok(None);
        };
        let (time, width, height, pixels) =
            self.scale_to(&frame, max_width, ffi::AV_PIX_FMT_RGB24, true)?;
        Ok(Some(RgbFrame {
            time,
            width,
            height,
            pixels,
        }))
    }
}

// ------------------------------------------------------------ trial decode

/// The outcome of one real trial decode, as data. Failure is a normal
/// result here: capability selection needs the reason, not an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialDecodeReport {
    /// The hardware acceleration method tried, or None for software.
    pub method: Option<String>,
    pub frames_decoded: usize,
    pub success: bool,
    /// Why the trial failed, when it did.
    pub failure: Option<String>,
}

/// Attempt to decode the first frames of `path`. With `hwaccel` None
/// this is a software decode; with Some(name) it is a real hardware
/// decode attempt through the named method (for example "vaapi" or
/// "cuda"), including device creation and selecting the hardware pixel
/// format, so success means the method genuinely works on this
/// machine for this file.
pub fn trial_decode(path: &Path, hwaccel: Option<&str>) -> TrialDecodeReport {
    match hwaccel {
        None => trial_decode_software(path),
        Some(name) => trial_decode_hardware(path, name),
    }
}

fn trial_decode_software(path: &Path) -> TrialDecodeReport {
    let failure = |reason: String| TrialDecodeReport {
        method: None,
        frames_decoded: 0,
        success: false,
        failure: Some(reason),
    };
    let mut decoder = match VideoDecoder::open(path) {
        Ok(decoder) => decoder,
        Err(e) => return failure(e.to_string()),
    };
    let mut frames = 0usize;
    loop {
        match decoder.next_gray_frame(160) {
            Ok(Some(_)) => {
                frames += 1;
                if frames >= 3 {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                return TrialDecodeReport {
                    method: None,
                    frames_decoded: frames,
                    success: false,
                    failure: Some(e.to_string()),
                }
            }
        }
    }
    if frames > 0 {
        TrialDecodeReport {
            method: None,
            frames_decoded: frames,
            success: true,
            failure: None,
        }
    } else {
        failure("decoder produced no frames".to_string())
    }
}

/// The hardware pixel format the get_format callback should select,
/// set just before a hardware decoder is opened. Trial decodes are
/// short and sequential by design, so one slot is enough; concurrent
/// hardware trials are not supported.
static HW_PIX_FMT: AtomicI32 = AtomicI32::new(ffi::AV_PIX_FMT_NONE);

unsafe extern "C" fn hw_get_format(
    _ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let wanted = HW_PIX_FMT.load(Ordering::SeqCst);
    let mut p = fmts;
    if !p.is_null() {
        while unsafe { *p } != ffi::AV_PIX_FMT_NONE {
            if unsafe { *p } == wanted {
                return wanted;
            }
            p = unsafe { p.add(1) };
        }
    }
    // The hardware format is not on offer: fall back to the first
    // format, which turns this into a software decode the caller
    // detects by the decoded frames not carrying the hardware format.
    if p.is_null() || fmts.is_null() {
        ffi::AV_PIX_FMT_NONE
    } else {
        unsafe { *fmts }
    }
}

fn trial_decode_hardware(path: &Path, name: &str) -> TrialDecodeReport {
    let failure = |reason: String| TrialDecodeReport {
        method: Some(name.to_string()),
        frames_decoded: 0,
        success: false,
        failure: Some(reason),
    };

    let cname = match CString::new(name) {
        Ok(cname) => cname,
        Err(_) => return failure("invalid method name".to_string()),
    };
    let device_type = unsafe { ffi::av_hwdevice_find_type_by_name(cname.as_ptr()) };
    if device_type == ffi::AV_HWDEVICE_TYPE_NONE {
        return failure(format!("unknown hardware acceleration method '{name}'"));
    }

    let (mut input, video_index, codec) = match open_input(path) {
        Ok(opened) => opened,
        Err(e) => return failure(e.to_string()),
    };

    // Find the hardware pixel format this decoder offers for the
    // requested device type.
    let mut hw_pix_fmt = ffi::AV_PIX_FMT_NONE;
    let mut index = 0;
    loop {
        match codec.hw_config(index) {
            Some(config) => {
                if config.device_type == device_type {
                    hw_pix_fmt = config.pix_fmt;
                    break;
                }
            }
            None => break,
        }
        index += 1;
    }
    if hw_pix_fmt == ffi::AV_PIX_FMT_NONE {
        return failure(format!(
            "decoder '{}' offers no {name} hardware configuration",
            codec.name().to_string_lossy()
        ));
    }

    let mut decoder = AVCodecContext::new(&codec);
    {
        let streams = input.streams();
        if let Err(e) = decoder.apply_codecpar(&streams[video_index].codecpar()) {
            return failure(e.to_string());
        }
    }

    // Create the hardware device and hand it to the decoder, with the
    // format callback that selects the hardware pixel format.
    let device = match rsmpeg::avutil::AVHWDeviceContext::create(device_type, None, None, 0) {
        Ok(device) => device,
        Err(e) => {
            return failure(format!(
                "cannot create a {name} hardware device on this machine: {e}"
            ))
        }
    };
    decoder.set_hw_device_ctx(device);
    HW_PIX_FMT.store(hw_pix_fmt, Ordering::SeqCst);
    unsafe {
        (*decoder.as_mut_ptr()).get_format = Some(hw_get_format);
    }
    if let Err(e) = decoder.open(None) {
        return failure(format!("cannot open hardware decoder: {e}"));
    }

    // Decode until the first frames arrive; success requires frames
    // that actually carry the hardware pixel format.
    let mut frames = 0usize;
    let mut hw_frames = 0usize;
    let mut eof_sent = false;
    let mut guard = 0usize;
    while hw_frames == 0 && guard < 512 {
        guard += 1;
        match decoder.receive_frame() {
            Ok(frame) => {
                frames += 1;
                if frame.format == hw_pix_fmt {
                    // Prove the pixels are usable by transferring the
                    // frame back to system memory.
                    let mut software = AVFrame::new();
                    match software.hwframe_transfer_data(&frame) {
                        Ok(()) => hw_frames += 1,
                        Err(e) => {
                            return TrialDecodeReport {
                                method: Some(name.to_string()),
                                frames_decoded: frames,
                                success: false,
                                failure: Some(format!(
                                    "hardware frame cannot transfer to memory: {e}"
                                )),
                            }
                        }
                    }
                }
                if frames >= 30 {
                    break;
                }
            }
            Err(e) if VideoDecoder::is_drain(&e) => {
                if eof_sent {
                    break;
                }
                match input.read_packet() {
                    Ok(Some(packet)) => {
                        if packet.stream_index == video_index as i32 {
                            let _ = decoder.send_packet(Some(&packet));
                        }
                    }
                    Ok(None) => {
                        let _ = decoder.send_packet(None);
                        eof_sent = true;
                    }
                    Err(e) => return failure(e.to_string()),
                }
            }
            Err(e) if VideoDecoder::is_flushed(&e) => break,
            Err(e) => return failure(e.to_string()),
        }
    }

    if hw_frames > 0 {
        TrialDecodeReport {
            method: Some(name.to_string()),
            frames_decoded: frames,
            success: true,
            failure: None,
        }
    } else if frames > 0 {
        failure(format!(
            "decoded {frames} frames but none in the {name} hardware format"
        ))
    } else {
        failure("hardware decoder produced no frames".to_string())
    }
}
