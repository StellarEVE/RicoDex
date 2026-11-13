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
- **Render:** CPU reference composition, GPU composition, effects, transitions and render-cache foundations.
- **App:** native four-page desktop interface and background job coordination.
- **CLI:** currently a version-printing stub, not a functional editing interface.

The foundation is real, but several user-facing workflows remain incomplete:

- The Edit page now supports source/duration trimming, splitting, ripple deletion, drag-to-reorder, pinning and undo/redo through Core commands and History. It remains a deliberately small single-track editing surface.
- Planner analysis uses coordinates relative to the user-selected source-trim window; installed timeline items are converted back to absolute original-media time. Core validation rejects items outside the usable source window, and source trim settings lock once a timeline exists.
- The app export path renders enabled video items as a sequential contiguous hard-cut run and now muxes the selected song as stereo AAC. The soundtrack starts at output time zero, is resampled to 48 kHz, and is truncated or silence-padded to the video run duration. Output-level tests decode the MP4 back and check audio duration and stereo channel separation. This is only the selected-song bed: clip-source audio, timeline audio-item placement, gain/fades and normalization are not integrated. Gaps, transitions and unified preview/export timing also remain outstanding.
- Application-level Save, Save As and Open are integrated with versioned Core JSON persistence. Media relinking verifies saved fingerprints; a project without a fingerprint can enroll one only from its original saved path, not from an arbitrary moved replacement. The complete Windows interaction workflow still needs a manual smoke test.
- Render scheduling and cache foundations need end-to-end integration and invalidation tests.
- Windows hardware behaviour must be verified on actual machines. The application defaults to OpenGL for startup compatibility; the wgpu renderer is an explicit opt-in until broader Windows validation exists.
- FFmpeg and bundled transitive-library licensing must be reviewed before public binary distribution.

## Delivery phases

### Phase 0: Reliable development and distribution baseline

**Goal:** every code change is built and tested on Windows, and validation packages behave predictably.

- Keep Windows workspace tests and release builds green.
- Verify the default OpenGL startup path on real Windows hardware.
- Verify project save/load and recovery behaviour.
- Validate FFmpeg decoding and encoding on reference machines.
- Test the portable package and installer on a clean Windows installation.
- Complete the FFmpeg and transitive-library license inventory before public binary distribution.

**Exit gate:** green Windows CI, a clean-machine launch, and a documented import/analyze/save/reopen/export smoke test. CI alone does not prove hardware compatibility.

### Phase 1: Complete the essential editing loop

**Goal:** users can refine and preserve a generated cut without bypassing the Core model.

- Route every edit through ricodex-core commands and History.
- Current Edit-page controls support source-in/duration trimming, midpoint splitting, ripple deletion, drag/reorder movement, pinning, undo and redo. Reordering compacts the video track into a contiguous cut to match the current sequential hard-cut exporter; it does not provide freeform gaps or transition editing.
- Preserve integer Tick values in the model; convert to seconds only at UI boundaries.
- Make the current project model the timeline's source of truth after every accepted command.
- Prevent invalid edits from partially mutating project state.
- Preserve generated-clip pinning and clearly report rejected edits.
- Provide application-level Save, Save As and Open controls backed by Core's atomic JSON persistence and backup file.
- Persist selected-song identity and per-asset intro/outro trims in project schema 2; migrate schema-1 projects with safe defaults and infer a selected song only when exactly one audio asset exists.
- Keep item source positions in absolute original-media coordinates. Convert the planner's trimmed-window offsets at plan installation, validate every item against the asset's usable trimmed range, and lock source-trim controls after a timeline exists.
- Rebuild the Media pool when reopening a project and verify every available source against its saved content fingerprint. A reopened non-empty timeline is conservatively treated as user-edited because generated/manual clip provenance is not yet persisted; adding media and regenerating the cut are blocked to protect existing edits.
- Provide a Locate flow for missing/moved files. Accept a moved replacement only when its fingerprint matches the saved asset. When no fingerprint was saved, allow fingerprint enrollment from the original stored path only; never silently adopt a different file.
- Add acceptance tests for schema migration, metadata round trips, editing history and fingerprint-matched relinking.

**Exit gate:** a user can generate a cut, trim/split/delete/reorder clips, undo/redo, save and reopen it with song selection and source trims intact, and recover moved media without substituting different footage. The actual desktop workflow must be smoke-tested on Windows; CI alone is not sufficient.

### Phase 2: One complete preview and export pipeline

**Goal:** preview and final output use the same timeline interpretation.

- Render multiple clips and tracks in timeline order.
- Support gaps, clip enable/disable and basic transitions.
- Add synchronized timeline audio and clip-audio mixing.
- Unify preview and final-export frame scheduling.
- Integrate effects and transitions into the full-timeline renderer.
- Use temporary output files and atomic finalization.
- Support cancellation, accurate progress and useful error reporting.
- Begin with a deliberately small, well-tested export matrix, such as MP4 with H.264 video and AAC audio.

**Exit gate:** a representative multi-clip project with gaps, transitions and audio previews and exports with matching timing and content.

### Phase 3: Render scheduling and cache correctness

**Goal:** interactive editing remains responsive and unchanged content is not needlessly recomputed.

- Add a priority-aware render scheduler for visible preview, near-playhead frames and background work.
- Use content-addressed cache keys that include every pixel-affecting input.
- Invalidate only cache entries affected by an edit.
- Share cache and render logic between preview and export.
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

- [ ] Current main commit passes Windows workspace tests.
- [ ] Windows release build and package staging pass.
- [ ] Application launches on a clean Windows machine using the default renderer.
- [ ] Import, analyze, edit, save, reopen and export smoke test passes.
- [ ] FFmpeg and bundled transitive-library licensing review is complete before public binary distribution.
