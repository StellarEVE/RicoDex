# RicoDex

**A local-first desktop video editor for beat-synchronised music videos.**

RicoDex analyses a song and local video footage, generates an editable first cut, and provides a native timeline for refining and exporting the result. The application is written in Rust and currently targets Windows.

RicoDex is an early-stage project, not a finished or production-ready editor. Its focus is a dependable music-video workflow rather than full professional-NLE feature parity.

## Current project status

The essential editing and project-persistence slice is implemented. The current Windows CI workflow has passed workspace tests, a release build, portable-package staging, and Inno Setup installer creation. CI verifies builds and automated tests; it does not replace interactive testing on a real Windows desktop.

The current app export path supports a selected-song soundtrack as stereo AAC alongside H.264 video for a contiguous hard-cut video sequence. The output-level integration test decodes the resulting MP4 and checks audio/video duration agreement and stereo channel separation.

The application remains alpha-stage. The Windows desktop acceptance checklist, broader hardware validation, and FFmpeg/transitive-library redistribution review are still outstanding. Do not treat an Actions artifact as a public release.

## Implemented workflow

### Import and generate

- Import media from local storage. The app does not require an account or upload source media to a cloud service.
- Analyze a selected song and source footage through the staged analysis pipeline.
- Generate a beat-synchronised first cut for timeline refinement.
- The current generator/editor integration protects pinned and manually edited clips from silent replacement. Re-analysis and adding more media are blocked while manual edits exist because planner/manual-edit provenance is not yet persisted.

### Edit the timeline

- Change a clip's source-in point and duration within the source asset's usable range.
- Split a clip at its midpoint.
- Delete a clip with ripple behaviour.
- Drag clips to reorder them.
- Pin clips and use undo/redo.
- Route timeline edits through Core validation, commands, and snapshot history.

The current sequential exporter expects a contiguous cut. Reordering therefore compacts the edited video sequence; it does not create freeform gaps. Source intro/outro trims are validated against each timeline item's source range and become locked once a timeline exists.

### Save projects and recover media

- Save, Save As, and Open project files using versioned JSON persistence.
- Schema-1 project files migrate to schema 2.
- Persist selected-song identity and per-asset intro/outro trims.
- Rebuild the Media pool when opening a project.
- Relink moved media only when its content fingerprint matches the saved asset. If the old project has no saved fingerprint, fingerprint enrollment is allowed only from the original stored path.

A reopened non-empty timeline is conservatively treated as user-edited because generated/manual clip provenance is not yet saved. This protects existing edits, but means media imports and regeneration remain blocked for that reopened timeline.

### Export

- Export an enabled, contiguous video sequence to MP4 using H.264 video and hard cuts.
- When a song is selected, mux it as stereo AAC at 48 kHz, starting at output time zero.
- Fit the soundtrack to the encoded video frame schedule: truncate longer audio or pad shorter audio with silence.
- Without a selected song, the current app export path is video-only.

The selected-song integration test checks that the exported MP4 contains decodable AAC audio, that left and right channels remain distinct, and that audio and video durations agree within the test tolerance.

## Current limitations and work in progress

The following are not yet supported by the app's full sequential export workflow:

- Audio captured from source video clips.
- Timeline audio tracks or arbitrary audio-item placement.
- Per-track or per-clip gain, fades, and loudness normalization.
- Timeline gaps and transitions in the sequential app exporter.
- Consistent frame scheduling and behaviour between interactive preview and final export.
- Cancellation during an active encode and atomic temporary-output finalization.
- Persisted provenance that distinguishes generated clips from manual edits.

The current Windows desktop workflow has not yet been manually smoke-tested across a clean installation and real hardware. Renderer and encoder behaviour on the supported hardware matrix still needs direct validation.

## Workspace

