# HyperDex Studio implementation roadmap

This document is the execution plan and canonical continuation guide for HyperDex. It distinguishes verified implementation from intended behaviour. A feature is not complete because a type, shader, isolated test or button exists; it is complete when the user can perform the workflow end to end and the result is verified.

## Instructions for future development sessions

This roadmap is the canonical continuation document for both contributors and new ChatGPT development sessions. The standalone `docs/NEW_CHAT_HANDOFF.md` has been retired. A new session must use this repository and this roadmap rather than relying on a prompt or status copied from an earlier conversation.

### Repository entry point and required reading

Use **HyperDex** in ordinary prose and `hyperdex` for crate, package and code identifiers. Reserve **HyperDex Studio** for document headings, titles and explicit product-name or branding contexts.


Repository: https://github.com/StellarEVE/HyperDex

Before changing code, read these documents from the current `main` branch:

1. `README.md`
2. `docs/DEVELOPMENT_STATUS.md`
3. `docs/ROADMAP.md` (this document)
4. `docs/PLAN.md`
5. `docs/WINDOWS_SMOKE_TEST.md`
6. `docs/LICENSING.md`

### Naming convention

Use **HyperDex** in ordinary prose and `hyperdex` for crate, package and code identifiers. Reserve **HyperDex Studio** for document headings, titles and explicit product-name or branding contexts.

Treat the live repository as authoritative. First inspect the current `main` head, recent commits, the current state of relevant Windows GitHub Actions runs, and the current source files for the task. Do not assume that commit hashes, workflow states, roadmap priorities, or implementation descriptions in an earlier conversation are still current. Reconcile discrepancies against code and completed workflow results before proceeding.

### Required Git and CI workflow

Work directly on `main`. Do not create a feature branch or pull request unless the owner changes that instruction. **Do not push after every small implementation slice.** Keep in-progress work together and push only when a substantial, reviewable amount of the codebase is ready or an entire stage/milestone is complete. Prefer one milestone push over a stream of small commits.

The milestone push is the Windows validation checkpoint:

1. Inspect the current `main` head, recent history, relevant source/tests and current workflow state before editing.
2. Implement a cohesive milestone. Add or update unit, integration and end-to-end tests appropriate to the behaviour. For media/render work, prefer deterministic fixtures and decode/check the resulting MP4.
3. Before pushing, run all available local formatting, lint and test checks. Review the complete diff and update affected documentation in the same batch.
4. Push the milestone as one logical commit batch. Do not push every internal fix or intermediate documentation adjustment separately.
5. Wait for the complete Windows CI run for that pushed state. Inspect all failed steps and collect the full set of failures before starting corrections. Do not rerun CI after each individual failure.
6. Fix all identified root causes together in the working tree, run local checks again, and push one consolidated corrective batch. Use another CI run to verify that batch. Never hide, bypass, weaken or relabel a failing test to make CI green.
7. Rerun an unchanged workflow only when a transient GitHub runner or infrastructure failure is confirmed. Do not use retries as a substitute for collecting and fixing code failures.
8. Report success only after the exact run for the final relevant commit completes successfully. Distinguish local tests, CI test/build checks, package creation and physical Windows/hardware acceptance.

**CI cost and artifact policy:**

- The Windows workflow runs tests and a release build for code pushes and pull requests. Documentation-only changes are ignored by push/PR triggers.
- A newer run for the same branch or pull request cancels an obsolete in-progress run.
- Portable ZIP and installer packaging are not part of ordinary push/PR validation. They run only when the owner manually dispatches the workflow with `package_windows=true`.
- Package artifacts are retained for **3 days**, not 14. Do not upload packages for intermediate development commits or routine test runs.
- Keep only the newest useful, fully validated Windows package. Delete superseded or failed-run artifacts from the repository's Actions artifact storage. Source code and durable documentation belong in Git; build packages do not.
- The workflow's 3-day setting applies to new artifacts only. Existing artifacts created under the previous 14-day policy must be manually deleted or allowed to expire; changing the workflow does not retroactively shorten their retention.

If required repository write access, a Rust execution environment, a Windows workflow, or workflow logs are unavailable, do not make uncompiled speculative implementation commits or claim verification. State the specific blocker and what could and could not be checked. Continue with independently verifiable work only where it does not undermine these rules.

### Architecture and implementation rules

