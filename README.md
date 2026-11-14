# HyperDex Studio

**A local-first desktop video editor for beat-synchronised music videos.**

HyperDex analyses a song and local video footage, generates an editable first cut, and provides a native timeline for refining and exporting the result. The application is written in Rust and currently targets Windows.

HyperDex is an early-stage project, not a finished or production-ready editor. Its focus is a dependable music-video workflow rather than full professional-NLE feature parity.

## Current project status

HyperDex has a working local-first analysis, editing, project-persistence and export foundation. The latest verified renderer milestone includes transition-aware multi-track video composition and stereo mixing of placed audio-track items with the selected-song bed. Windows CI run #95 passed workspace tests, release build and Windows packaging for the renderer-side audio mixer. The Edit page supports transition controls and audio-track placement with undo/redo. However, the desktop export worker still calls the selected-song-only renderer entry point and does not pass decoded audio-track items to the mixer; placed audio items are therefore not yet audible in exports produced by the app. Windows CI runs #96–#99 failed before any workflow step started (runner ID 0, no steps and no job logs), so the renamed workspace still needs a successful full Windows run. CI does not replace interactive testing on a real Windows desktop.

The current main branch contains the persisted schema-4 transition model, a transition-aware multi-track exporter, Edit-page transition controls and a renderer API for mixing placed audio items. Core enforces Audio/Video asset compatibility, and track creation is an undoable Core command. The shared integer-Tick transition map compresses each outgoing/incoming source region into one output interval, maps other tracks through the same clock, accumulates sequential transitions and rejects overlapping cross-track regions. The renderer can mix placed audio items on the compressed timeline while the selected-song bed remains at normal output speed. However, `hyperdex-app::jobs::run_export` currently calls `render_timeline_with_audio`, which does not pass placed audio assets. The app-level audio-track export integration remains unfinished; run #95 verifies the renderer-side mixer and full Windows packaging, not that missing app wiring. Windows CI runs #96–#99 failed before any workflow steps ran, and provide no evidence of a source compilation failure.

The Windows desktop acceptance checklist, broader hardware validation, and FFmpeg/transitive-library redistribution review remain outstanding. Do not treat an Actions artifact as a public release.

## Implemented workflow

### Import and generate

- Import media from local storage. The app does not require an account or upload source media to a cloud service.
- Analyze a selected song and source footage through the staged analysis pipeline.
- Generate a beat-synchronised first cut for timeline refinement.
- Clip provenance and pin state are persisted per item. User edits are marked manual and pinned; pin changes are undoable and saved. Existing schema-1/2 timelines migrate conservatively with clips protected as manual/pinned. New media can be imported after reopening, and analysis refines only unpinned provisional/generated slots while carrying pinned clips through the planner.

### Edit the timeline

- Change a clip's source-in point and duration within the source asset's usable range.
- Split a clip at its midpoint.
- Delete a clip with ripple behaviour.
- Drag clips to reorder them.
- Pin clips and use undo/redo.
- Set, change and clear incoming transitions using the Edit inspector. Transition type and duration edits use Core commands and support undo/redo.
- Create audio tracks, append imported audio files as timeline items, mute tracks and remove items. These placements are saved through Core command history. The renderer API can mix decoded audio items into stereo AAC alongside the optional selected-song bed. The desktop export worker is not yet wired to supply those decoded assets, so app-export audio-track playback remains incomplete.
- Core supports validated, undoable audio-track creation and rejects audio/video asset mismatches. In the renderer API, placed audio is mixed only from visible, unmuted tracks and enabled items; the selected-song bed remains at normal output speed.
- Route timeline edits through Core validation, commands, and snapshot history.

The Edit-page reorder workflow compacts the edited track into a contiguous sequence; the underlying multi-track renderer can preserve gaps already present in project data. Source intro/outro trims are validated against each timeline item's source range and become locked once a timeline exists.

### Save projects and recover media

- Save, Save As, and Open project files using versioned JSON persistence.
- Schema-1 projects migrate through schema 2 defaults to schema 3; schema-1 and schema-2 timelines migrate with legacy clips protected as manual and pinned.
- Persist selected-song identity, per-asset intro/outro trims, and per-clip provenance/pin state.
- Rebuild the Media pool when opening a project.
- Relink moved media only when its content fingerprint matches the saved asset. If the old project has no saved fingerprint, fingerprint enrollment is allowed only from the original stored path.

Schema-1 and schema-2 projects predate persisted provenance. Their existing clips are therefore migrated as manual and pinned to avoid accidental changes. Users can explicitly unpin clips to let the planner reconsider them.

### Export

