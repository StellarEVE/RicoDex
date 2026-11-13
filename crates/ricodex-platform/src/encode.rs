//! Video and audio encoding over rsmpeg. The Platform layer owns all
//! FFmpeg access and exposes plain PCM/RGB buffers to higher crates.
//! Video-only exports remain supported; the synchronized export path
//! adds an AAC stereo stream to the same MP4 muxer.

use std::ffi::CString;
use std::path::Path;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::ffi;
use rsmpeg::swscale::SwsContext;

use crate::error::PlatformError;

const AUDIO_SAMPLE_RATE: u32 = 48_000;

fn encode_error(reason: impl Into<String>) -> PlatformError {
    PlatformError::EncodeFailed {
        reason: reason.into(),
    }
}

/// State for the AAC stream sharing the video output muxer.
struct AudioEncoderState {
    encoder: AVCodecContext,
    encoder_name: String,
    stream_index: usize,
    encoder_time_base: ffi::AVRational,
    stream_time_base: ffi::AVRational,
    frame_size: usize,
    samples_encoded: u64,
}

fn create_audio_stream(
    output: &mut AVFormatContextOutput,
    bitrate_kbps: u32,
) -> Result<AudioEncoderState, PlatformError> {
    let codec = AVCodec::find_encoder(ffi::AV_CODEC_ID_AAC)
        .ok_or_else(|| encode_error("no AAC audio encoder in this FFmpeg build"))?;
    let encoder_name = codec.name().to_string_lossy().into_owned();
    let mut encoder = AVCodecContext::new(&codec);
    encoder.set_sample_rate(AUDIO_SAMPLE_RATE as i32);
    encoder.set_sample_fmt(ffi::AV_SAMPLE_FMT_FLTP);
    encoder.set_ch_layout(ffi::AV_CHANNEL_LAYOUT_STEREO);
    encoder.set_time_base(ffi::AVRational {
        num: 1,
        den: AUDIO_SAMPLE_RATE as i32,
    });
    encoder.set_bit_rate(bitrate_kbps.max(64) as i64 * 1_000);
    if output.oformat().flags & ffi::AVFMT_GLOBALHEADER as i32 != 0 {
        encoder.set_flags(encoder.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    }
    encoder.open(None).map_err(|e| {
        encode_error(format!("cannot open AAC encoder {encoder_name}: {e}"))
    })?;

    // AAC's native encoder uses fixed-size planar-float frames. Read
    // its resolved frame size rather than assuming a codec constant.
    let frame_size = unsafe { (*encoder.as_ptr()).frame_size.max(1) as usize };
    let encoder_time_base = ffi::AVRational {
        num: 1,
        den: AUDIO_SAMPLE_RATE as i32,
    };
    let stream_index = output.streams().len();
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
                "cannot copy AAC parameters to the output stream",
            ));
        }
    }

    Ok(AudioEncoderState {
        encoder,
        encoder_name,
        stream_index,
        encoder_time_base,
        stream_time_base: encoder_time_base,
        frame_size,
        samples_encoded: 0,
    })
}

fn drain_audio_packets(
    output: &mut AVFormatContextOutput,
    audio: &mut AudioEncoderState,
) -> Result<(), PlatformError> {
    loop {
        match audio.encoder.receive_packet() {
            Ok(mut packet) => {
                packet.set_stream_index(audio.stream_index as i32);
                if packet.duration <= 0 {
                    unsafe {
                        (*packet.as_mut_ptr()).duration = audio.frame_size as i64;
                    }
                }
                packet.rescale_ts(audio.encoder_time_base, audio.stream_time_base);
                output
                    .interleaved_write_frame(&mut packet)
                    .map_err(|e| encode_error(format!("cannot write AAC packet: {e}")))?;
            }
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderDrainError) => {
                return Ok(())
            }
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {
                return Ok(())
            }
            Err(e) => return Err(encode_error(format!("cannot receive AAC packet: {e}"))),
        }
    }
}

