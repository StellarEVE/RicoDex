# RicoDex implementation roadmap

This roadmap distinguishes the existing foundation from the intended product. Update it only as verified work lands.

## Existing foundation

The imported alpha.9 workspace contains eight Rust crates, song structure analysis, staged footage analysis, a deterministic planner, project persistence, CPU and GPU composition paths, transition/effect foundations, a four-page desktop shell, a CLI and a Windows build workflow.

The RicoDex alpha.9 release notes historically reported 67 workspace tests passing. This is not a fresh RicoDex test result. The first Windows CI run must establish the current baseline.

## Known incomplete or open work

- Alpha.9 notes describe the Edit page as selection and inspection only; richer editing operations and command/undo integration remain work.
- App-level export is a sequential contiguous run with hard cuts. Full timeline export with transitions, gaps, audio mixing and transition-aware cache integration remains work.
- Background render scheduling, export queue and per-transition cache integration need end-to-end verification and completion.
- Real Windows hardware proof remains required for startup reliability, native decode, trial-based hardware encode selection, dual-GPU switching and low-end hardware behaviour.
- The startup issue on the low-end reference machine needs its startup log and device-specific diagnosis.
- The exact FFmpeg redistribution configuration and guaranteed software encoder floor must be decided before binary distribution.
- UI design tokens, icons and widget inventory remain open design work.

## Work sequence

1. Reproduce the current baseline with clean Windows workspace tests and a release build.
2. Resolve and document the FFmpeg licence/distribution configuration before bundling binaries.
3. Complete Edit operations, command integration, pin protection and undo/redo tests.
4. Implement full timeline render/export, transitions, gaps, clip audio mixing and atomic output handling.
5. Integrate background render scheduling and content-addressed cache state for clips and transitions.
6. Stabilise Windows startup and hardware capability trials on real reference hardware.
7. Package and test portable and installer builds on clean Windows environments.
8. Publish only after CI and the release acceptance checklist are complete.

## Contribution rules

Preserve local-only operation and workspace boundaries. Keep edit positions in fixed integer ticks. Route codec and OS media dependencies through Platform. Keep the CPU compositor as the correctness oracle for GPU output. Do not claim a feature works without tests and accurate user-visible status. Software-rendered tests do not prove GPU performance, and a sandbox without real graphics hardware does not prove accelerated paths.