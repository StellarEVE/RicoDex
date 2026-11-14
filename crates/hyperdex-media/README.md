# hyperdex-media

Footage analysis for HyperDex: content fingerprints, stage 0 container
probing, the stage 1 sparse sampling pass (poster, filmstrip, coarse
scenes, black ranges, rough motion), the stage 2 full decode (exact
scenes, black ranges, motion scores, best moments, progress callbacks),
and the fingerprint keyed analysis cache. All codec work lives in
hyperdex-platform; this crate only consumes its plain data frames.

## Building against rsmpeg and FFmpeg

hyperdex-platform links FFmpeg as a library through the rsmpeg crate with
its `link_system_ffmpeg` feature. This workspace pins the generated
FFmpeg 8.1 Windows ABI binding in `vendor/ffmpeg8_1_binding.rs`; the
Windows build needs the matching shared development package (headers,
import libraries and runtime DLLs). A runtime-only installation is not
enough. See `vendor/README.md` for binding provenance and the Windows CI
environment.

Windows Cargo environment (substitute your FFmpeg installation path):

    FFMPEG_DLL_PATH=C:\path\to\ffmpeg\lib
    FFMPEG_INCLUDE_DIR=C:\path\to\ffmpeg\include
    FFMPEG_BINDING_PATH=<workspace>\vendor\ffmpeg8_1_binding.rs
    PATH=<ffmpeg>\bin;%PATH%

What each piece does:

* `FFMPEG_BINDING_PATH` keeps generated struct layouts consistent with
  the FFmpeg 8.1 Windows build instead of relying on host-specific
  bindgen output.

## Tests

The integration tests generate their fixtures with the system ffmpeg
(a moving box segment joined to a solid segment with a hard cut, and
a white, black, white clip), so ffmpeg must be on PATH to run them.
