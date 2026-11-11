//! Video encoding over rsmpeg: the write side that pairs with
//! [`crate::VideoDecoder`]. One video stream, H.264 in an MP4
//! container, fed with tightly packed RGB24 frames in presentation
//! order and written with interleaved packets. Like the decoder, this
//! module hands the rest of the workspace plain data only: bytes in,
//! a file out, and the encoder's name reported as a string so callers
//! can record what actually encoded their video.
//!
//! Encoder selection: the encoder is found by name first, "h264",
//! which is what a build carrying only FFmpeg's native H.264 encoder
//! answers to. When no encoder carries that exact name, the first
//! encoder registered for the H.264 codec id is used instead: on the
//! development machine's FFmpeg 8.1.2 that resolves to libx264,
//! because that build registers no plain "h264" encoder. The chosen
//! name is available through [`VideoEncoder::encoder_name`] either
//! way, so the choice is never silent.

use std::ffi::CString;
use std::path::Path;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::ffi;
use rsmpeg::swscale::SwsContext;

use crate::error::PlatformError;

fn encode_error(reason: impl Into<String>) -> PlatformError {
    PlatformError::EncodeFailed {
        reason: reason.into(),
    }
}

/// A video file opened for encoding: fixed geometry, frame rate and
/// bitrate, frames pushed in presentation order.
pub struct VideoEncoder {
    output: AVFormatContextOutput,
    encoder: AVCodecContext,
    encoder_name: String,
    scaler: SwsContext,
    yuv_frame: AVFrame,
    encoder_time_base: ffi::AVRational,
    stream_time_base: ffi::AVRational,
    width: u32,
    height: u32,
    frames_written: u64,
    finished: bool,
}

