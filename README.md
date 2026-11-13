# RicoDex

**A local-first Rust desktop video editor that builds music videos around the structure of the song.**

RicoDex analyses a selected song and source videos, generates a beat-synchronised first cut, and provides a native timeline editor for refinement before local export.

- Native Windows desktop application, written in Rust.
- Media is imported from local disks and stays local.
- No accounts, subscriptions, telemetry, cloud processing, media uploads or watermarks.
- The generator proposes an editable first cut. User-edited or pinned clips are protected from silent regeneration.
- Four workspaces: Media, Cut, Edit and Export.

## Current state

RicoDex is a local-first Rust desktop video editor built on a modular workspace. It is under active development and is not yet a completed release. The current editor slice includes source-in/duration trimming, midpoint splitting, ripple deletion, drag-to-reorder, pinning and undo/redo, all routed through Core commands and history. Reordering keeps the current hard-cut export sequence contiguous. Planner offsets are converted back to absolute source-media time, and Core validation keeps clip ranges inside each asset's usable intro/outro-trimmed window. Source trim settings lock once a timeline exists. Project Save, Save As and Open are integrated with versioned Core JSON persistence; schema-1 projects migrate to schema 2. Selected-song identity and per-source trims are persisted. Moved media can be relinked only after its content fingerprint matches; if no fingerprint was saved, only the original stored path can be used to establish one. A reopened non-empty timeline is conservatively treated as edited, so re-analysis and adding media are blocked until planner/manual-edit provenance is persisted. The latest changes are awaiting complete Windows CI validation and a manual desktop smoke test. The sequential hard-cut app export now includes the selected song as a stereo AAC soundtrack, resampled to 48 kHz and fitted to the video run duration. Clip-source audio, timeline audio-item placement, volume/fades, gaps, transitions, preview/export consistency, real Windows hardware validation and FFmpeg redistribution licensing remain outstanding.

Do not treat a documented target feature as implemented until code and tests verify it. Read docs/ROADMAP.md and docs/PLAN.md.

## Workspace

| Crate | Responsibility |
|---|---|
| ricodex-core | Timeline model, exact tick time, commands, invariants, history and persistence |
| ricodex-audio | Song decoding and musical structure analysis |
| ricodex-media | Fingerprinting, staged footage analysis and analysis cache |
| ricodex-platform | Codec and operating-system media access, decoder/encoder capability trials |
| ricodex-render | CPU reference compositor, GPU compositor, effects, transitions and render cache |
| ricodex-engine | Generation planner and cancellable analysis/render orchestration |
| ricodex-app | Native desktop interface |
| ricodex-cli | Command-line front end over the shared engine |

The internal ricodex-* crate and executable identifiers are retained from the imported foundation for compatibility. The product identity is RicoDex.

## Build

A stable Rust toolchain and FFmpeg development files required by rsmpeg are needed. On Windows, configure FFMPEG_DLL_PATH, FFMPEG_INCLUDE_DIR and LIBCLANG_PATH as shown in .github/workflows/ci.yml.

    cargo test --workspace
    cargo build --release --workspace

The foundation binaries are currently named ricodex-app.exe and ricodex-cli.exe. On Windows, the desktop app defaults to the OpenGL (glow) renderer for compatibility. The wgpu renderer can be selected explicitly by setting `RICODEX_RENDERER=wgpu` before launch.

## CI and distribution

Pushes, pull requests and manual workflow dispatches run Windows workspace tests and a release build. The workflow stages the FFmpeg runtime DLLs, includes the Microsoft Visual C++ Redistributable installer, and produces a portable ZIP plus an Inno Setup installer as GitHub Actions artifacts. It does not create a GitHub Release. Workflow artifacts are retained for 14 days.

These are experimental CI validation packages, not approved public redistribution releases. The LGPL FFmpeg variant is used instead of the GPL variant, but the transitive dependency licence inventory and corresponding-source obligations still require review before public redistribution. See docs/LICENSING.md.

## Documents

- Complete product breakdown: docs/PLAN.md
- Implementation roadmap and current gaps: docs/ROADMAP.md
- Windows manual acceptance checklist: docs/WINDOWS_SMOKE_TEST.md
- Licensing and binary distribution: docs/LICENSING.md
- Release notes: RELEASE_NOTES.md

## License

Source is intended to be licensed under Apache-2.0; see LICENSE. This does not settle the separate licensing obligations of FFmpeg or optional codec libraries bundled with a binary.