| Crate | Responsibility |
|---|---|
| `ricodex-core` | Project and timeline model, exact tick time, validation, edit commands, undo/redo history, and JSON persistence |
| `ricodex-audio` | Song decoding, stereo audio decoding, and musical structure analysis |
| `ricodex-media` | Media fingerprinting, staged footage analysis, and analysis cache |
| `ricodex-platform` | FFmpeg-backed probing, decoding, encoding, and hardware capability trials |
| `ricodex-engine` | Deterministic cut planning and analysis orchestration |
| `ricodex-render` | CPU reference composition, GPU composition, effects, transitions, sequential export, and render cache |
| `ricodex-app` | Native desktop interface and background-job coordination |
| `ricodex-cli` | CLI executable placeholder; currently prints its name and version only |

The internal `ricodex-*` crate and executable names are retained from the imported foundation. The product identity is RicoDex.

## Build from source

### Requirements

- A stable Rust toolchain.
- FFmpeg 8.1 shared development files compatible with the workspace's `rsmpeg` bindings, including headers, import libraries, and runtime DLLs.
- LLVM/Clang with `libclang` available for bindgen.
- On Windows, use the environment-variable layout shown in [the Windows CI workflow](.github/workflows/ci.yml).

### Windows environment

Set the following variables to the corresponding paths in your FFmpeg and LLVM installations. The FFmpeg `bin` directory must also be on `PATH`.

```powershell
$env:FFMPEG_DLL_PATH = "C:\path\to\ffmpeg\lib"
$env:FFMPEG_INCLUDE_DIR = "C:\path\to\ffmpeg\include"
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$env:Path = "C:\path\to\ffmpeg\bin;$env:Path"
```

Then run the workspace checks and release build:

```powershell
cargo test --workspace
cargo build --release --workspace
```

To launch the desktop application from the workspace:

```powershell
cargo run -p ricodex-app
```

The application defaults to the OpenGL (`glow`) renderer for Windows startup compatibility. To opt in to the wgpu renderer for a test run:

```powershell
$env:RICODEX_RENDERER = "wgpu"
cargo run -p ricodex-app
```

The release desktop executable is named `ricodex-app.exe). The CLI executable is named `ricodex-cli.exe) and is currently a version-printing stub.

## Windows CI and packages

The [Windows CI workflow](.github/workflows/ci.yml) runs for pull requests targeting `main), pushes to `main), and manual workflow dispatches. It:

1. Downloads the FFmpeg LGPL shared development build and configures the Rust bindings.
2. Downloads a separate GPL-enabled FFmpeg executable only to generate H.264 test fixtures. This test-only executable is not the FFmpeg runtime packaged with RicoDex.
3. Audits test configuration and runs `cargo test --workspace).
4. Builds the release workspace.
5. Stages the desktop app, CLI executable, FFmpeg runtime DLLs, Microsoft Visual C++ Redistributable, and notices.
6. Produces a portable ZIP and an Inno Setup installer as workflow artifacts.

Artifacts are retained for 14 days. The workflow does not publish a GitHub Release.

These packages are experimental CI validation artifacts, not approved public redistribution releases. The current packaging uses the FFmpeg LGPL shared variant, but that choice alone does not establish compliance for every transitive dependency. The full DLL licence inventory, corresponding source/build materials, notices, and any applicable redistribution obligations must be reviewed before public binary distribution. See [Licensing and distribution](docs/LICENSING.md).

## Project documents

- [Product plan](docs/PLAN.md)
- [Implementation roadmap and current gaps](docs/ROADMAP.md)
- [Windows desktop acceptance checklist](docs/WINDOWS_SMOKE_TEST.md)
- [Licensing and binary distribution](docs/LICENSING.md)
- [Initial foundation migration notes](RELEASE_NOTES.md)

## License

RicoDex source is intended to be licensed under Apache-2.0; see [LICENSE](LICENSE). This does not resolve the separate licensing obligations for FFmpeg and any optional or transitive libraries bundled with a Windows binary.