impl VideoEncoder {
    /// Create an MP4 encoder writing to `path`. Frames pushed later
    /// must be exactly `width` by `height` RGB24.
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        bitrate_kbps: u32,
    ) -> Result<VideoEncoder, PlatformError> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(encode_error(format!(
                "H.264 with 4:2:0 chroma needs even, nonzero dimensions, got {width}x{height}"
            )));
        }
        if fps_num == 0 || fps_den == 0 {
            return Err(encode_error("frame rate must be a positive rational"));
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| PlatformError::InvalidPath {
                path: path.to_path_buf(),
            })?;
        let cpath = CString::new(path_str).map_err(|_| PlatformError::InvalidPath {
            path: path.to_path_buf(),
        })?;

        // Find the H.264 encoder: by name first, then by codec id (see
        // the module docs for why both).
        let name = CString::new("h264").unwrap();
        let codec = AVCodec::find_encoder_by_name(&name)
            .or_else(|| AVCodec::find_encoder(ffi::AV_CODEC_ID_H264))
            .ok_or_else(|| encode_error("no H.264 encoder in this FFmpeg build"))?;
        let encoder_name = codec.name().to_string_lossy().into_owned();

        let mut output = AVFormatContextOutput::create(&cpath).map_err(|e| {
            encode_error(format!("cannot create output {}: {e}", path.display()))
        })?;

        let mut encoder = AVCodecContext::new(&codec);
        encoder.set_width(width as i32);
        encoder.set_height(height as i32);
        encoder.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
        // One tick per frame: the encoder time base is the frame
        // duration, so frame indices are timestamps.
        encoder.set_time_base(ffi::AVRational {
            num: fps_den as i32,
            den: fps_num as i32,
        });
        encoder.set_framerate(ffi::AVRational {
            num: fps_num as i32,
            den: fps_den as i32,
        });
        encoder.set_bit_rate(bitrate_kbps as i64 * 1_000);
        encoder.set_gop_size((fps_num / fps_den.max(1)).max(1) as i32);
        // No B frames: every H.264 encoder accepts that, and packet
        // order stays identical to frame order.
        encoder.set_max_b_frames(0);
        if output.oformat().flags & ffi::AVFMT_GLOBALHEADER as i32 != 0 {
            encoder.set_flags(encoder.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
        }
        encoder.open(None).map_err(|e| {
            encode_error(format!("cannot open encoder {encoder_name}: {e}"))
        })?;

        let encoder_time_base = ffi::AVRational {
            num: fps_den as i32,
            den: fps_num as i32,
        };
        {
            let mut stream = output.new_stream();
            let result = unsafe {
                (*stream.as_mut_ptr()).time_base = encoder_time_base;
                ffi::avcodec_parameters_from_context(
                    (*stream.as_mut_ptr()).codecpar,
                    encoder.as_mut_ptr(),
                )
            };
            if result < 0 {
                return Err(encode_error(
                    "cannot copy encoder parameters to the output stream",
                ));
            }
        }
        output
            .write_header(&mut None)
            .map_err(|e| encode_error(format!("cannot write header: {e}")))?;
        // The muxer may replace the stream time base while writing
        // the header (the MP4 muxer does, with its own timescale), so
        // the value packets must be rescaled into is the one the
        // stream carries now, read back after the header.
        let stream_time_base = output.streams()[0].time_base;

        let scaler = SwsContext::get_context(
            width as i32,
            height as i32,
            ffi::AV_PIX_FMT_RGB24,
            width as i32,
            height as i32,
            ffi::AV_PIX_FMT_YUV420P,
            ffi::SWS_BILINEAR as ffi::SwsFlags,
            None,
            None,
            None,
        )
        .ok_or_else(|| encode_error("cannot create swscale context"))?;

        let mut yuv_frame = AVFrame::new();
        yuv_frame.set_width(width as i32);
        yuv_frame.set_height(height as i32);
        yuv_frame.set_format(ffi::AV_PIX_FMT_YUV420P);
        yuv_frame
            .alloc_buffer()
            .map_err(|e| encode_error(format!("cannot allocate frame: {e}")))?;

        Ok(VideoEncoder {
            output,
            encoder,
            encoder_name,
            scaler,
            yuv_frame,
            encoder_time_base,
            stream_time_base,
            width,
            height,
            frames_written: 0,
            finished: false,
        })
    }

    /// The name of the encoder this build actually resolved to.
    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Encode one RGB24 frame, tightly packed rows of
    /// `width * 3` bytes, `width` by `height` as passed to
    /// [`VideoEncoder::create`].
    pub fn encode_rgb24(&mut self, pixels: &[u8]) -> Result<(), PlatformError> {
        if self.finished {
            return Err(encode_error("encoder already finished"));
        }
        let expected = self.width as usize * self.height as usize * 3;
        if pixels.len() != expected {
            return Err(encode_error(format!(
                "frame has {} bytes, expected {expected}",
                pixels.len()
            )));
        }

        // Wrap the caller's bytes in a source frame without copying
        // the planes one by one: swscale reads through the frame's
        // data pointers and line sizes, which we point at the buffer.
        let mut src = AVFrame::new();
        src.set_width(self.width as i32);
        src.set_height(self.height as i32);
        src.set_format(ffi::AV_PIX_FMT_RGB24);
        unsafe {
            (*src.as_mut_ptr()).data[0] = pixels.as_ptr() as *mut u8;
            (*src.as_mut_ptr()).linesize[0] = (self.width * 3) as i32;
        }
        let scale_result =
            self.scaler
                .scale_frame(&src, 0, self.height as i32, &mut self.yuv_frame);
        // The source frame must not free the caller's buffer when it
        // drops: clear the plane pointer we lent it before anything
        // else can fail.
        unsafe {
            (*src.as_mut_ptr()).data[0] = std::ptr::null_mut();
        }
        scale_result.map_err(|e| encode_error(format!("cannot convert frame to YUV: {e}")))?;

        unsafe {
            (*self.yuv_frame.as_mut_ptr()).pts = self.frames_written as i64;
        }
        match self.encoder.send_frame(Some(&self.yuv_frame)) {
            Ok(()) => {}
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::SendFrameAgainError) => {
                self.drain_packets()?;
                self.encoder
                    .send_frame(Some(&self.yuv_frame))
                    .map_err(|e| encode_error(format!("cannot send frame: {e}")))?;
            }
            Err(e) => return Err(encode_error(format!("cannot send frame: {e}"))),
        }
        self.frames_written += 1;
        self.drain_packets()
    }

    fn drain_packets(&mut self) -> Result<(), PlatformError> {
        loop {
            match self.encoder.receive_packet() {
                Ok(mut packet) => {
                    packet.set_stream_index(0);
                    // Packets drained from the flush can arrive with
                    // no duration, and a durationless final packet
                    // makes the muxer end the track one frame early
                    // (the file then reports a duration one frame
                    // short and players drop the last frame). Every
                    // packet here is exactly one frame, so say so.
                    if packet.duration <= 0 {
                        unsafe {
                            (*packet.as_mut_ptr()).duration = 1;
                        }
                    }
                    packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
                    self.output
                        .interleaved_write_frame(&mut packet)
                        .map_err(|e| encode_error(format!("cannot write packet: {e}")))?;
                }
                Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderDrainError) => {
                    return Ok(())
                }
                Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {
                    return Ok(())
                }
                Err(e) => return Err(encode_error(format!("cannot receive packet: {e}"))),
            }
        }
    }

    /// Flush the encoder and write the trailer. Consumes the encoder;
    /// the file is complete only after this returns Ok.
    pub fn finish(mut self) -> Result<(), PlatformError> {
        // SAFETY of the borrow pattern: send_frame(None) is the
        // documented flush. RsmpegError::EncoderFlushedError on a
        // repeat send is treated as already flushed.
        match self.encoder.send_frame(None) {
            Ok(()) => {}
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {}
            Err(e) => return Err(encode_error(format!("cannot flush encoder: {e}"))),
        }
        self.drain_packets()?;
        self.output
            .write_trailer()
            .map_err(|e| encode_error(format!("cannot write trailer: {e}")))?;
        self.finished = true;
        Ok(())
    }
}
