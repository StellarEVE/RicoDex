# Licensing and binary distribution decision

HyperDex Studio source is intended to be licensed under Apache-2.0; see the repository's LICENSE.

The Windows CI workflow currently uses the BtbN FFmpeg 8.1 LGPL shared build rather than its GPL shared build. This avoids intentionally selecting the GPL variant and keeps FFmpeg in dynamically linked DLLs. BtbN documents that its LGPL variant omits GPL-only dependencies such as libx264 and libx265. This choice alone does not establish full redistribution compliance.

The workflow creates experimental GitHub Actions artifacts only. It stages FFmpeg runtime DLLs from the LGPL build, includes the LGPL v2.1 license text, records the FFmpeg build information and archive SHA-256, and includes the Microsoft Visual C++ Redistributable installer. It does not create a GitHub Release.

For media integration tests, CI separately downloads a GPL-enabled FFmpeg executable to generate H.264 test fixtures with libx264. The media and platform test helpers use this executable only for fixture generation through the HYPERDEX_TEST_FFMPEG environment variable. That GPL test build is not used as the FFmpeg library linked by HyperDex and is not copied into the staged portable ZIP or installer.

Before public redistribution, the owner must complete and document:
- the exact FFmpeg and transitive dependency inventory, including each DLL's licence and required notices;
- the corresponding source code and build materials for the exact FFmpeg build and any bundled libraries;
- the exact FFmpeg configure options and optional libraries or encoders to exclude;
- the guaranteed software encoding floor on machines without a working hardware encoder;
- any additional notices, source-offer, reverse-engineering, relinking or other obligations applicable to the selected build.

The CI package includes a visible status notice stating that it is an experimental validation artifact and must not be redistributed as a public release until this review is complete. Do not treat the availability of an Actions artifact as a completed legal review. Dynamic linking alone does not settle every FFmpeg and external codec licence obligation.