- Core remains the source of truth for project data, timeline invariants and edit validation.
- Every user edit must pass through a Core command using clone, validate, commit semantics and participate in undo/redo history where appropriate. UI widgets must not mutate project data directly.
- All timeline positions, durations, overlap calculations and output-time mapping use integer `Tick` values at 254,016,000,000 ticks per second. Do not use floating-point seconds for edit or timeline arithmetic.
- Derive output frame timestamps and frame counts from integer frame indices and the rational frame-rate numerator/denominator. Avoid cumulative rounded frame-duration addition.
- Keep FFmpeg and codec-library access inside `hyperdex-platform`. Other crates consume platform abstractions and must not directly call FFmpeg APIs.
- CPU composition is the correctness reference. GPU output must match it for supported operations; GPU availability must never be required for correctness.
- Preserve established track ordering, hidden-track behaviour, disabled-item behaviour, gap handling, deterministic source-time mapping and soundtrack synchronization when changing the renderer.
- Long-running work must use background jobs, bounded resources, accurate progress and real cancellation. Do not claim cancellation until active decoding/rendering/encoding actually stops.
- Keep project JSON readable and versioned. Add forward migrations and round-trip tests for schema changes; do not silently discard older project data.
- Keep scope focused on the music-video workflow. Do not pursue broad professional-NLE parity or reopen explicit product non-goals without an owner decision.
- Do not silently resolve decisions reserved for the owner, especially codec licensing/export-floor choices, design-system details and physical hardware acceptance.

### How to use the roadmap and status documents

This file defines the delivery sequence, acceptance criteria and durable engineering instructions. `docs/DEVELOPMENT_STATUS.md` is the concise snapshot of what is actually implemented, what is incomplete and what exact commits/runs verify it. `README.md` is the user-facing product summary and must not overstate capabilities. When these documents disagree with the current source or CI, verify the source and workflow, then correct all affected documents in the same focused development slice. Do not recreate a separate chat-handoff document; maintain the continuation instructions here.


## Product direction

HyperDex is a local-first, music-intelligent video editor. Its defining workflow is:

**Import media → understand the song and footage → generate an editable first cut → refine it on a timeline → export a synchronized video.**

HyperDex is not intended to reproduce the entire feature surface of DaVinci Resolve or another professional NLE. It should make this narrower workflow dependable, fast, approachable and unusually good.

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