- Export enabled clips across visible video tracks to MP4 using H.264 video, hard cuts and persisted incoming transitions. Tracks composite in project order, with later tracks above earlier tracks.
- Each active transition consumes its duration from the combined timeline: the outgoing tail and incoming head form one transition region, shortening the output by that duration. The shared Tick-based map keeps other tracks synchronized and accumulates sequential, non-overlapping transitions.
- The selected-song stereo AAC soundtrack follows the compressed output frame schedule, remaining at normal playback speed and being truncated or padded to the final video duration.
- Preserve timeline gaps. A gap or disabled clip on an upper video track reveals active lower tracks; areas with no active visible video layer render black.
- When a song is selected, mux it as stereo AAC at 48 kHz, starting at output time zero.
- Fit the soundtrack to the encoded video frame schedule: truncate longer audio or pad shorter audio with silence.
- Without a selected song, the current app export path is video-only.

The selected-song integration test checks that the exported MP4 contains decodable AAC audio, that left and right channels remain distinct, and that audio and video durations agree within the test tolerance.

## Current limitations and work in progress

The following are not yet supported by the app's full sequential export workflow:

- Audio captured from source video clips.
- Per-item/track gain, fades, normalization and a dedicated limiter.
- Per-track or per-clip gain, fades, and loudness normalization.
- Interactive preview is not yet driven by the same schedule and renderer as export. Source-video audio, gain/fades, normalization, and a dedicated limiter also remain incomplete.
- Consistent frame scheduling and behaviour between interactive preview and final export.
- Cancellation during an active encode and atomic temporary-output finalization.
- Manual clips can be explicitly unpinned to allow planner refinement; older projects migrate conservatively and require deliberate unpinning where regeneration is desired.

The current Windows desktop workflow has not yet been manually smoke-tested across a clean installation and real hardware. Renderer and encoder behaviour on the supported hardware matrix still needs direct validation.

## Workspace

| Crate | Responsibility |
|---|---|
| `hyperdex-core` | Project and timeline model, exact tick time, validation, edit commands, undo/redo history, and JSON persistence |
| `hyperdex-audio` | Song decoding, stereo audio decoding, and musical structure analysis |
| `hyperdex-media` | Media fingerprinting, staged footage analysis, and analysis cache |
| `hyperdex-platform` | FFmpeg-backed probing, decoding, encoding, and hardware capability trials |
| `hyperdex-engine` | Deterministic cut planning and analysis orchestration |
| `hyperdex-render` | CPU reference composition, GPU composition, effects, transitions, sequential export, and render cache |
| `hyperdex-app` | Native desktop interface and background-job coordination |
| `hyperdex-cli` | CLI executable placeholder; currently prints its name and version only |

The internal crates and binaries use the `hyperdex-*` naming convention. The full product identity is HyperDex Studio.

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
cargo run -p hyperdex-app
```

The application defaults to the OpenGL (`glow`) renderer for Windows startup compatibility. To opt in to the wgpu renderer for a test run:

```powershell
$env:HYPERDEX_RENDERER = "wgpu"
cargo run -p hyperdex-app
```

The release desktop executable is named `hyperdex-app.exe` and is branded HyperDex Studio. The CLI executable is named `hyperdex-cli.exe` and is currently a version-printing stub.

## Windows CI and packages

The [Windows CI workflow](.github/workflows/ci.yml) runs for pull requests targeting `main`, pushes to `main`, and manual workflow dispatches. It:

1. Downloads the FFmpeg LGPL shared development build and configures the Rust bindings.
2. Downloads a separate GPL-enabled FFmpeg executable only to generate H.264 test fixtures. This test-only executable is not the FFmpeg runtime packaged with HyperDex.
3. Audits test configuration and runs `cargo test --workspace`.
4. Builds the release workspace.
5. Stages the desktop app, CLI executable, FFmpeg runtime DLLs, Microsoft Visual C++ Redistributable, and notices.
6. Produces a portable ZIP and an Inno Setup installer as workflow artifacts.

Artifacts are retained for 14 days. The workflow does not publish a GitHub Release.

These packages are experimental CI validation artifacts, not approved public redistribution releases. The current packaging uses the FFmpeg LGPL shared variant, but that choice alone does not establish compliance for every transitive dependency. The full DLL licence inventory, corresponding source/build materials, notices, and any applicable redistribution obligations must be reviewed before public binary distribution. See [Licensing and distribution](docs/LICENSING.md).

## Project documents

- [Product plan](docs/PLAN.md)
- [Implementation roadmap, current gaps and development-session instructions](docs/ROADMAP.md)
- [Development status and verified capabilities](docs/DEVELOPMENT_STATUS.md)
- [Windows desktop acceptance checklist](docs/WINDOWS_SMOKE_TEST.md)
- [Licensing and binary distribution](docs/LICENSING.md)
- [Initial foundation migration notes](RELEASE_NOTES.md)

## License

HyperDex source is intended to be licensed under Apache-2.0; see [LICENSE](LICENSE). This does not resolve the separate licensing obligations for FFmpeg and any optional or transitive libraries bundled with a Windows binary.
