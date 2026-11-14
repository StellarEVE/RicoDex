# Windows desktop smoke test

Run this checklist against the latest portable ZIP or installer from GitHub Actions. Record the package type and commit SHA. A passing CI run is necessary, but does not replace interactive checks.

## Install and launch
- [ ] Extract the portable ZIP or install the Inno Setup package.
- [ ] Confirm the Microsoft Visual C++ x64 runtime is present.
- [ ] Launch RicoDex with the packaged launcher and confirm the main window opens using the default OpenGL renderer.
- [ ] Close and relaunch once.
- [ ] If launch fails, collect `ricodex.log` and the Windows error details.

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
- [ ] Close and reopen RicoDex, then open the project with Ctrl+O.
- [ ] Confirm song selection, source trims, timeline order, durations and source-in positions are restored.
- [ ] Confirm generated/manual provenance and pin states are restored from the project.
- [ ] Confirm pinned clips remain unchanged during re-analysis, while unpinned generated clips may be refined.
- [ ] Import another video after reopening, analyze again, and confirm the existing pinned/manual clips remain intact.
- [ ] Confirm Ctrl+Shift+S opens Save As.

## Relink moved media
- [ ] Close RicoDex and move a source file without changing its contents.
- [ ] Reopen the project, choose Locate, and select the moved original or an identical copy. Confirm acceptance.
- [ ] Repeat with a different file and confirm it is rejected.
- [ ] Confirm the project becomes dirty after a successful path update and can be saved.

## Export
- [ ] Export MP4 and confirm progress completes and the file exists.
- [ ] Open the output in a media player and check clip order and soundtrack playback.
- [ ] Confirm the selected song is audible as stereo AAC and starts at output time zero.
- [ ] Verify no frames come from before an intro trim or after an outro trim.
- [ ] Compare audio and video duration with the contiguous timeline duration.
- [ ] Repeat without a selected song and confirm the documented video-only behaviour.

## Known scope limits

The app exporter currently produces a contiguous hard-cut video sequence and includes the selected song as a stereo AAC soundtrack when one is selected. Clip-source audio, timeline audio tracks, fades/gain controls, gaps, transitions and unified interactive preview/export remain Phase 2 work. Passing this smoke test does not complete the FFmpeg and transitive-library licensing review required for public binary redistribution.
