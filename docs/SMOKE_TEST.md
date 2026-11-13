# Windows desktop smoke test

Use this checklist on a Windows machine with the latest portable ZIP or installer from the RicoDex GitHub Actions workflow. Record the commit SHA and package type with the results. CI passing is necessary, but it does not replace these interactive checks.

## 1. Install and launch

- [ ] Extract the portable ZIP or install the Inno Setup package.
- [ ] Confirm the Microsoft Visual C++ x64 runtime is present.
- [ ] Launch RicoDex using the packaged launcher.
- [ ] Confirm the main window opens using the default OpenGL renderer.
- [ ] Close and relaunch once to check startup is repeatable.
- [ ] If launch fails, collect `ricodex.log` and the Windows error details.

## 2. Import and analyze

- [ ] Import one song and at least two video files.
- [ ] Wait for probing to finish and confirm the files show as ready.
- [ ] Select the song asset.
- [ ] Set a non-zero intro trim and outro trim on one video.
- [ ] Start analysis and wait for the generated cut to finish.
- [ ] Confirm the generated cut contains clips from available video sources.
- [ ] Export later in this test and verify the selected source ranges respect the configured trims.

## 3. Edit the cut

- [ ] Open the Edit page and select a clip.
- [ ] Change its source-in point and duration within the displayed usable source window.
- [ ] Split a clip at its midpoint.
- [ ] Delete a clip and confirm later clips ripple left.
- [ ] Drag a clip before another clip and confirm the order changes.
- [ ] Confirm the reordered sequence stays contiguous for the current sequential exporter.
- [ ] Undo and redo each operation and confirm the timeline returns to the expected state.
- [ ] Confirm invalid source ranges are rejected without corrupting the project.

## 4. Save and reopen

- [ ] Use Save As to create a new `.rdx` project.
- [ ] Make another edit and save with Ctrl+S.
- [ ] Confirm the unsaved-changes indicator clears.
- [ ] Close RicoDex and reopen it.
- [ ] Open the saved project with Ctrl+O.
- [ ] Confirm the song selection, source trims, timeline order, clip durations and source-in points are restored.
- [ ] Confirm the existing timeline is protected from accidental re-analysis or new media imports. This is currently intentional because generated-versus-manual clip provenance is not yet persisted.
- [ ] Confirm Ctrl+Shift+S opens Save As.

## 5. Verify media relinking

- [ ] With RicoDex closed, move one source file without changing its contents.
- [ ] Reopen the project and use Locate for the missing source.
- [ ] Select the moved original or an identical copy and confirm it is accepted.
- [ ] Repeat with a different file and confirm it is rejected.
- [ ] Confirm the project remains dirty after a successful path update and can be saved.

## 6. Export

- [ ] Choose an MP4 output path and start export.
- [ ] Confirm progress reaches completion and the output file exists.
- [ ] Open the output in a media player and check that clips appear in the expected order.
- [ ] Check that no frames come from before an asset's intro trim or after its outro trim.
- [ ] Check output duration against the contiguous timeline duration.

## Known scope limits

This build's app exporter produces a contiguous, video-only hard-cut sequence. Audio mixing, timeline gaps, transitions and a unified interactive preview/export pipeline are not part of this smoke test; they are Phase 2 work. A successful smoke test does not complete the FFmpeg/transitive-library licensing review required for public binary redistribution.
