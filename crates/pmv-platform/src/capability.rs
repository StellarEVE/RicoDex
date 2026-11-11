//! The capability report: what this machine and the linked FFmpeg
//! build can actually do, gathered from the library itself rather
//! than assumed. Hardware acceleration methods are the ones the
//! linked build reports; whether each has a matching decoder is
//! checked against every decoder the build carries. Whether a method
//! works on this hardware is a separate question, answered only by
//! [`crate::trial_decode`].

use rsmpeg::ffi;

/// One hardware acceleration method the linked FFmpeg build knows
/// about, and whether any decoder in the build can use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwAccelMethod {
    pub name: String,
    pub has_decoder: bool,
}

/// A snapshot of this machine's media capabilities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityReport {
    /// Operating system and architecture, for example "linux x86_64".
    pub os: String,
    /// CPU description: the model name where the OS exposes one
    /// (Linux /proc/cpuinfo), else the target architecture, else
    /// "unknown".
    pub cpu: String,
    /// Every hardware acceleration method the linked FFmpeg build
    /// reports, in the build's own enumeration order.
    pub hwaccels: Vec<HwAccelMethod>,
    /// True when a software decoder exists for H.264, the baseline
    /// codec the pipeline must always be able to fall back to.
    pub software_fallback: bool,
}

/// Gather the capability report from the linked FFmpeg build.
pub fn capability_report() -> CapabilityReport {
    let mut hwaccels = Vec::new();
    for device_type in rsmpeg::avutil::hwdevice_iterate_types() {
        if device_type == ffi::AV_HWDEVICE_TYPE_NONE {
            continue;
        }
        let name = rsmpeg::avutil::hwdevice_get_type_name(device_type)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        hwaccels.push(HwAccelMethod {
            has_decoder: any_decoder_supports(device_type),
            name,
        });
    }

    CapabilityReport {
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        cpu: cpu_description(),
        hwaccels,
        software_fallback: rsmpeg::avcodec::AVCodec::find_decoder(ffi::AV_CODEC_ID_H264).is_some(),
    }
}

fn cpu_description() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in info.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    if key.trim() == "model name" {
                        let value = value.trim();
                        if !value.is_empty() {
                            return value.to_string();
                        }
                    }
                }
            }
        }
    }
    std::env::consts::ARCH.to_string()
}

/// Whether any decoder in the linked build carries a hardware
/// configuration for `device_type`.
fn any_decoder_supports(device_type: ffi::AVHWDeviceType) -> bool {
    unsafe {
        let mut opaque: *mut std::ffi::c_void = std::ptr::null_mut();
        loop {
            let codec = ffi::av_codec_iterate(&mut opaque);
            if codec.is_null() {
                return false;
            }
            if ffi::av_codec_is_decoder(codec) == 0 {
                continue;
            }
            let mut index = 0;
            loop {
                let config = ffi::avcodec_get_hw_config(codec, index);
                if config.is_null() {
                    break;
                }
                if (*config).device_type == device_type {
                    return true;
                }
                index += 1;
            }
        }
    }
}
