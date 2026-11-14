# RicoDex implementation roadmap

This document is the execution plan for RicoDex. It distinguishes verified implementation from intended behaviour. A feature is not complete because a type, shader, isolated test or button exists; it is complete when the user can perform the workflow end to end and the result is verified.

## Product direction

RicoDex is a local-first, music-intelligent video editor. Its defining workflow is:

**Import media → understand the song and footage → generate an editable first cut → refine it on a timeline → export a synchronized video.**

RicoDex is not intended to reproduce the entire feature surface of DaVinci Resolve or another professional NLE. It should make this narrower workflow dependable, fast, approachable and unusually good.

## Existing foundation

The workspace contains eight Rust crates with explicit responsibility boundaries:

- **Core:** project and timeline model, exact tick time, validation, edit commands, snapshot undo/redo and JSON persistence.
- **Audio:** in-process song decoding, onset/tempo detection, bars, phrases and change points.
- **Media:** file fingerprinting, staged footage analysis and analysis cache.
- **Platform:** FFmpeg-backed probing, decoding, encoding and hardware capability trials.
- **Engine:** deterministic cut planning and staged analysis orchestration.
- **Render:** rational timeline frame scheduling, timeline-layer sampling, CPU/GPU composition, effects, transitions, sequential export and transition-cache foundations.
- **App:** native four-page desktop interface and background job coordination.
- **CLI:** currently a version-printing stub, not a functional editing interface.

### Current verified implementation checkpoint (14 November 2026)

The latest fully verified end-to-end export implementation is commit 3bdd237 (Render visible video tracks in stacking order). Windows CI run #81 passed workspace tests, release build, runtime dependency staging, portable ZIP creation, Windows installer creation and artifact upload. This confirms the software build and package workflow, not physical desktop acceptance or hardware compatibility.

The current main branch has since added a persisted transition model in schema 4. The latest validation-order correction is commit 67ae474; Windows CI run #85 is currently validating that change. Do not treat the schema-4 transition model as fully verified or as rendered in app exports until the current CI run passes and renderer integration is complete.

- A shared rational TimelineFrameSchedule calculates frame timestamps from an integer frame index and rational frame rate. It supports bounded absolute-time ranges and has tests for long-duration timestamp stability.
- TimelineFrameSchedule::layers_at samples active enabled items across project tracks, maps each layer to source time, and skips hidden tracks and muted audio tracks.
- The app-driven exporter renders enabled clips across every visible video track in project track order, with later tracks above earlier tracks. It composites active frames with source-over alpha. A gap or disabled clip on an upper track reveals lower tracks; black is emitted only when no visible video layer is active. MP4 integration tests cover stacking order, hidden tracks, disabled upper clips, gaps and the selected-song soundtrack.
- The exporter preserves ordinary gaps and disabled-clip intervals on each track. Same-track items remain non-overlapping in the Core model.
- The selected-song soundtrack is muxed as stereo AAC, resampled to 48 kHz and synchronized to the output frame count. Tests decode the resulting MP4 and check audio duration and stereo channel separation.
- Core schema 4 adds an optional incoming transition to each timeline item. The transition stores its kind, parameters and duration, is set through an undoable Core command, and is validated against adjacent items on the same video track. Schemas 1–3 migrate without inventing transitions. Schema-1/2 provenance and pin protection remain preserved; schema-3 provenance and pin state are retained by the schema-4 migration.
- The transition model is currently a persistence and validation foundation only. The existing render_project path still handles exactly two video items with one transition, a request-level effect stack and an optional transition-region render cache. It is not integrated into app-driven multi-track timeline export, and the main Edit UI has no transition editing control yet.
- Preview playback using the same scheduler/compositor, full-timeline transition rendering, timeline audio-item placement, clip-source audio, gain/fades, normalization, full-timeline effects, atomic output finalization and true mid-render cancellation remain incomplete.

Other existing application foundations:

- The Edit page supports source/duration trimming, splitting, ripple deletion, drag-to-reorder, pinning and undo/redo through Core commands and History. It remains a deliberately small single-track editing surface. The current reorder workflow compacts items into a contiguous cut; there is not yet a freeform gap or transition editing workflow in the UI.
- Planner analysis uses coordinates relative to the user-selected source-trim window; installed timeline items are converted back to absolute original-media time. Core validation rejects items outside the usable source window, and source trim settings lock once a timeline exists.
- Application-level Save, Save As and Open are integrated with versioned Core JSON persistence. Schema 3 added per-clip generated/manual provenance and pin state; schema 4 adds optional per-item incoming transition metadata. Schema-1/2 timelines migrate conservatively with existing clips marked manual and pinned; schema 3 keeps its provenance and pin state when migrating to schema 4. Media relinking verifies saved fingerprints; a project without a fingerprint can enroll one only from its original saved path, not from an arbitrary moved replacement. Pinned clips are carried into the next planner refinement; unpinned generated slots can be reconsidered after more media is imported.
- The render cache currently supports transition-region reuse in the narrow two-clip render path. End-to-end cache sharing and invalidation for preview and full-timeline export remain outstanding.
- Windows hardware behaviour must be verified on actual machines. The application defaults to OpenGL for startup compatibility; the wgpu renderer is an explicit opt-in until broader Windows validation exists.
- FFmpeg and bundled transitive-library licensing must be reviewed before public binary distribution.

