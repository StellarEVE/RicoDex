# RicoDex alpha.9 foundation import

This is the initial RicoDex repository migration of the existing PMV-Genie alpha.9 Rust foundation. This migration establishes the RicoDex repository identity and carries over the existing Core, Audio, Media, Platform, Render, Engine, App and CLI crates.

No new feature-completion claim is made by this migration. See docs/ROADMAP.md for known incomplete work and docs/PLAN.md for the complete intended product behaviour.

The upstream PMV-Genie alpha.9 notes historically reported 67 tests passing. This is not a fresh RicoDex test result. The first RicoDex CI run is the verification gate for this repository.

## Distribution restriction

Bundled binary release packaging is not enabled. The FFmpeg distribution and codec licensing decision remains open and must be resolved before shipping executables.