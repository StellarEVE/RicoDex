# Vendored FFmpeg compatibility files

`ffmpeg8_1_binding.rs` is generated Rust FFI output from
[`CCExtractor/rusty_ffmpeg`](https://github.com/CCExtractor/rusty_ffmpeg),
commit `95cb0ee8b43475511b2455808215d599b4c28991` (upstream message:
“Update tracked binding to FFmpeg@8.1 and rust-bindgen@0.72.1”). The
binding is kept in the repository so Windows builds use the same
FFmpeg 8.1 ABI layouts instead of host-specific bindgen output.

`rsmpeg/` is the MIT-licensed `rsmpeg 0.18.0+ffmpeg.8.0` source with one
small compatibility change in `src/avcodec/parser.rs`: cast `AVCodecID`
to the C API's declared `int` on MSVC as well as other targets. The
upstream code only performed this conversion off MSVC, which failed to
compile with the pinned binding's unsigned enum alias.

Keep the binding and the FFmpeg 8.1 development libraries together. Do
not regenerate the binding with an arbitrary host toolchain; update it
only when changing the supported FFmpeg ABI, then run the complete
Windows workspace and packaging checks.