## Delivery phases

### Phase 0: Reliable development and distribution baseline

**Goal:** every code change is built and tested on Windows, and validation packages behave predictably.

- Keep Windows workspace tests and release builds green. The current CI workflow passes workspace tests, release builds and package staging (verified by run #76).
- Verify the default OpenGL startup path on real Windows hardware.
- Verify project save/load and recovery behaviour.
- Validate FFmpeg decoding and encoding on reference machines.
- Test the portable package and installer on a clean Windows installation.
- Complete the FFmpeg and transitive-library license inventory before public binary distribution.

**Exit gate:** green Windows CI, a clean-machine launch, and a documented import/analyze/save/reopen/export smoke test. CI alone does not prove hardware compatibility.

### Phase 1: Complete the essential editing loop

**Goal:** users can refine and preserve a generated cut without bypassing the Core model.

- Route every edit through ricodex-core commands and History.
- Current Edit-page controls support source-in/duration trimming, midpoint splitting, ripple deletion, drag/reorder movement, pinning, undo and redo. Reordering compacts the video track into a contiguous cut. The UI does not yet provide freeform gap or transition editing, even though the export renderer can preserve gaps already represented in a project.
- Preserve integer Tick values in the model; convert to seconds only at UI boundaries.
- Make the current project model the timeline's source of truth after every accepted command.
- Prevent invalid edits from partially mutating project state.
- Preserve generated-clip pinning and clearly report rejected edits.
- Provide application-level Save, Save As and Open controls backed by Core's atomic JSON persistence and backup file.
- Persist selected-song identity and per-asset intro/outro trims in project schema 2; schema 3 additionally persists clip provenance and pin state; schema 4 adds optional incoming transition metadata to items. Migrate schema-1/2 timelines conservatively, preserving old clips as manual and pinned when their origin is unknown.
- Keep item source positions in absolute original-media coordinates. Convert the planner's trimmed-window offsets at plan installation, validate every item against the asset's usable trimmed range, and lock source-trim controls after a timeline exists.
- Rebuild the Media pool when reopening a project and verify every available source against its saved content fingerprint. Restore clip provenance and pins from the project. Existing manual/pinned clips are kept during refinement, while unpinned generated clips can be reconsidered after more media is imported.
- Provide a Locate flow for missing/moved files. Accept a moved replacement only when its fingerprint matches the saved asset. When no fingerprint was saved, allow fingerprint enrollment from the original stored path only; never silently adopt a different file.
- Add acceptance tests for schema migration, metadata round trips, editing history and fingerprint-matched relinking.

**Exit gate:** a user can generate a cut, trim/split/delete/reorder clips, undo/redo, save and reopen it with song selection and source trims intact, and recover moved media without substituting different footage. The actual desktop workflow must be smoke-tested on Windows; CI alone is not sufficient.

### Phase 2: One complete preview and export pipeline

**Goal:** preview and final output use the same timeline interpretation.

Verified foundations:

- A rational frame schedule is shared by the sequential exporter and the timeline sampling API. Frame times are calculated from integer frame indices and a rational frame rate rather than accumulated floating-point frame durations.
- The app-driven exporter handles multiple clips across visible video tracks, hard cuts, ordinary gaps and gaps left by disabled clips. Track order is bottom-to-top; upper-track gaps reveal lower tracks, and uncovered frames are encoded as black. Windows CI run #81 passed the multi-track MP4 integration tests and full packaging pipeline.
- The selected-song soundtrack can be encoded as stereo AAC alongside the video, with output-level tests for duration and channel separation.
- The two-item transition/effect rendering path and transition-region cache exist, but are not yet part of full-timeline app export. Schema 4 now persists an incoming transition choice on an item with Core validation and an undoable command; its latest validation correction is awaiting Windows CI run #85.

Remaining work, in implementation order:

1. **Full-timeline transitions.** The persisted transition metadata, backward-compatible schema-4 migration, validation and undoable command are implemented on main; the latest validation-order correction is being checked by CI run #85. Still integrate transition rendering into the multi-track timeline path, define exact output-time compression semantics, and reject ambiguous overlapping transition regions. The Edit UI must eventually expose this metadata through Core commands.
2. **Timeline audio mixing.** Add placed audio items and clip-source audio, then support synchronized mixing, gain, fades and normalization. Preserve the existing selected-song bed as one supported audio source.
3. **Preview/export parity.** Drive preview playback and final export from the same schedule, source-time mapping, track composition, transition and audio rules.
4. **Full-timeline effects.** Apply per-clip and track-level effects through the same pipeline used by preview and export.
5. **Export finalization and cancellation.** Write to a temporary destination and atomically finalize on success; implement true cancellation during decoding/rendering/encoding, accurate progress and useful errors.

Keep the first supported output matrix deliberately small and testable, such as MP4 with H.264 video and AAC audio.

**Completed implementation stage: Phase 2.1, multi-track video composition.** The app-driven exporter now composites all visible video tracks in project order. Windows CI run #81 passed workspace tests and the complete packaging pipeline.

**Current implementation stage: Phase 2.2, full-timeline transitions.**

The first data-model slice is on main: an incoming clip can persist a transition kind, parameters and duration; Core validates the adjacent cut and duration constraints; `SetItemTransition` participates in undo/redo; and schema 1–3 projects migrate with no transition silently added. Windows CI run #85 is checking the latest validation correction. The remaining implementation is to define output-time compression semantics, integrate the CPU transition compositor into the multi-track exporter, then add UI controls. Acceptance tests should cover a two-clip dissolve, a transition with a lower video track underneath, multiple sequential transitions where their regions do not overlap, disabled/hidden-track behaviour, duration/frame-count reduction, and soundtrack synchronization. Reject transition regions that overlap in project time until simultaneous cross-track transitions have an explicit compositing model.

**Phase 2 exit gate:** a representative multi-track project with gaps, transitions and synchronized audio previews and exports with matching timing and content.

### Phase 3: Render scheduling and cache correctness

**Goal:** interactive editing remains responsive and unchanged content is not needlessly recomputed.

- Add a priority-aware render scheduler for visible preview, near-playhead frames and background work.
- Extend content-addressed cache keys to cover every pixel-affecting input in the full-timeline renderer.
- Invalidate only cache entries affected by an edit.
- Share cache and render logic between preview and export; current cache support is limited to transition regions in the narrow two-item renderer.
- Bound memory, disk usage and concurrent decode/render work.
- Test cancellation and stale-result rejection.

**Exit gate:** repeated preview of unchanged content reuses valid cache entries, edits invalidate the correct regions, and background rendering never blocks UI interaction.

### Phase 4: Hardware compatibility and performance

**Goal:** hardware accelerates RicoDex when it works, but is never required for correctness.

- Keep the CPU compositor as the correctness reference and fallback.
- Probe hardware decoding and encoding through real capability trials, not vendor assumptions.
- Keep OpenGL as the proven application-startup default until wgpu startup is validated across the supported hardware matrix.
- Add proxies/optimized media and memory-efficient frame buffering.
- Test integrated graphics, low-end discrete GPUs, dual-GPU laptops and CPU-only fallback.
- Record adapter, renderer, decoder and encoder decisions in diagnostics.

**Exit gate:** a documented hardware matrix shows correct output on every supported fallback path and measured performance on reference machines.

### Phase 5: Music intelligence as the product differentiator

**Goal:** generated cuts respond to musical structure and footage quality, not only a uniform beat grid.

- Improve verse/chorus/drop and section-change detection.
- Use beat strength and section energy to vary cut density.
- Improve shot diversity and avoid repeated or visually unsuitable footage.
- Match motion and scene characteristics to musical passages.
- Add user-facing generation styles such as slow, balanced, energetic and beat-heavy.
- Maintain a repeatable evaluation set of songs and footage; measure timing, variety, black-frame avoidance and user corrections.

**Exit gate:** measured improvements across a fixed evaluation set and user controls that predictably change generation behaviour.

### Phase 6: Product polish and extensibility

**Goal:** make the core workflow efficient and maintainable before exposing broad extension points.

- Finish UI design tokens, keyboard shortcuts and timeline navigation.
- Add reusable presets and project templates where they improve the core workflow.
- Define stable scripting and automation interfaces over the existing command model.
- Consider a plugin API only after render, media and project interfaces are stable.
- Improve diagnostics, accessibility and onboarding.

**Exit gate:** the core workflow is stable enough that extensions do not require breaking internal interfaces.

## Explicit non-goals for the current product

Do not spend early milestones attempting full professional-NLE parity. Defer node-based grading, a Fusion-scale compositor, multicamera editing, a DAW replacement, cloud collaboration, user accounts and a general-purpose plugin marketplace. Reopen these decisions only when they clearly support RicoDex's primary workflow.

## Engineering rules

1. The Core project model is the source of truth. UI widgets do not mutate project data directly.
2. Every user edit is a validated command and participates in undo/redo.
3. Edit positions and durations remain integer Tick values. Floating-point seconds are only for UI and external boundaries.
4. Codec and operating-system media access stays in Platform.
5. CPU composition remains the correctness oracle for GPU output.
6. Long-running work uses cancellable jobs, bounded resources and real progress reporting.
7. Do not claim support without end-to-end tests and accurate UI status.
8. Software-rendered CI is not proof of hardware performance or clean-machine Windows compatibility.

## Current release gates

- [x] Multi-track export commit passes Windows workspace tests, release build, runtime dependency staging, portable ZIP and installer packaging (CI run #81).
- [ ] Schema-4 incoming-transition model and validation-order correction pass Windows CI (run #85 currently in progress).
- [x] Windows package staging, portable ZIP and installer creation pass for the verified multi-track export commit (CI run #81).
- [ ] Application launches on a clean Windows machine using the default renderer.
- [ ] Import, analyze, edit, save, reopen and export smoke test passes.
- [ ] FFmpeg and bundled transitive-library licensing review is complete before public binary distribution.
