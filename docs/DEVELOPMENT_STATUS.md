# HyperDex Studio development status

**Status date:** 14 November 2026  
**Repository:** https://github.com/StellarEVE/HyperDex  
**Working branch:** `main`  
**Base commit for the HyperDex Studio rename:** `b26245777ae5dae13d66d3ba5add93f6fb7ace9a`  
**Current verification:** schema-4 validation passed the complete Windows CI pipeline in run [#85](https://github.com/StellarEVE/HyperDex/actions/runs/43560350393). Documentation consolidation passed the complete pipeline in run [#89](https://github.com/StellarEVE/HyperDex/actions/runs/43561776169). Transition mapping passed the complete Windows pipeline in run [#90](https://github.com/StellarEVE/HyperDex/actions/runs/43564641742). Exporter integration passed the complete Windows CI pipeline in run [#91](https://github.com/StellarEVE/HyperDex/actions/runs/43565688966). Edit-page transition controls passed the complete workflow in run [#92](https://github.com/StellarEVE/HyperDex/actions/runs/43566269122). Core audio-track foundation passed the complete workflow in run [#93](https://github.com/StellarEVE/HyperDex/actions/runs/43567127799). Audio placement run [#94](https://github.com/StellarEVE/HyperDex/actions/runs/43567806957) passed workspace tests, release compilation, runtime staging and portable ZIP creation; installer compilation was active at the latest check. Audio mixer and app-export integration run [#95](https://github.com/StellarEVE/HyperDex/actions/runs/43568365543) passed workspace tests and was compiling release binaries. The HyperDex Studio rename commit triggers a fresh full Windows workflow.

This document is the practical engineering snapshot. The product plan describes intended product behaviour; this file describes what is present in code and what is still incomplete. A persisted type or isolated helper is not the same as an end-to-end application feature.

## 1. Product summary

HyperDex Studio is a local-first native desktop editor for making music-synchronised videos. Users import a song and local video footage, run staged analysis, receive an editable first cut, refine the timeline and export a finished MP4. The product is intentionally narrower than a general professional non-linear editor. Media remains on the user's machine.

The workspace is Rust and currently targets Windows. It contains eight crates:

| Crate | Responsibility |
|---|---|
| `hyperdex-core` | Project model, exact Tick time, validation, commands, undo/redo and JSON persistence |
| `hyperdex-audio` | Audio decoding, tempo/onset analysis, bars, phrases and change points |
| `hyperdex-media` | Media fingerprints, staged footage analysis and analysis cache |
| `hyperdex-platform` | FFmpeg-backed probe/decode/encode and hardware capability trials |
| `hyperdex-engine` | Deterministic cut planning and analysis orchestration |
| `hyperdex-render` | Frame scheduling, CPU/GPU composition, effects, transitions, timeline export and render cache |
| `hyperdex-app` | Native Media, Cut, Edit and Export pages plus background jobs |
| `hyperdex-cli` | Placeholder executable that currently prints its name/version |

## 2. Implemented capabilities

### Project and edit model

- Project data is validated by Core and persisted as readable JSON.
- Core validates track and asset kinds: Audio tracks contain Audio assets; Video tracks contain Video assets.
- Track creation uses `Command::AddTrack`, creates its item group atomically and participates in undo/redo.
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
- Windows CI run [#81](https://github.com/StellarEVE/HyperDex/actions/runs/43554149714) passed the workspace tests, release build, runtime staging, portable ZIP, Inno Setup package and artifact upload for the multi-track exporter.

## 3. Current transition status

The transition work is split into two distinct parts:

**Persisted model foundation on main:** schema 4 stores `ItemTransition { kind, params, duration }` on the incoming item at a cut. Core validates adjacency and duration constraints, the command participates in undo/redo, and older schemas migrate without inventing transitions. The latest validation-order correction `67ae474` passed complete Windows CI run [#85](https://github.com/StellarEVE/HyperDex/actions/runs/43560350393).

**Full-timeline rendering:** `TimelineTransitionMap` is consumed by the app-driven multi-track MP4 exporter. It compresses the combined outgoing/incoming region by exactly the persisted transition duration, maps non-transitioning tracks through the same compressed output clock, accumulates sequential compression, and rejects ambiguous overlapping regions. Dedicated outgoing/incoming readers supply the CPU compositor; the blended result is inserted at the transition track's normal stack position, with lower tracks underneath and upper tracks above. MP4 tests cover one dissolve, a transition with an upper video overlay, two sequential transitions, output frame-count reduction and selected-song stereo AAC duration/channel synchronization.

**Edit-page audio controls:** the UI supports audio track creation, append of a probed audio asset, mute/unmute and non-ripple removal. The AppState placement helpers validate the target track and audio asset, derive source range from trims, append after the last clip and use Core command history. Tests cover sequential items, source/timeline coordinates, undo, invalid types and probe readiness. Windows CI for this UI slice is the current workflow and must complete before claiming it is verified.

**Edit-page transition controls:** the inspector now allows selecting one of the supported transition kinds, changing its duration and clearing it. It applies `Command::SetItemTransition`, so Core validation, manual/pin provenance and History remain authoritative. AppState tests cover set, change, clear, undo and redo. This UI slice awaits Windows CI run #92.

**Remaining transition verification:** exporter MP4 integration tests passed in Windows run [#91](https://github.com/StellarEVE/HyperDex/actions/runs/43565688966). Mapping run [#90](https://github.com/StellarEVE/HyperDex/actions/runs/43564641742) also passed the full pipeline. The independent two-clip `render_project` path remains a separate narrower renderer.

## 4. Explicitly incomplete

The following remain planned or incomplete. Do not describe them as supported end-to-end:

- Preview playback driven by the same timeline schedule and renderer as export.
- Audio-item placement is exposed in Edit: users can create audio tracks, append probed audio assets, mute tracks and remove audio items. The renderer now mixes decoded audio-item PCM into AAC, but the app export worker is not yet connected to decode and pass those assets.
- Audio captured from source video clips.
- Mixing of multiple audio sources, per-clip/per-track gain, fades and loudness normalization.
- Per-clip and track-level full-timeline effects.
- Shared preview/export cache keys and complete cache invalidation.
- Atomic temporary-file finalization for exports.
- True cancellation during active decoding, rendering and encoding.
- Physical Windows acceptance on clean machines and a representative hardware matrix.
- FFmpeg and transitive-library licensing review before public binary redistribution.

The HyperDex Studio Edit UI has a video clip editor plus a separate audio-track placement list. Audio items can be appended, muted and removed, but freeform audio drag placement and audible audio-track mixing are not yet implemented.

## 5. Verification and release status

- Multi-track exporter: verified by Windows CI run #81, including workspace tests and packaging.
- Schema-4 transition model: passed the complete Windows CI pipeline in run #85.
- Full-timeline transition export: MP4 integration tests are committed; Windows run #91 workspace tests passed and release compilation was running at the latest check.
- Edit-page transition controls: Core command integration and AppState tests are committed; Windows run #92 is the verification gate.
- Previous CI runs #83 and #84 failed during the first transition-model test pass. Run #83 exposed a stale test type name; run #84 exposed validation error-ordering in the transition-limit test. The stale test type was corrected, and commit `67ae474` moves transition-region aggregate validation after individual transition validation. The complete validation workflow passed in run #85.
- Windows CI builds and packages the application, but it does not establish clean-machine interactive acceptance or hardware performance.
- The Windows smoke-test checklist is `docs/WINDOWS_SMOKE_TEST.md`.
- The CI artifact is a validation package, not an approved public release. See `docs/LICENSING.md`.

## 6. Recommended next implementation sequence

1. Complete Windows CI runs #92 and #93, diagnosing any failing test or packaging step before treating Phase 2.2 and the Core audio model as verified.
2. Integrate decoded stereo audio-item mixing into MP4 export, honoring hidden tracks, muted tracks and disabled items; verify the resulting AAC stream by decoding the exported MP4.
3. Continue with source-video audio, gain/fades and normalization only after placed-audio mixing is end-to-end verified.

Avoid beginning preview parity or cache redesign until the timeline transition interpretation is stable. Do not introduce simultaneous transition overlaps across different video tracks without an explicit compositing and timeline-mapping design.

## 7. Engineering constraints

- Work directly on `main`; do not create a feature branch or PR unless the owner changes that instruction.
- Make focused commits and verify each meaningful slice with Windows GitHub Actions.
- Do not claim a feature or commit passed until its exact CI run has completed successfully.
- Keep Core as the source of truth. UI edits go through Core commands and history.
- Use integer `Tick` values for all timeline positions and durations.
- Keep codec/FFmpeg access in `hyperdex-platform`.
- Keep CPU composition as the correctness reference for GPU results.
- Update `README.md`, `docs/ROADMAP.md` and this status document when capabilities or priorities materially change.
- Include the exact commit SHA and CI run link in progress handoffs.