fn encode_audio_frame(
    output: &mut AVFormatContextOutput,
    audio: &mut AudioEncoderState,
    interleaved_stereo: &[f32],
) -> Result<(), PlatformError> {
    let frame_size = audio.frame_size;
    if interleaved_stereo.len() != frame_size * 2 {
        return Err(encode_error(format!(
            "AAC frame has {} samples, expected {}",
            interleaved_stereo.len(),
            frame_size * 2
        )));
    }

    let mut frame = AVFrame::new();
    frame.set_nb_samples(frame_size as i32);
    frame.set_format(ffi::AV_SAMPLE_FMT_FLTP);
    frame.set_ch_layout(ffi::AV_CHANNEL_LAYOUT_STEREO);
    frame.set_sample_rate(AUDIO_SAMPLE_RATE as i32);
    frame
        .alloc_buffer()
        .map_err(|e| encode_error(format!("cannot allocate AAC frame: {e}")))?;

    let left_ptr = frame.data_mut()[0] as *mut f32;
    let right_ptr = frame.data_mut()[1] as *mut f32;
    if left_ptr.is_null() || right_ptr.is_null() {
        return Err(encode_error("AAC frame did not allocate both stereo planes"));
    }
    for sample in 0..frame_size {
        let left = interleaved_stereo[sample * 2];
        let right = interleaved_stereo[sample * 2 + 1];
        let left = if left.is_finite() { left.clamp(-1.0, 1.0) } else { 0.0 };
        let right = if right.is_finite() { right.clamp(-1.0, 1.0) } else { 0.0 };
        unsafe {
            *left_ptr.add(sample) = left;
            *right_ptr.add(sample) = right;
        }
    }

    frame.set_pts(audio.samples_encoded as i64);
    audio.samples_encoded += frame_size as u64;
    match audio.encoder.send_frame(Some(&frame)) {
        Ok(()) => {}
        Err(e) if matches!(e, rsmpeg::error::RsmpegError::SendFrameAgainError) => {
            drain_audio_packets(output, audio)?;
            audio
                .encoder
                .send_frame(Some(&frame))
                .map_err(|e| encode_error(format!("cannot send AAC frame: {e}")))?;
        }
        Err(e) => return Err(encode_error(format!("cannot send AAC frame: {e}"))),
    }
    drain_audio_packets(output, audio)
}

/// A video file opened for encoding. When created with
/// VideoEncoder::create_with_audio, it also owns a stereo AAC
/// encoder and muxes both streams into one MP4 file.
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
    audio: Option<AudioEncoderState>,
    /// Incomplete AAC input frame, interleaved L/R. Kept between calls
    /// because video frame boundaries rarely coincide with AAC frames.
    audio_pending: Vec<f32>,
    finished: bool,
}

