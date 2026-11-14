# RicoDex development status

**Status date:** 14 November 2026  
**Repository:** https://github.com/StellarEVE/RicoDex  
**Working branch:** `main`  
**Current code head at the time this document was prepared:** `4615e4da46ed26e9d11f00dd6dffa1e666f0d380`  
**Current verification:** Windows CI run [#85](https://github.com/StellarEVE/RicoDex/actions/runs/43560350393) passed the complete Windows tests/build/package pipeline for schema-4 validation. Documentation consolidation commit [#89](https://github.com/StellarEVE/RicoDex/actions/runs/43561776169) also passed the complete pipeline.

This document is the practical engineering snapshot. The product plan describes intended product behaviour; this file describes what is present in code and what is still incomplete. A persisted type or isolated helper is not the same as an end-to-end application feature.

## 1. Product summary

RicoDex is a local-first native desktop editor for making music-synchronised videos. Users import a song and local video footage, run staged analysis, receive an editable first cut, refine the timeline and export a finished MP4. The product is intentionally narrower than a general professional non-linear editor. Media remains on the user's machine.

The workspace is Rust and currently targets Windows. It contains eight crates:

| Crate | Responsibility |
|---|---|
| `ricodex-core` | Project model, exact Tick time, validation, commands, undo/redo and JSON persistence |
| `ricodex-audio` | Audio decoding, tempo/onset analysis, bars, phrases and change points |
| `ricodex-media` | Media fingerprints, staged footage analysis and analysis cache |
| `ricodex-platform` | FFmpeg-backed probe/decode/encode and hardware capability trials |
| `ricodex-engine` | Deterministic cut planning and analysis orchestration |
| `ricodex-render` | Frame scheduling, CPU/GPU composition, effects, transitions, timeline export and render cache |
| `ricodex-app` | Native Media, Cut, Edit and Export pages plus background jobs |
| `ricodex-cli` | Placeholder executable that currently prints its name/version |

## 2. Implemented capabilities

### Project and edit model

- Project data is validated by Core and persisted as readable JSON.
- Time positions and durations use integer ticks at 254,016,000,000 ticks per second.
- Edit commands use clone, validate, commit semantics; accepted edits participate in snapshot undo/redo.
- Save, Save As, Open, backup creation and fingerprint-based media relinking are integrated in the desktop app.
- Per-asset intro/outro trims and selected-song identity are persisted.
- Schema 3 added per-item generated/manual provenance and pin state. Legacy schema-1/2 projects migrate conservatively, protecting existing clips as manual and pinned.
- Schema 4 adds an optional incoming transition to each item. It stores a transition kind, parameters and duration. `SetItemTransition` is a Core command and participates in undo/redo. Validation requires an adjacent previous item on the same video track, a positive duration shorter than both neighbouring clips, and transition regions that do not consume an entire clip.
- Schema-1/2 migration retains the conservative manual/pinned behaviour. Schema-3 migration preserves existing provenance/pin values and initializes new transition metadata to `None`.

### Analysis and planning

- Imported assets are fingerprinted and probed.
- Song decoding/analysis extracts a beat and musical-structure representation.
- Footage analysis uses staged passes and a fingerprint-keyed analysis cache.
- The planner produces deterministic, editable cut plans and supports refinement while protecting manually edited/pinned material.
- The desktop application coordinates analysis and export in background worker threads.

### Timeline rendering and export

- `TimelineFrameSchedule` derives frame timestamps from integer frame indices and rational frame rates. It supports absolute bounded timeline ranges and avoids cumulative floating-point frame-time drift.
- The schedule sampler maps active items to source timestamps and skips hidden tracks, disabled items and muted audio tracks as appropriate for sampling.
- The app-driven renderer exports enabled clips across visible video tracks in project track order. Later tracks are above earlier tracks.
- Each visible video track advances through its own active clip stream. Active frames composite with source-over alpha. A gap or disabled clip on an upper track reveals lower tracks; black is emitted only where no visible video layer contributes.
- Ordinary gaps and intervals occupied by disabled clips are preserved in the rendered timeline. Items on an individual Core track remain non-overlapping.
- The app can mux the selected song as stereo AAC, resampled to 48 kHz and synchronized to the encoded video frame count. The MP4 integration test decodes the output and verifies duration and stereo channel separation.
- The renderer has CPU reference implementations for the named transitions and a GPU compositor for a supported subset. A separate `render_project` path can render exactly two clips with one transition, a request-level effect stack and an optional transition-region cache.
- Windows CI run [#81](https://github.com/StellarEVE/RicoDex/actions/runs/43554149714) passed the workspace tests, release build, runtime staging, portable ZIP, Inno Setup package and artifact upload for the multi-track exporter.

## 3. Current transition status

The transition work is split into two distinct parts:

**Persisted model foundation on main:** schema 4 stores `ItemTransition { kind, params, duration }` on the incoming item at a cut. Core validates adjacency and duration constraints, the command participates in undo/redo, and older schemas migrate without inventing transitions. The latest validation-order correction `67ae474` passed complete Windows CI run [#85](https://github.com/StellarEVE/RicoDex/actions/runs/43560350393).

**Renderer mapping foundation:** `TimelineTransitionMap` provides deterministic integer-Tick output-time compression, source-time lookup for non-transitioning tracks, outgoing/incoming source coordinates and blend progress for the transitioning track, sequential compression accumulation, and cross-track overlap rejection. Unit tests cover a single transition, two sequential transitions, overlapping regions, and hidden/disabled endpoints. This map is not yet used by the app-driven exporter.

**Not yet complete:** the multi-track exporter still renders hard cuts, and the Edit UI has no transition controls. The independent two-clip `render_project` path is not a substitute for full-timeline transition rendering.

## 4. Explicitly incomplete

The following remain planned or incomplete. Do not describe them as supported end-to-end:

- Full-timeline transition rendering, including integration of the transition map, outgoing/incoming frame decoding, multi-track layer composition and MP4 verification.
- Transition editing controls on the desktop Edit page.
- Preview playback driven by the same timeline schedule and renderer as export.
- Timeline audio items and arbitrary audio-track placement.
- Audio captured from source video clips.
- Mixing of multiple audio sources, per-clip/per-track gain, fades and loudness normalization.
- Per-clip and track-level full-timeline effects.
- Shared preview/export cache keys and complete cache invalidation.
- Atomic temporary-file finalization for exports.
- True cancellation during active decoding, rendering and encoding.
- Physical Windows acceptance on clean machines and a representative hardware matrix.
- FFmpeg and transitive-library licensing review before public binary redistribution.

The current Edit UI remains a small single-video-track editing surface. The renderer can consume multi-track project data, but the application does not yet provide a complete multitrack editing workflow.

## 5. Verification and release status

- Multi-track exporter: verified by Windows CI run #81, including workspace tests and packaging.
- Schema-4 transition model: passed the complete Windows CI pipeline in run #85.
- Transition timeline mapping foundation: unit tests are added in this implementation slice; the corresponding Windows CI run is its verification gate.
- Previous CI runs #83 and #84 failed during the first transition-model test pass. Run #83 exposed a stale test type name; run #84 exposed validation error-ordering in the transition-limit test. The stale test type was corrected, and commit `67ae474` moves transition-region aggregate validation after individual transition validation. The run #85 workspace-test step now passes; the final workflow conclusion is still required.
- Windows CI builds and packages the application, but it does not establish clean-machine interactive acceptance or hardware performance.
- The Windows smoke-test checklist is `docs/WINDOWS_SMOKE_TEST.md`.
- The CI artifact is a validation package, not an approved public release. See `docs/LICENSING.md`.

## 6. Recommended next implementation sequence

1. Integrate `TimelineTransitionMap` into the multi-track frame schedule and renderer.
2. Add dedicated outgoing/incoming frame readers for the transition track; do not ask one stateful track decoder to seek to two source times in one output frame.
3. Render the transition as a single track contribution, then composite that result into the stack so lower tracks remain visible beneath transparent transition output and higher tracks remain above it.
4. Add integration tests for a two-clip dissolve, a lower track underneath the transition, two sequential non-overlapping transitions, hidden/disabled clips, output frame-count reduction and selected-song synchronization.
5. Add the Edit-page transition control through `Command::SetItemTransition`, with undo/redo and clear invalid-state feedback.
6. Only after transitions pass, begin timeline audio-item placement and mixing.

Avoid beginning preview parity or cache redesign until the timeline transition interpretation is stable. Do not introduce simultaneous transition overlaps across different video tracks without an explicit compositing and timeline-mapping design.

## 7. Engineering constraints

- Work directly on `main`; do not create a feature branch or PR unless the owner changes that instruction.
- Make focused commits and verify each meaningful slice with Windows GitHub Actions.
- Do not claim a feature or commit passed until its exact CI run has completed successfully.
- Keep Core as the source of truth. UI edits go through Core commands and history.
- Use integer `Tick` values for all timeline positions and durations.
- Keep codec/FFmpeg access in `ricodex-platform`.
- Keep CPU composition as the correctness reference for GPU results.
- Update `README.md`, `docs/ROADMAP.md` and this status document when capabilities or priorities materially change.
- Include the exact commit SHA and CI run link in progress handoffs.
