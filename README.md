# RicoDex

**A local-first Rust desktop video editor that builds music videos around the structure of the song.**

RicoDex analyses a selected song and source videos, generates a beat-synchronised first cut, and provides a native timeline editor for refinement before local export.

- Native Windows desktop application, written in Rust.
- Media is imported from local disks and stays local.
- No accounts, subscriptions, telemetry, cloud processing, media uploads or watermarks.
- The generator proposes an editable first cut. User-edited or pinned clips are protected from silent regeneration.
- Four workspaces: Media, Cut, Edit and Export.

## Current state

This repository starts from the existing Rust foundation in StellarEVE/PMV-Genie, continued under the RicoDex name. It is not yet a completed release. The alpha.9 notes describe the Edit page as selection/inspection-focused, app export as a sequential hard-cut subset, and real Windows hardware validation and FFmpeg redistribution licensing as outstanding.

Do not treat a documented target feature as implemented until code and tests verify it. Read docs/ROADMAP.md and docs/PLAN.md.

## Workspace

| Crate | Responsibility |
|---|---|
| pmv-core | Timeline model, exact tick time, commands, invariants, history and persistence |
| pmv-audio | Song decoding and musical structure analysis |
| pmv-media | Fingerprinting, staged footage analysis and analysis cache |
| pmv-platform | Codec and operating-system media access, decoder/encoder capability trials |
| pmv-render | CPU reference compositor, GPU compositor, effects, transitions and render cache |
| pmv-engine | Generation planner and cancellable analysis/render orchestration |
| pmv-app | Native desktop interface |
| pmv-cli | Command-line front end over the shared engine |

The internal pmv-* crate and executable identifiers are retained from the imported foundation for compatibility. The product identity is RicoDex.

## Build

A stable Rust toolchain and FFmpeg development files required by rsmpeg are needed. On Windows, configure FFMPEG_DLL_PATH, FFMPEG_INCLUDE_DIR and LIBCLANG_PATH as shown in .github/workflows/ci.yml.

    cargo test --workspace
    cargo build --release --workspace

The foundation binaries are currently named pmv-app.exe and pmv-cli.exe.

## CI and distribution

Pushes and pull requests to main run Windows workspace tests and a release build. CI does not prove compatibility on every physical GPU or Windows machine.

Binary distribution is intentionally not enabled yet. The bundled FFmpeg build and redistributable encoder set require a documented licence decision before downloadable packages can be published. See docs/LICENSING.md.

## Documents

- Complete product breakdown: docs/PLAN.md
- Implementation roadmap and current gaps: docs/ROADMAP.md
- Licensing and binary distribution: docs/LICENSING.md
- Release notes: RELEASE_NOTES.md

## License

Source is intended to be licensed under Apache-2.0; see LICENSE. This does not settle the separate licensing obligations of FFmpeg or optional codec libraries bundled with a binary.