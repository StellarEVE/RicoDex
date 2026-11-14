# Windows desktop smoke test

Run this checklist against a portable ZIP or installer produced by a manual Windows CI dispatch with `package_windows=true`. Record the package type and commit SHA. A passing CI run is necessary, but does not replace interactive checks.

## Install and launch
- [ ] Extract the portable ZIP or install the Inno Setup package.
- [ ] Confirm the Microsoft Visual C++ x64 runtime is present.
- [ ] Launch HyperDex with the packaged launcher and confirm the main window opens using the default OpenGL renderer.
- [ ] Close and relaunch once.
- [ ] If launch fails, collect `hyperdex.log` and the Windows error details.

## Import and analyze
- [ ] Import one song and at least two videos.
- [ ] Wait for probing to finish, select the song, and set non-zero intro/outro trims on one video.
- [ ] Start analysis and wait for the generated cut.
- [ ] Export later and verify selected source ranges respect the configured trims.

## Edit the cut
- [ ] Change a clip's source-in point and duration within the displayed usable source window.
- [ ] Split a clip at its midpoint.
- [ ] Delete a clip and confirm later clips ripple left.
- [ ] Drag a clip before another clip and confirm the order changes.
- [ ] Confirm the reordered sequence stays contiguous for the current sequential exporter.
- [ ] Undo and redo each operation.
- [ ] Confirm an invalid source range is rejected without corrupting the project.

## Save and reopen
- [ ] Use Save As to create an `.rdx` project.
- [ ] Make another edit and save with Ctrl+S.
- [ ] Confirm the unsaved-changes indicator clears.
- [ ] Close and reopen HyperDex, then open the project with Ctrl+O.
- [ ] Confirm song selection, source trims, timeline order, durations and source-in positions are restored.
- [ ] Confirm generated/manual provenance and pin states are restored from the project.
- [ ] Confirm pinned clips remain unchanged during re-analysis, while unpinned generated clips may be refined.
- [ ] Import another video after reopening, analyze again, and confirm the existing pinned/manual clips remain intact.
- [ ] Confirm Ctrl+Shift+S opens Save As.

## Relink moved media
- [ ] Close HyperDex and move a source file without changing its contents.
- [ ] Reopen the project, choose Locate, and select the moved original or an identical copy. Confirm acceptance.
- [ ] Repeat with a different file and confirm it is rejected.
- [ ] Confirm the project becomes dirty after a successful path update and can be saved.

## Export
- [ ] Export MP4 and confirm progress completes and the file exists.
- [ ] Open the output in a media player and check clip order and soundtrack playback.
- [ ] Confirm the selected song is audible as stereo AAC and starts at output time zero.
- [ ] Verify no frames come from before an intro trim or after an outro trim.
- [ ] Compare audio and video duration with the contiguous timeline duration.
- [ ] Repeat without a selected song. Confirm enabled items on visible, unmuted audio tracks still produce stereo AAC; confirm the export is video-only when no audible audio-track items exist.

## Known scope limits

The app exporter composites visible video tracks, preserves timeline gaps and disabled-clip intervals, applies persisted incoming transitions, and mixes the selected song plus enabled items on visible, unmuted audio tracks into stereo AAC. A worker-level MP4 integration test covers placed audio timing. Source-video audio, gain/fades, normalization, unified interactive preview/export, and clean-machine hardware acceptance remain incomplete. Passing this smoke test does not complete the FFmpeg and transitive-library licensing review required for public binary redistribution.

Placed-audio export passes a local app-worker MP4 integration test. The packaged build remains unverified for this path until Windows CI passes and the interactive checklist is completed.
