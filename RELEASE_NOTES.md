# HyperDex Studio alpha.9 foundation

This document records the baseline Rust foundation for HyperDex and the Core, Audio, Media, Platform, Render, Engine, App and CLI crates.

No new feature-completion claim is made by this migration. See docs/ROADMAP.md for known incomplete work and docs/PLAN.md for the complete intended product behaviour.

The initial imported foundation's notes reported 67 tests passing. That historical count is not a current HyperDex test result. Current acceptance is determined by the Windows CI runs recorded in docs/DEVELOPMENT_STATUS.md.

## Distribution restriction

Bundled binary release packaging is not enabled. The FFmpeg distribution and codec licensing decision remains open and must be resolved before shipping executables.