The latest complete Windows package verification is run [#104](https://github.com/StellarEVE/HyperDex/actions/runs/43614319131) for commit `211d6ad73cfbe40aaef05af19b98260eabbb3ca1`. It passed workspace tests, release build, runtime dependency staging, portable ZIP creation, Windows installer creation and artifact upload. This is the latest full package validation for the current application source.

The current CI-policy/workflow configuration is validated by run [#106](https://github.com/StellarEVE/HyperDex/actions/runs/43616944483) for commit `9dca8e9b3772cedac6450d3c2900f6235a6ff6cd`. Workspace tests and the release build passed. Packaging and artifact-upload steps were skipped as intended for an ordinary push. This validates the ordinary CI path, not the manual packaging path under the new workflow.

The current source includes the persisted schema-4 transition model, transition-aware multi-track export and desktop audio-item export wiring. The app-worker audio export wiring was introduced in source commit `fd5e2089f70d458dddd34653c128e9cbabcba267` and is covered by the successful full Windows verification in #104. CI results do not establish physical desktop acceptance or hardware compatibility.

The CI cost policy is active: ordinary code push/PR runs execute workspace tests and a release build only; documentation-only changes are ignored; newer runs cancel stale runs for the same branch or pull request; Windows packaging runs only by manual dispatch with `package_windows=true`; and new package artifacts expire after 3 days. The cleanup has left one retained package artifact, `14121717047` (183,397,750 bytes), from run #104 and commit `211d6ad`. Superseded artifacts were deleted. Existing artifacts are not retroactively affected by the 3-day retention setting.


## Delivery phases

### Phase 0: Reliable development and distribution baseline

**Goal:** every code change is built and tested on Windows, and validation packages behave predictably.

- Keep ordinary Windows push/PR workspace tests and release builds green. The current cost-controlled workflow passed both checks in run #106. Full packaging passed in run #104 under the prior always-package workflow; the manual `package_windows=true` path under the current workflow still needs a deliberate package run when a new package is required.
- Verify the default OpenGL startup path on real Windows hardware.
- Verify project save/load and recovery behaviour.
- Validate FFmpeg decoding and encoding on reference machines.
- Test the portable package and installer on a clean Windows installation.
- Complete the FFmpeg and transitive-library license inventory before public binary distribution.

**Exit gate:** green Windows CI, a clean-machine launch, and a documented import/analyze/save/reopen/export smoke test. CI alone does not prove hardware compatibility.

### Phase 1: Complete the essential editing loop

**Goal:** users can refine and preserve a generated cut without bypassing the Core model.

- Route every edit through hyperdex-core commands and History.
- Current Edit-page controls support source-in/duration trimming, midpoint splitting, ripple deletion, drag/reorder movement, pinning, undo/redo, transition kind/duration/clear controls, and append-only audio-item placement with track mute and removal. Reordering compacts the video track into a contiguous cut. Freeform gap creation and freeform audio-item dragging are not yet implemented.
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
- The app-driven exporter handles multiple clips across visible video tracks, hard cuts, persisted incoming transitions, ordinary gaps and gaps left by disabled clips. Track order is bottom-to-top; upper-track gaps reveal lower tracks, and uncovered frames are encoded as black. Windows CI run #81 passed the original multi-track hard-cut MP4 integration tests and full packaging pipeline. The transition-aware exporter and its MP4 coverage passed complete Windows CI run #91.
- The selected-song soundtrack can be encoded as stereo AAC alongside the video, with output-level tests for duration and channel separation.
- The two-item transition/effect rendering path and transition-region cache remain separate. Schema 4 persists incoming transitions through Core commands; the shared `TimelineTransitionMap` is consumed by the multi-track app exporter for output-time compression, source-time mapping, and overlap rejection.

Remaining work, in implementation order:

1. **Expand placed-audio behaviour.** Audio-track creation, placement, mute/removal, worker decoding, stereo mixing and an app-worker MP4 integration test are implemented. The remaining audio work is source-video audio, per-item/track gain, fades, normalization and a dedicated limiter, with output-level tests. Run full Windows CI and desktop smoke testing for the current implementation first.
2. **Preview/export parity.** Drive preview playback and final output from the same schedule, source-time mapping, track composition, transition and audio rules. Preserve a correct software path and bounded render work.
3. **Full-timeline effects.** Apply per-clip and track-level effects through the same pipeline used by preview and export.
4. **Export finalization and cancellation.** Write to a temporary destination and atomically finalize on success; implement true cancellation during decoding/rendering/encoding, accurate progress and useful errors.

Keep the first supported output matrix deliberately small and testable, such as MP4 with H.264 video and AAC audio.

**Completed implementation stage: Phase 2.1, multi-track video composition.** The app-driven exporter now composites all visible video tracks in project order. Windows CI run #81 passed workspace tests and the complete packaging pipeline.

**Current implementation stage: Phase 2.3, placed-audio application integration.**

The first data-model slice is on main: an incoming clip can persist a transition kind, parameters and duration; Core validates the adjacent cut and duration constraints; `SetItemTransition` participates in undo/redo; and schema 1–3 projects migrate with no transition silently added. The schema-4 validation correction at `67ae474269399175d4d507ca45b8cc12c067ea5b` passed complete Windows CI run #85. The renderer-side `TimelineTransitionMap` now drives the CPU multi-track exporter. It compresses the outgoing/incoming transition region once, maps other tracks through the same output clock, rejects overlapping cross-track transition regions, uses independent outgoing/incoming readers, and preserves track stack order. MP4 tests cover a dissolve, an upper track above a transition, sequential transitions, frame-count reduction and soundtrack synchronization. The Edit inspector supports transition type, duration and clearing through Core commands, with AppState tests for set/change/clear and undo/redo. The exporter passed the complete Windows workflow in run #91 and the UI controls passed the complete workflow in run #92. Cross-track transition regions that overlap in project time remain rejected until simultaneous transitions have an explicit compositing and time-mapping design.

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

**Goal:** hardware accelerates HyperDex when it works, but is never required for correctness.

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

Do not spend early milestones attempting full professional-NLE parity. Defer node-based grading, a Fusion-scale compositor, multicamera editing, a DAW replacement, cloud collaboration, user accounts and a general-purpose plugin marketplace. Reopen these decisions only when they clearly support HyperDex's primary workflow.

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
- [x] Schema-4 incoming-transition model and validation-order correction pass the complete Windows CI workflow (run #85).
- [x] Windows package staging, portable ZIP and installer creation pass for the verified multi-track export commit (CI run #81).
- [ ] Application launches on a clean Windows machine using the default renderer.
- [ ] Import, analyze, edit, save, reopen and export smoke test passes.
- [ ] FFmpeg and bundled transitive-library licensing review is complete before public binary distribution.


### Phase 2.2 implementation progress: transition timeline mapping and full-timeline export

The renderer now has a pure, decoder-independent `TimelineTransitionMap` foundation. It is built for the exact set of video tracks participating in an export and defines the shared time interpretation:

- An incoming transition of duration `D` at cut `C` uses outgoing source time `[C-D, C)` and incoming source time `[C, C+D)`.
- Those two source halves produce one output region of duration `D`. The full source region `[C-D, C+D)` is compressed by exactly `D` ticks.
- During that output region, tracks not undergoing the transition sample a continuous source timeline that advances from `C-D` to `C+D` at 2x speed. The transitioning track gets separate outgoing and incoming source timestamps plus normalized blend progress.
- Sequential transitions accumulate compression once each. A later transition's output start is its source start minus all earlier transition durations.
- Cross-track transition regions that overlap in source project time are rejected because they cannot share one deterministic global source-time mapping.
- Hidden tracks are excluded. If either adjacent clip is disabled, its transition is inactive and does not compress output time. Only tracks selected for rendering contribute transition metadata.
- The output schedule is derived from the compressed Tick range using the existing rational frame-rate scheduler. No per-frame rounded durations are accumulated.

The time model is consumed by the app-driven exporter. Each transition track is decoded with independent outgoing and incoming readers, and its blended frame is inserted into the project's normal bottom-to-top track stack. Other tracks use the same compressed source-time map. The selected-song AAC path uses output frame count, so audio remains synchronized and does not change playback speed. MP4 integration tests cover a two-clip dissolve, an upper video layer over a transition, sequential non-overlapping transitions, compressed frame count and stereo AAC synchronization. The Edit inspector exposes transition type, duration and clear controls through Core commands; AppState coverage verifies changes and undo/redo. The complete Windows runs #91 and #92 passed for the exporter and UI slices.


### Phase 2.3 implementation progress: audio-track Core foundation

The Core model enforces track and asset compatibility: Audio tracks accept Audio assets only, and Video tracks accept Video assets only. Invalid placements fail project validation and cannot be committed through command history or saved as a valid project.

Track creation is represented by `Command::AddTrack`. It creates the track and corresponding empty item group as one atomic Core edit, participates in snapshot undo/redo, and relies on normal project validation to reject duplicate track IDs. Tests cover valid and invalid asset/track combinations, AddTrack undo/redo, and duplicate-ID rejection.

The Edit page exposes audio-track creation, append-only placement of probed audio assets, mute/unmute and item removal. The desktop worker decodes unique source assets and passes them with the optional selected-song bed to the renderer, which mixes stereo AAC. A Windows app-worker integration test verifies placed audio is present only during its timeline interval. Full Windows CI and manual desktop checks are still required.


### Phase 2.3 implementation progress: audio-item placement

The Edit page now exposes audio tracks separately from the video timeline. Users can create multiple audio tracks, append a probed audio asset after the last item on a track, mute/unmute a track and remove an audio item. AppState validates that the target track is an unlocked audio track and that the asset exists, is an audio asset and has completed probing. Source intro/outro trims define the placed item's source range; its timeline duration is the remaining usable source duration.

Each placement is a Core `InsertItem` command and is marked manual/pinned in the same history entry. Track creation is `Command::AddTrack`; mute and removal use existing Core commands. AppState tests cover sequential placement, saved source/timeline coordinates, one-step undo, track removal by undo, wrong track/asset kinds and unready files.

This is timeline metadata and editing at the application layer. The desktop worker now decodes and supplies placed audio assets to the renderer; app-worker MP4 integration verifies the output audio interval. Treat the feature as locally verified, with full CI and interactive desktop verification still outstanding.


### Phase 2.3 integration status: desktop audio-item export

The renderer exposes `render_timeline_with_audio_items` and `StereoAudioAssetInput`, with MP4-level tests. `hyperdex-app::jobs::run_export` decodes each unique asset referenced by the selected song or enabled items on visible, unmuted Audio tracks, passes the PCM to the renderer, and reports failures for missing, wrong-kind or undecodable assets. An app-worker MP4 integration test verifies the output audio interval. Full Windows CI and manual desktop acceptance remain outstanding.

### Phase 2.3 implementation progress: stereo audio-item renderer

The renderer accepts decoded PCM inputs keyed by audio asset ID. Enabled items on visible, unmuted Audio tracks are sampled against the shared transition-compressed source timeline and summed into the synchronized stereo AAC stream. The selected-song bed is sampled on output time and is not sped up by transitions. Placed audio can produce AAC even without a selected-song bed. Muted tracks, hidden tracks and disabled items do not contribute.

The current mixer sums sources and clamps samples to the legal PCM range. Per-item volume, fades, normalization and a dedicated limiter remain separate work. Renderer MP4 tests cover placed audio without a song bed, stereo channels, item-end silence and muted/hidden/disabled items. The app-worker MP4 test covers placed-asset decoding and output timing. Full Windows CI and interactive export smoke testing are pending.