impl VideoEncoder {
    /// Create a video-only MP4 encoder. RGB24 input frames must be
    /// exactly width by height.
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        bitrate_kbps: u32,
    ) -> Result<VideoEncoder, PlatformError> {
        Self::create_internal(path, width, height, fps_num, fps_den, bitrate_kbps, None)
    }

    /// Create an MP4 encoder with H.264 video and stereo AAC audio.
    /// Audio input is interleaved stereo f32 at 48 kHz and may be
    /// supplied in arbitrary even-length chunks through
    /// encode_stereo_audio.
    pub fn create_with_audio(
        path: &Path,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        video_bitrate_kbps: u32,
        audio_bitrate_kbps: u32,
    ) -> Result<VideoEncoder, PlatformError> {
        Self::create_internal(
            path,
            width,
            height,
            fps_num,
            fps_den,
            video_bitrate_kbps,
            Some(audio_bitrate_kbps),
        )
    }

    fn create_internal(
        path: &Path,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        bitrate_kbps: u32,
        audio_bitrate_kbps: Option<u32>,
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
        encoder.set_max_b_frames(0);
        if output.oformat().flags & ffi::AVFMT_GLOBALHEADER as i32 != 0 {
            encoder.set_flags(encoder.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
        }
        encoder
            .open(None)
            .map_err(|e| encode_error(format!("cannot open encoder {encoder_name}: {e}")))?;

        let encoder_time_base = ffi::AVRational {
            num: fps_den as i32,
            den: fps_num as i32,
        };
        let video_stream_index = output.streams().len();
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

        // All streams must exist before write_header. The MP4 muxer
        // may rewrite their time bases, which are read back below.
        let mut audio = match audio_bitrate_kbps {
            Some(bitrate) => Some(create_audio_stream(&mut output, bitrate)?),
            None => None,
        };

        output
            .write_header(&mut None)
            .map_err(|e| encode_error(format!("cannot write header: {e}")))?;
        let stream_time_base = output.streams()[video_stream_index].time_base;
        if let Some(audio) = audio.as_mut() {
            audio.stream_time_base = output.streams()[audio.stream_index].time_base;
        }

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
            audio,
            audio_pending: Vec::new(),
            finished: false,
        })
    }

    /// The name of the video encoder this build actually resolved to.
    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    /// The AAC encoder name, when an audio stream was requested.
    pub fn audio_encoder_name(&self) -> Option<&str> {
        self.audio.as_ref().map(|audio| audio.encoder_name.as_str())
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Encode one RGB24 frame, tightly packed rows of width * 3 bytes.
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

    /// Queue interleaved stereo f32 samples at 48 kHz. Chunks may end
    /// anywhere; complete AAC frames are sent immediately and a partial
    /// frame is retained until the next call or finish.
    pub fn encode_stereo_audio(&mut self, samples: &[f32]) -> Result<(), PlatformError> {
        if self.finished {
            return Err(encode_error("encoder already finished"));
        }
        if self.audio.is_none() {
            return Err(encode_error("audio was not enabled for this encoder"));
        }
        if samples.len() % 2 != 0 {
            return Err(encode_error("stereo audio input must contain L/R sample pairs"));
        }
        if samples.is_empty() {
            return Ok(());
        }

        self.audio_pending.extend_from_slice(samples);
        let frame_size = self.audio.as_ref().unwrap().frame_size;
        let block_len = frame_size * 2;
        while self.audio_pending.len() >= block_len {
            let block: Vec<f32> = self.audio_pending.drain(..block_len).collect();
            let (output, audio) = (&mut self.output, self.audio.as_mut().unwrap());
            encode_audio_frame(output, audio, &block)?;
        }
        Ok(())
    }

    fn drain_packets(&mut self) -> Result<(), PlatformError> {
        loop {
            match self.encoder.receive_packet() {
                Ok(mut packet) => {
                    packet.set_stream_index(0);
                    if packet.duration <= 0 {
                        unsafe {
                            (*packet.as_mut_ptr()).duration = 1;
                        }
                    }
                    packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
                    self.output
                        .interleaved_write_frame(&mut packet)
                        .map_err(|e| encode_error(format!("cannot write video packet: {e}")))?;
                }
                Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderDrainError) => {
                    return Ok(())
                }
                Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {
                    return Ok(())
                }
                Err(e) => return Err(encode_error(format!("cannot receive video packet: {e}"))),
            }
        }
    }

    fn flush_audio(&mut self) -> Result<(), PlatformError> {
        if self.audio.is_none() {
            return Ok(());
        }

        if !self.audio_pending.is_empty() {
            let frame_size = self.audio.as_ref().unwrap().frame_size;
            let mut padded = vec![0.0f32; frame_size * 2];
            padded[..self.audio_pending.len()].copy_from_slice(&self.audio_pending);
            self.audio_pending.clear();
            let (output, audio) = (&mut self.output, self.audio.as_mut().unwrap());
            encode_audio_frame(output, audio, &padded)?;
        }

        let (output, audio) = (&mut self.output, self.audio.as_mut().unwrap());
        match audio.encoder.send_frame(None) {
            Ok(()) => {}
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {}
            Err(e) => return Err(encode_error(format!("cannot flush AAC encoder: {e}"))),
        }
        drain_audio_packets(output, audio)
    }

    /// Flush both encoders and write the trailer. Consumes the encoder;
    /// the file is complete only after this returns Ok.
    pub fn finish(mut self) -> Result<(), PlatformError> {
        self.flush_audio()?;
        match self.encoder.send_frame(None) {
            Ok(()) => {}
            Err(e) if matches!(e, rsmpeg::error::RsmpegError::EncoderFlushedError) => {}
            Err(e) => return Err(encode_error(format!("cannot flush video encoder: {e}"))),
        }
        self.drain_packets()?;
        self.output
            .write_trailer()
            .map_err(|e| encode_error(format!("cannot write trailer: {e}")))?;
        self.finished = true;
        Ok(())
    }
}
