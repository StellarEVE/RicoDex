# New-chat handoff prompt

Paste this prompt into a new ChatGPT conversation:

\`\`\`text
Continue development of RicoDex from the existing GitHub repository:
https://github.com/StellarEVE/RicoDex

Read these repository documents first:
- README.md
- docs/DEVELOPMENT_STATUS.md
- docs/ROADMAP.md
- docs/PLAN.md
- docs/WINDOWS_SMOKE_TEST.md
- docs/LICENSING.md

Work directly on main. Do not create a feature branch or pull request. Make focused commits, run the Windows GitHub Actions workflow after each meaningful implementation slice, inspect failures, fix them, and do not claim success until the exact run completes successfully. Keep README.md, docs/ROADMAP.md and docs/DEVELOPMENT_STATUS.md accurate as the implementation changes.

The current verified milestone is Phase 2.1 multi-track video composition. The app-driven exporter renders visible video tracks in project order, later tracks above earlier tracks, preserves gaps, reveals lower tracks through upper-track gaps, and supports the selected-song stereo AAC soundtrack. Windows CI run #81 passed workspace tests, release build and Windows packaging.

The current main branch is working on Phase 2.2. Project schema 4 adds optional incoming transition metadata to each item (kind, parameters, duration), an undoable Core command, validation rules, and migration from schemas 1-3. The latest validation-order correction is commit 67ae474269399175d4d507ca45b8cc12c067ea5b. At the time this handoff was written, Windows CI run #85 was still running:
https://github.com/StellarEVE/RicoDex/actions/runs/43560350393
Check its current result before making more changes. Earlier runs #83 and #84 failed on transition-model tests; the stale test type was fixed and the validation-order failure was corrected in commit 67ae474. Diagnose any remaining CI issue before proceeding.

Important boundary: persisted transitions are NOT yet rendered by the app-driven multi-track exporter, and there is no Edit-page transition control. The separate render_project path renders exactly two clips with one transition, a request-level effect stack and an optional transition-region cache, but it is not full-timeline integration.

Next objective:
1. Verify/fix CI for the schema-4 model commit.
2. Integrate incoming transitions into the shared timeline/export renderer with precise integer-tick and rational-frame timing.
3. Define output-time compression for each adjacent-cut transition. Render the outgoing tail and incoming head as one transition region, with clear semantics for lower tracks beneath the transition and upper tracks above it.
4. Reject ambiguous overlapping transition regions unless a deliberate global mapping/compositing design is added.
5. Add MP4 integration tests for a dissolve, lower-track visibility underneath it, sequential non-overlapping transitions, disabled/hidden clips, correct output duration/frame count, and soundtrack synchronization.
6. Add an Edit-page transition control via the Core command and verify undo/redo.
7. Update docs and continue to the next roadmap stage only after Windows CI is green.

Other known gaps: unified preview/export parity, timeline audio items, source-clip audio, gain/fades/normalization, full-timeline clip/track effects, shared cache invalidation, atomic export finalization, true active-render cancellation, physical Windows/hardware acceptance, and FFmpeg/transitive-library licensing review.

Keep scope disciplined. Core owns project data and validation; all timeline arithmetic uses Tick; FFmpeg access stays in ricodex-platform; CPU composition is the correctness oracle; no feature is complete without integration tests and verified Windows CI.
\`\`\`
