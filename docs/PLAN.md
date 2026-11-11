# RicoDex product breakdown

The original planning document intentionally used the unnamed label "the Program". RicoDex is the product name chosen after that document was written. The body below is retained as the owner's original specification.

---

# The Program: a complete breakdown

A definition document for an unnamed program. No brand name has been chosen, and this document deliberately contains none: it refers to the software only as the Program, and to its internal parts by plain functional names. It consolidates everything the planning process produced: the original product brief, the study of nine reference programs, page by page feature verdicts, a full architecture process run deliberately through five locked decisions, a staged analysis pipeline designed after the first working version exposed its weaknesses, and the hard lessons of a first foundation build and its first real releases. It is written so that any competent reader, human or language model, can produce a concise and faithful implementation plan from it without needing any other document.

How to read it: sections 1 to 5 define what the Program is, who it serves, and where its borders are. Sections 6 to 9 specify behaviour. Sections 10 to 14 specify machinery and the reasoning behind it. Sections 15 to 18 cover reliability, building, releasing, testing and licensing. Sections 19 to 22 list what is still undecided, the build order the decisions dictate, and the short list of rules no plan may violate.

## 1. What the Program is

The Program is a desktop video editor specialised in exactly one job: making music videos whose cuts land on the music. You bring one song and a pile of source videos. The Program analyses the structure of the song, studies the footage, and generates a first cut that is already synced to the beat. It then hands you a real timeline editor to make that cut yours, and exports the finished video through a graphics accelerated pipeline that has been preparing your edit in the background while you worked.

In one sentence: an automatic, structural, beat synced first cut, inside an editor worth finishing it in.

It is a native application, fully local. There is no account, no subscription, no watermark, no server and no cloud component. Media comes from the user's own disks and never leaves them. It is open source under the Apache 2.0 licence. Its spirit is that of the free local editors in the CapCut class: friendly, fast, no catches. Its depth is that of the professional timeline editors in the Premiere class, underneath. Its working shape is a small set of full screen pages in the manner of DaVinci Resolve, shrunk to the four pages this one job needs: Media, Cut, Edit, Export.

The Program is deliberately not a general purpose editor competing with the professional suites on their whole territory. Research into a large open source editor that attempted full professional parity measured that ambition at thousands of hours of work. The Program instead occupies the niche none of the studied tools occupied: music intelligence. The song is not an audio track that happens to be present. The song is the structure the whole edit is built from.

## 2. Who it serves

The Program serves the fan music video world in the broad sense: people who cut existing footage to music as a creative act. That family includes videos cut from animated series and films, from games, from live action film and television, and from adult material. These communities have used single purpose generator tools for years, and those tools share the same limits: they generate a cut and then abandon the user, with weak or no editing afterwards, slow analysis pipelines built on external processes, and no respect for the structure of the song beyond a raw beat interval. The Program is built for the maker who wants the generator's speed and the editor's control in one local tool, and who may have modest hardware. It must feel alive on a low end laptop while scaling up on strong machines.

The intended user is not assumed to be technical. Controls carry plain names. Features that are not yet real are never drawn as decoration: a control that cannot work is absent or visibly disabled with its reason printed, because a tool that pretends is worse than a tool that admits.

## 3. Design principles

Each principle below is a decision, with the reasoning that produced it. A plan that violates one of these is not a plan for this Program.

* **Local files only, permanently.** Media is imported from local disks by file picker or drag and drop. No external media library service is integrated, ever. One such integration was considered during planning and refused outright and permanently, on the principle that the Program's relationship is between the user and their own files. Nothing is uploaded, nothing is indexed anywhere else, and no account exists to lose.
* **The generator proposes, the editor disposes.** Automation produces the first draft only. Nothing the generator does is beyond the user's power to undo, rearrange, replace or protect from further automation. This principle later produced the pinned clip rule in the analysis pipeline: whatever the user has touched is never silently recut.
* **Music is structure, not a metronome.** Beats have strengths. Songs have bars, phrases, and change points where the music itself lurches, such as a chorus arriving. Cutting decisions use the whole structure: a strong cut belongs on a strong beat, a section change in the song deserves a section change in the footage, and quiet passages should not be cut like choruses.
* **Nondestructive everything.** A project is pure data: clips, ranges, properties, keyframes, effects and order. Source files are never modified, never renamed, never written to. Every edit is an undoable command. Derived data of any kind lives outside the project file and is always disposable.
* **Graphics acceleration first, never graphics only.** Composition, preview and footage analysis run on the graphics processor where the machine offers one, because that is where the performance for this workload lives. But hardware varies enormously, so the design is a capability ladder per machine, measured at runtime, with a correct software path as the floor and as the correctness reference. No vendor is hardcoded. A low end laptop with an entry level discrete graphics card, dual graphics switching and no hardware encoder is kept as a reference floor that must still feel responsive. It is a test case, never the design target.
* **Exact integer time.** All edit mathematics runs on fixed integer ticks at 254,016,000,000 ticks per second. That rate divides evenly by every common video frame rate and by both 44,100 Hz and 48,000 Hz audio, so video edits and audio samples can never drift against each other, and floating point seconds are banned from edit maths. (This is also the internal tick rate a major professional editor uses, found in its scripting documentation during research.) Floating point remains acceptable for analysis and scoring, never for positions and durations.
* **Crash proofing is a feature.** Saves are atomic. A backup of the previous save is kept beside the project. Losing work to a crash is treated as a defect, not an accident. The same stance produced a startup log written from the program's first moments, so that even a failure before any window appears leaves evidence.
* **Self contained shipping.** The Program ships as a finished package: a portable archive and an installer. Its codec libraries travel beside the executables, because the executables are dynamically linked against them and will not start on a clean machine without them. First run needs no downloads and no installation ritual beyond the package itself.
* **Honesty in the interface.** Progress is real progress, reported as work completes, never a bar that sits frozen while a hidden process works. Disabled controls say why. Substituted components, such as an encoder chosen because the requested one does not exist in the installed codec build, are reported by their resolved name, never silently.
* **Scope discipline.** The Program is a hybrid on purpose: the approachability of a simple generator editor and the timeline depth of a professional one, and nothing beyond what the one job needs. Every refused feature in section 5 was refused for a stated reason, and refusals stand until the product's owner reopens them explicitly.

## 4. The end to end workflow

1. On the Media page, the user imports a song and a set of source videos. Import triggers only fast container probing: duration, resolution, frame rate, codec, rotation. No decoding, no waiting.
2. The user sets per video intro and outro trims, chooses the song if several were imported, and presses the one deliberate button, Begin analysis. Importing alone never starts heavy work.
3. The user lands on the Cut page. Song analysis completes almost immediately, so the beat grid exists within moments. A quick sampling pass covers every video, and as soon as the whole pool has quick coverage, the planner generates a rough cut. A full analysis pass then deepens the data file by file, and the rough cut refines itself where the user has not touched it. Progress is per asset and overall, with an estimated time remaining, and everything is cancellable per asset and per stage.
4. The user previews the cut with the song in sync, and either exports directly from the Cut page or continues to the Edit page.
5. On the Edit page, the user arranges the multitrack timeline: moving, trimming, splitting, swapping, adding transitions and effects, animating properties with keyframes, placing text, and protecting finished clips from regeneration.
6. On the Export page, the user picks format, codec, resolution, frame rate and quality. Much of the timeline has already been rendered into the background cache during editing, so export renders only what is missing or stale, writes to a temporary file, and renames it into place only when complete, so a finished file is never half written.

## 5. What the Program is not

The boundaries were chosen as deliberately as the features, each with its reason.

* **No persistent media library.** There is no database of the user's collection living between projects. A project owns its sources; everything learned about them lives in that project's cache. Reason: a library is a second product, with its own privacy and maintenance weight, and the one job does not need it.
* **No filtering of the pool by tags or performers.** The pool is the files the user imported. Reason: the Program does not classify people's content, and curation belongs to the user.
* **No song library.** Songs are files in a project, like videos.
* **No machine learning classification of content.** No background removal, no text to speech, no voice cloning, no consumer AI stack. The single learning based component is local transcription used to draft lyric captions, which the user then edits. It runs on the user's machine.
* **No external media service integration of any kind.** Local disks only, permanently.
* **No importing beats from chart files or timestamp lists.** The Program hears the song itself. External beat data would bypass the analysis that makes the cut structural.
* **No automatic silence trimming of songs.** The song the user chose is the song, whole.
* **No live performance mode.** This is an editing tool, not a performance instrument.
* **No downloadable transition or shader packages.** The built in set is the set. This keeps the rendering engine one owned, testable codebase.
* **One timeline per project.** No multiple sequences. The one job has one output being built at a time.
* **No timeline interchange export.** No foreign edit decision formats, no sidecar beat files. The project file is readable data, and the exported video is the product.
* **No automated framing judgements.** No smart crop, no match cutting. Framing and continuity are the user's craft; the Program supplies the tools, not the taste.
* **Text appears only when the user puts it there.** Nothing textual is generated onto the video automatically, including captions, which are drafted only on request and remain editable data.

## 6. The four pages in detail

The interface is four pages over one shared project state. Page navigation lives only in a footer bar, as wide tabs taking up much of the bar, with no duplicate tabs at the top. That rule came from direct use: duplication wasted vertical space the timeline needs.

### 6.1 Media page

* Import of songs and videos from local disks, by file picker or drag and drop.
* On import, each file is probed only: duration, resolution, frame rate, codec and rotation appear immediately, with poster frames and filmstrips filling in behind from a cheap background pass.
* The pool shows every asset with its probe data. There is no library behind it.
* Per video intro and outro trims are set here, once, and are respected by every later generation. The reasoning: most sources have logos, intros or credit tails that should never be cut from, and stating that once beats trimming every generated clip forever.
* The song is chosen here when several audio files were imported. A project may also hold multiple songs, whose beat timelines stitch into one continuous track.
* Song analysis results are presented honestly: detected tempo, beat count, bar structure and change points, so the user can see what the planner will believe about the song.
* A missing media relink flow: when a file has moved, the project recognises it again by its content fingerprint rather than by path alone, because paths are hints and content is identity.
* The Begin analysis button is the gate to the Cut page. It is enabled only when a song is chosen and at least one video exists, and when disabled it prints the reason.

### 6.2 Cut page

* The beat engine made visible: onsets with strengths on a tempo grid, bars and phrases, and a change point layer marking structural turns in the song.
* The staged pipeline at work, described fully in section 8: a rough cut within moments, then continuous refinement, with per asset progress bars, a weighted overall bar, an estimated time remaining, and a plain status line in forms like "rough cut ready, refining 3 of 8 videos".
* Every clip on the strip shows whether it is provisional or settled. Provisional clips are drawn dimmed and hatched. Every clip carries a pin toggle.
* Generation controls, specified in section 7, can be adjusted and generation rerun. Clips the user has pinned survive regeneration untouched.
* A full preview player for the generated cut: the song audible in sync, scrubbing, transport controls, loop.
* One click onward: export this cut immediately, or continue to the Edit page. A user satisfied on this page never has to open the editor.

### 6.3 Edit page

* A multitrack timeline: video, overlay, text and audio tracks, colour coded by type, with mute and lock, and tracks added on demand. Adjustment layers apply a look or an effect stack to everything beneath them. Overlay tracks support blend modes.
* Editing operations, all as undoable commands: drag, trim, split, swap sources, swap positions, reorder, insert, remove, enable and disable, ripple behaviour, freeze frame.
* Transitions between clips from the named set in section 10, each a genuine form implemented in the compositor, never a renamed fade.
* A full clip inspector: transform (position, scale, rotation, anchor point), crop, speed, volume, fit, opacity, and a pinned flag that protects the clip from regeneration.
* Keyframe animation for animatable properties, with a curve editor: linear, eased and Bezier interpolation with value graphs, so a zoom pulse or a volume swell is drawn rather than guessed.
* Clip audio retained: every clip keeps its own sound, mixed against the song tracks with per clip volume, song gain and fades, loudness meters, and a normalisation action for clip audio.
* An effects system: per clip effect stacks and adjustment layers carrying the effects named in section 10, plus colour looks and split screen layouts.
* Text as a first class citizen: manual text and styled titles on text tracks, and lyric captions drafted locally from the song, stored as an editable caption track.
* The user's own timeline markers: coloured, named, with durations, exportable as a list.
* Cache state visible on the timeline: every clip and transition region shows whether its background render is pending or ready.
* Workspace presets for page layout, and shortcut profiles matching the well known editors, plus custom bindings.

### 6.4 Export page

* Output choice: the container and codec matrix of section 10, resolution, frame rate, and quality expressed per encoder so the numbers mean something on the encoder actually used.
* Fit choices for mismatched aspect ratios: crop, black bars, or blurred background padding.
* The render queue: multiple export jobs, each with progress and an estimated time remaining, running in the background without ever locking the editor.
* Export presets, part of one preset system that also covers generation settings, clip properties, favourite effects and transitions, and recently used items.
* The resolved encoder name is displayed on the page, so what actually encoded the file is never a mystery.

## 7. Generation controls

The planner's controls carry plain names, each with a defined meaning. A plan must implement these semantics, not just these labels.

* **Clip variance:** how far a clip's start point may wander from its most obvious candidate position. Low variance hugs scene starts and best moments; high variance explores.
* **Source reuse protection:** a penalty that grows as a source is used again, so a long pool is not strip mined from one favourite file.
* **Spread:** preference for picks spread across distant sections of long sources, rather than clustered in one good minute.
* **Continue start points:** a source resumes where its last clip stopped, so consecutive clips from one source continue its footage in order. This is a proven control inherited from the studied generator tools.
* **Placement modes:** at minimum, a windowed proportional walk in which used spans are never reused, and evenly spaced windows across each source.
* **Minimum and maximum clip lengths,** expressible in beats, in bars, and in seconds, because musical thinking and clock thinking are both legitimate.
* **Cut offset:** cuts may land slightly ahead of the beat, because a cut that arrives a fraction early feels ahead of the music and one on the exact sample can feel late.
* **Snap switches:** whether clip boundaries snap to scene boundaries, to black range edges, or to neither, traded against strict beat placement.
* **Cut templates:** the cutting pattern of an existing finished video, meaning its sequence of clip lengths against the beat grid, can be extracted and reapplied to new material.
* **Presets:** generation settings save and load through the one app wide preset system.
* **Determinism:** generation is driven by an explicit seed, so the same project, settings and seed produce the same cut, byte for byte. Randomness the user cannot reproduce is a bug in a tool like this, because a liked cut must be reachable again.

## 8. Analysis

Analysis is where the first version of this product idea failed in real use, and the design below is the direct answer. The failure, measured in the earlier attempt: footage analysis ran three separate full decode passes on the processor, one each for scenes, black ranges and motion, at tiny frame size, none of them using the graphics hardware that sat idle, and each pass blocked until the whole file finished, so the progress bar froze and the user waited blind. Every rule in this section exists to make that failure structurally impossible.

### 8.1 Song analysis

* Decoding through a pure Rust audio library first, with the codec library as fallback for formats it cannot read.
* Onset detection by multiband spectral flux: the spectrum is split into bands and each band's energy increase is tracked, which separates a kick from a hi hat from a vocal entrance far better than a single broadband measure. Each onset carries a strength.
* Tempo estimation by autocorrelation of the onset signal, checked against a synthetic fixture of known tempo (a click train generated at exactly 120 beats per minute must measure within one beat per minute, and does).
* Two grid philosophies: a transient mode that trusts strong onsets, and a grid mode that trusts the measured tempo and phase. Bars and phrases are derived from the grid.
* Change point detection on the loudness envelope, marking where the song's energy regime changes.
* Waveform peaks for display, and loudness data for the editor's meters.
* Song analysis runs on the processor, deliberately. The workload is seconds, and a graphics path for it would add complexity where nothing is gained. Knowing where acceleration does not pay is part of the design.

### 8.2 Footage analysis, staged

Analysis lands in stages, so the Cut page is never a waiting room.

* **Stage 0, probing.** On import, milliseconds per file. Container metadata only, no decoding at all.
* **Stage 1, the quick pass.** Seconds per file. A sparse sampling pass at a few frames per second and small frame size. Seeking lands on keyframes, so the pass decodes forward briefly after each seek; that is measured behaviour of real decoders, not an assumption. From this one pass come filmstrips, coarse scene boundaries (accurate within a few frames, which is fine at this stage), black ranges, and a rough motion estimate. Its work is not wasted: it makes the filmstrips the interface needs anyway.
* **Stage 2, the full pass.** One single full decode per file, frames analysed by the Program's own graphics shaders, emitting results chunk by chunk as the decode advances: exact scene boundaries, precise black ranges, motion curves, best moments. Progress ends at exactly 1.0, and the user watches numbers move, because progress is reported per chunk from the Program's own code, not inferred from an external process.

### 8.3 The refinement contract

* **Breadth before depth.** Stage 1 covers every video before Stage 2 begins on any of them, so the rough cut draws from the whole pool rather than whichever file finished first. Stage 2 then works in priority order, with footage actually used in the current cut analysed first.
* **Provisional and pinned.** The planner generates rough cut version one the moment the song grid and Stage 1 coverage exist, working from durations, coarse scenes and black ranges. As Stage 2 results land for a video, its boundaries snap to exact frames and its candidate scores improve, and the planner may replace provisional clips with better ones. Any clip the user has touched, moved, trimmed, locked or pinned is excluded from replacement, permanently, until the user unpins it. Refined results carry pinned and settled clips byte for byte.
* **Cancellation granularity.** Cancelling is per asset and per stage, and lands between stages, files and renders. The user is never trapped inside a single long operation.
* **Caching by content.** Every analysis result is cached under the file's content fingerprint, so reopening a project whose media is unchanged skips straight to done.

### 8.4 Scoring

Candidate windows in each source are scored on: alignment of the window's start with a scene boundary; avoidance of black ranges, which are excluded rather than merely penalised where they would produce a black clip; motion amount suited to the song's energy at that point; best moment data from the full pass; reuse penalties from section 7; and the user's trims, which are hard limits, not scores. The scoring code is a deliberate seam: today it computes on the processor over decoded frames, and its interface is exactly where a shader implementation later drops in without changing callers.

## 9. The planner

The planner is the montage brain, a separate component with one job: given the song grid, the analysis data and the generation settings, produce a timeline.

* It works in two modes against one settings object: a provisional plan from partial data, and a refined plan as data completes, under the contract of section 8.3.
* Experiments in the earlier attempt taught a structural lesson that is kept: when a settings type lives in the core model but lacks fields the planner needs (placement mode, beats per bar, spread and snap switches), the planner owns a wrapper settings type rather than quietly growing the core type. Borders are respected even when inconvenient.
* Pinned clips are inputs, not obstacles: the planner plans around them and never reports them as changed.
* The planner is pure with respect to its inputs. Same project snapshot, same analysis, same settings, same seed: same plan. This is what makes its behaviour testable and its surprises debuggable.

## 10. Composition, rendering and export

### 10.1 The owned and rented split

The engine decision, and the deepest one in the architecture: **composition is owned completely; translation is rented.**

Composition means everything that decides what the pixels are: compositing tracks, transitions, effects, masks, colour lookup tables, and the footage analysis pass. All of it is the Program's own code, running as its own shaders on the graphics processor, with a small processor based reference compositor beside it. Translation means decoding source files into frames and encoding finished frames into files. That is rented: mature codec libraries, called in process as libraries, behind the Program's own decoder and encoder traits. Never as external subprocess jobs, which was the earlier attempt's slowness and blindness, and never as a rented filter graph, which would hand the composition back to the library the Program deliberately does not trust with it.

If a rented tenant fails, the tenant is replaced behind its trait, not the engine. The one exception: if the owned compositor itself had failed its proof test, the whole split would have been reopened. It did not. A standalone spike rendered the hardest transition as a custom shader and matched its inputs pixel exactly at both ends, with the middle frame provably a real zoom and not a disguised fade, differing from a plain crossfade on about ninety five percent of pixels.

### 10.2 The two compositors and the oracle rule

* The **reference compositor** computes on the processor, slowly and obviously correctly. It is the oracle: the definition of right.
* The **graphics compositor** computes the same operations as shaders, fast. It is correct only insofar as it agrees with the oracle. The agreed tolerance, verified by automated comparison on real frames: a maximum difference of one channel step per channel, with a low mean. Any shader work that cannot pass oracle comparison does not ship.
* This pattern was taken from the large reference editor studied during research, which keeps exactly such a reference implementation. It converts "the graphics path looks right" from an opinion into a test.

### 10.3 Transitions

Eleven named transitions plus the hard cut, each implemented as its own genuine form in both compositors: Cross Dissolve; Dip to Color, to black or to white; Film Dissolve, which dissolves in linear light and so looks different from a plain dissolve in a way the user can see; Dip to Dissolve, a brightness dipping approximation whose approximation is documented in the code rather than hidden; Crash Zoom; Directional Zoom; Twist Wheel; Radial Twist; Push; Slide; Split. The honest note from the earlier attempt stands as a rule: a previous renderer leaned on a codec library's built in transition filters and silently delivered fades where zooms were promised, because that library's filter set lacked the zoom forms. Owning the compositor is what makes this list a promise instead of a hope.

### 10.4 Effects

The target effect vocabulary, organised as per clip stacks and adjustment layers: zoom, shake, stutter, strobe, RGB shift, glitch, tunnel, invert, echo, hue shift, flash, beat pulse, colour looks, and split screen layouts. The first foundation build implements a smaller honest core on the processor path: brightness, contrast, saturation, blur and sharpen, each with a canonical text description used in cache keys, so that equal stacks always describe themselves identically. Effects graduate from the processor set to shaders under the oracle rule.

### 10.5 The render cache

Modelled on the professional editor whose cache the planning studied most closely.

* While the user edits, an idle background worker renders timeline clips with their properties, keyframes and effects baked in, plus the transition regions spanning clip boundaries, into high quality cache files.
* Every unit is **content addressed**: its key is a hash of everything that can change its pixels, including source identity, ranges, properties, effect stack descriptions, transition kind and the machine's capability report. Undoing an edit therefore restores the earlier cached state instead of discarding it, because the earlier key simply becomes current again.
* Cache state is visible on the timeline, per clip and per transition region.
* Export consumes finished units and renders only what is missing or stale. A bypass option renders everything from the sources for a clean full quality pass.
* The first foundation build implements the content addressed cache and a sequential export path (one contiguous run of items, hard cuts, gaps rejected with no file written) as its honest subset. The background worker, the queue and transition region rendering are the specified target the subset grows into.

### 10.6 Encoding

* The target output matrix: H.264, H.265, AV1 and VP9, in MP4, MKV, WebM and MOV. The foundation default is 1920 by 1080 at 30 frames per second, MP4, H.264.
* Hardware encoders are detected **by trial encode**, never merely by name: the Program attempts a real short encode and believes only success. The vendor blocks, in order per machine: NVENC on NVIDIA, QuickSync on Intel, AMF on AMD, VideoToolbox on Apple systems. Software encoding is the floor.
* A resolved name rule, learned from a real build: the installed codec library registered no encoder literally named for plain H.264, only its well known software implementations and hardware wrappers. The encoder lookup therefore tries the requested name, falls back to the first registered encoder for that codec, and always reports the resolved name to the interface and the user. Substitution is allowed; silence about it is not.
* Exports write to a temporary file and rename on completion.

## 11. Architecture

### 11.1 Language

Rust, chosen by scoring candidates against five criteria set before scoring: performance; easy to code and maintain, with language models and human contributors alike; relatively bug free and easy to fix bugs in; easy to add features out of the blue; stable on most modern hardware. The reasoning, in brief: for a codebase largely written with model assistance, a strict compiler is a permanent machine reviewer that never sleeps, and a large reference editor in the same language is an existence proof at scale. C++ failed the bug criteria. C# was the runner up and lost on the performance core and native graphics story. Go and Python failed the performance core. TypeScript was interface layer only. Zig was too young. The honest costs are accepted and mitigated: compile times, mitigated by many small modules; and the steepest learning curve, mitigated by the compiler's own error teaching. One second language is sanctioned: WGSL, for graphics shaders, and nothing else.

### 11.2 Module map

The Program is a workspace of modules with one job each. Names here are functional, not branded.

* **Core.** The timeline model and project format: assets, tracks, items, transitions as data, effect stacks, settings, markers, multiple songs. The fixed tick time type. All edits as commands. Snapshot undo and redo history. Persistence. Nothing in Core knows about codecs, graphics or interface toolkits.
* **Audio.** Song decoding, onset and tempo analysis, bars, phrases, change points, waveform peaks, and the transcription drafting path. Depends on Core only for shared types.
* **Media.** Content fingerprinting, the staged footage analysis of section 8, filmstrips, and the analysis cache. Works through Platform for decoding.
* **Platform.** The sole home of hardware and operating system media access: the runtime capability report, decoder and encoder implementations, trial decode and trial encode. If code outside Platform names a codec library or a vendor API, the build is wrong.
* **Render.** The two compositors, transitions, effects, the content addressed render cache, and render jobs that produce finished files.
* **Engine.** The planner, the pipeline orchestration that runs song analysis, staged footage analysis and refinement as one cancellable job, and the progress and cancellation hooks every front end consumes.
* **App.** The desktop interface: four pages, jobs presentation, theme. The only module that may depend on the interface toolkit.
* **CLI.** A command line front end over the same engine, for scripted generation and export. It exists partly for users and partly as proof: if the CLI can drive it, the borders are honest.

### 11.3 Internal borders

* **Dependency direction is enforced by a tool**, an automatic workspace check that runs in the test suite, not by good intentions. Lower modules never import upward. Nothing below App depends on the interface toolkit, its windowing library, or its framework.
* **Edits are commands.** Every user action and every planner action is a command value applied by one path: clone the state, apply, validate, commit. Validation enforces the model invariants of section 12, so an invalid timeline cannot be constructed, let alone saved. Because edits are data, the engine is scriptable by design, and an agent facing interface in the style of modern tool protocols is a planned later addition over the same command model, not a retrofit.
* **Jobs are messages.** Long work runs as jobs communicating over channels, carrying progress and a cancellation token, consumed identically by the App, the CLI and the tests. The interface never blocks on a job.
* **Codec tenants sit behind traits.** Decoder and encoder are traits defined by the Program. Tenants, in order per machine and per file: platform native hardware paths first (Media Foundation with Direct3D 11 video acceleration on Windows, VideoToolbox on macOS, VA API on Linux), then a broad library fallback tenant for exotic files, then software as the floor.
* **No outward plugin border** exists in the foundation. Extensibility is the command model, not hosted third party code.

## 12. Data formats

* **Time.** Fixed integer ticks at 254,016,000,000 per second, over a signed 64 bit integer. All positions, durations and edit maths in ticks.
* **Project file.** A single readable JSON document, pretty printed, carrying a schema version and a forward migration chain: each schema version knows how to become the next, so old projects open forever. The foundation schema starts at version 1 with the tick rate, command model, invariants and content addressed cache already designed in.
* **Model invariants,** enforced at validation: item identifiers unique; items on a track sorted and never overlapping; durations positive; every item pointing at a known asset.
* **Media identity.** A file is identified by content fingerprint: its size plus a hash of sampled bytes from its head, middle and tail. The stored path is a hint for finding it, never its identity. Fingerprinting never modifies the user's files.
* **Derived data.** Analysis results and the render cache live outside the project file, in one known cache home, keyed by fingerprint and content hash respectively, always disposable, with a user facing purge that empties it. Deleting the entire cache home must cost the user nothing but recompute time.
* **Saves.** Atomic: write to a temporary file, then rename over the target, with a backup of the previous save kept beside the project.
* **Settings, presets and profiles** are plain data structures, shared identically by every front end, so the App and the CLI can never disagree about what a setting means.

## 13. Interface design

* **Toolkit strategy: hybrid.** An immediate mode graphical toolkit serves as the shell only, under three conditions locked during planning. One: hero surfaces are custom from the start, meaning the compositor preview presented as a graphics texture inside the shell, a hand drawn timeline, scopes, waveforms, meters and the keyframe curve editor; stock widgets carry ordinary chrome under a funded design system; and any section graduates to hand drawn when stock widgets cannot reach the design, cost too much repaint at real project sizes, or lack the needed input precision. Two: the front end stays replaceable, one interface module, every user action expressed as an engine command, so a future front end is a new module over the same engine. Three: two toolkits alive in one window at the same time is refused; floating windows inside the one toolkit are fine.
* **Theme.** Black and silver surfaces and text, teal as the primary action and highlight colour, a Material You design language, and subtle small corner radius everywhere, neither sharp nor strongly rounded. Page tabs only in the footer, wide, as fixed in section 6.
* **Background jobs everywhere.** Import probes, analysis, generation, rendering and export all run off the interface thread, with visible progress in the footer and on the page that owns the job.
* **Still open:** the design system detail beneath these rules, meaning the token values, the icon set and the widget kit inventory.

## 14. Hardware strategy

* **The capability report.** At startup the Program builds a report of what the machine actually offers: graphics adapters, hardware decode paths, hardware encoders proven by trial, and the codec library's registered capabilities. This report shapes the compositor's path choices, is mixed into cache keys (because the same edit may render differently on different paths, and the cache must never serve pixels computed under different rules as if equal), and informs the export planner.
* **Trial means trial.** A hardware decode is reported as working only when a frame genuinely arrives in the hardware pixel format. A probe that returns a promising name proves nothing; earlier work contained a probe function that was never even called, and its existence had been mistaken for capability. Do not repeat that.
* **Dual graphics laptops are a first class case.** On switchable graphics machines, the integrated processor's hardware decode, reached through the platform media APIs, may be the best path even when a discrete card is present, and the discrete card from one major vendor has, in its entry level form, no hardware encoder at all while still offering hardware decode. Selection is per machine, per file, measured, never assumed from a brand.
* **The floor test.** The engine must feel alive on the reference low end machine class: integrated plus entry discrete graphics, no hardware encoder, hardware decode only through platform APIs. If a feature cannot degrade to that machine gracefully, the feature is not finished.
* **Where acceleration is refused:** song analysis, per section 8.1. Restraint is part of the strategy.

## 15. Reliability, including the startup problem

The foundation build's most instructive failure happened before any window: on the reference low end laptop, the packaged app exited silently when clicked, no error, no window, no trace, while the command line tool from the same package ran perfectly, which proved the package and system runtimes were sound and isolated the fault to how the app starts its graphics. The app is a windowed binary with no console, so any failure in that moment is invisible by construction. The rules below are the permanent answer, written for any future front end.

* **Decide the renderer before creating any window.** The windowing layer permits exactly one event loop per process, a constraint proven the hard way when an in process retry design failed on it. So the app first probes the graphics stack with no window at all: if an adapter answers, it starts on the accelerated graphics renderer; if none answers, it falls back to the OpenGL renderer. An environment variable override can force either path for diagnosis.
* **Log from the first moments.** The app appends to a log file beside the executable, recording its version, the probe verdict and the adapter description, the chosen renderer, window creation, any panic (through a panic hook), any framework error, and clean exit. Each line opens and closes the file, so the last line survives a crash. Logging itself must never be able to panic; a diagnostic that can cause the failure it exists to explain is worse than none.
* **Fallbacks are paths, not apologies.** The fallback renderer is a full working path, exercised by automated live runs, not a degraded error screen. In the development sandbox, which offers no accelerated adapter at all, the entire application runs on the fallback path end to end: import, staged analysis to completion, page screenshots, clean exit, with the log narrating every step.
* **Honest status at the time of writing:** the packaged build containing these fixes still exits silently on the one real user machine, and diagnosis is waiting on that machine's log file. The architecture above is what makes that diagnosis a matter of reading one file instead of guessing. A plan should treat "works on the real low end Windows machine" as an explicit acceptance gate, not an assumption.

## 16. Building and releasing

* **Platform scope for now: Windows executables only.** Other platforms are not excluded by the architecture; they are excluded by the current owner's instruction, and the module borders keep them cheap to add later.
* **How the codec binding builds on Windows,** recorded because it cost real time: the binding's build script does not use the Unix package configuration system on Windows at all. It must be handed the codec library explicitly, through the FFMPEG_DLL_PATH variable pointing at the shared build's library folder (which selects dynamic linking), FFMPEG_INCLUDE_DIR pointing at its headers for the binding generator, and LIBCLANG_PATH pointing at an LLVM installation, whose libclang the generator requires. A local build attempted without these fails inside the binding's build script with the message that no linking method is set. That failure looks like a code error and is purely an environment error. The shared development build of the codec library, the same one the release pipeline downloads, provides exactly the include, library and binary folders this needs.
* **The release pipeline** runs on a real Windows runner: it downloads the pinned codec shared build, sets the environment above, runs the full workspace test suite as a gate, builds in release mode, stages an application folder containing both executables and the codec dynamic libraries, and ships exactly two downloads, a portable zip and an installer built with a standard setup compiler, plus the automatic source archives. A tag points at a commit on the main branch; there are no side branch releases, a workflow rule adopted after an early release was built from a side branch and read as hidden.
* **A second inherited lesson:** legacy continuous integration left over from an earlier incarnation of the project can run beside the real release gate and fail noisily for unrelated environment reasons, such as a plain Linux runner with no codec development libraries. Pipelines that are not the gate should be deleted, not explained forever.
* **Source archives are not the product.** They contain text only, so binary test fixtures cannot travel in them; the test suite therefore regenerates its one binary audio fixture from the same synthetic signal when the file is absent, which keeps source builds honest.

## 17. Testing and verification

* **Layers:** unit tests inside each module; integration tests across module borders, including planner determinism and the refine contract (refining must carry pinned and settled clips byte for byte); and live runs of the real application driven by environment hooks that import fixture media, select a page, run the pipeline and capture a screenshot, so interface claims are verified by pictures, not impressions.
* **Fixtures are synthetic and self describing:** a click train at an exactly known tempo for audio; short generated videos with known content for media; their generation is part of the test code, so the suite is reproducible anywhere.
* **The oracle comparisons** of section 10.2 are automated tests, as are the model invariants of section 12 and the layer enforcement of section 11.3.
* **Reporting discipline:** verification results are stated with their limits attached. Software rendered graphics prove shader correctness, not graphics performance. A sandbox with no real graphics hardware proves fallback paths, not accelerated ones. The final hardware proof is always a named real machine.

## 18. Licensing

* The Program itself is Apache 2.0.
* The rented codec library is the complication. A build of it that travels with the executables must respect its licence shape: the permissive configuration requires dynamic linking, which the packaging already does, and excludes the strongly copylefted encoder libraries, which the permissive configuration therefore cannot ship. The release pipeline currently stages a shared build whose configuration sits in tension with the project's own licence, and this is recorded openly rather than resolved quietly.
* **Open decision, belonging to the owner:** the licence and export floor decision, meaning which codec build configuration may ship, and consequently which software encoders form the guaranteed export floor on machines with no hardware encoder. The trial encode ladder and the resolved name rule exist partly so that either outcome of this decision can be implemented without redesign.

## 19. What the reference studies contributed

Nine programs were studied: single purpose generators in this niche, a free local editor in the CapCut class whose organisation and friendliness set the spirit, and a large open source Premiere class editor written almost entirely in Rust, at roughly a third of a million lines, which reimplements a professional editor's behaviour without the famous codec library inside it at all. From them, the Program takes judgement rather than code, except where licences plainly allow borrowing from the Rust editor: the proven generation controls of section 7; the cache model of section 10.5; the reference compositor oracle pattern; a command architecture demonstrated at a scale of hundreds of commands driven identically by interface, command line and external tools; enforced module layering as a tested property; and the scope warning that general editor parity costs thousands of hours while the music intelligence niche sits unoccupied. One widely repeated rumour about the large editor, that it was decompiled from the professional product it mirrors, was investigated and is false on all evidence; it is a clean room reimplementation, which is precisely why its patterns are safe to learn from.

## 20. Open decisions

These belong to the owner and are listed so a plan can mark them instead of silently choosing.

1. **Licence and export floor,** per section 18.
2. **Fallback codec tenant.** By the locked engine decision, a broad library tenant sits between platform native paths and software. The container binding spike has since run: exactly one candidate binding matched the pinned codec library version on label, built cleanly and decoded a real frame; a second candidate worked but carried version skew risk against its declared target; a third failed to compile and is out. The evidence backed candidate is therefore the on label binding, and it is what the foundation build uses, but the formal choice remains the owner's.
3. **Design system detail,** per section 13.
4. **Hardware proof on the real machine,** per section 15: the remaining platform spikes (native decode measurement on the low end machine class, the hardware trial matrix, scrub and cache behaviour) run guided on the owner's Windows machine.

## 21. Build order

The order the architecture dictates, and the order the foundation build in fact followed, with each slice independently verified before the next began:

1. Core: the tick time type, the model with invariants, commands, history, persistence.
2. Audio analysis, proven against the known tempo fixture.
3. The planner, with determinism and the refine contract proven in tests.
4. Platform: the capability report and trial based decoder and encoder.
5. Media: fingerprinting, then the staged analysis pipeline with its cache.
6. Render: the reference compositor first, as oracle; the graphics compositor against it; transitions and the first effects; the content addressed cache; a render job producing a real file.
7. The application: pages in order, Media, Cut, Edit, Export, over the finished engine, with the staged pipeline driving the Cut page.
8. The command line front end, thin over the same engine.
9. Packaging and release: the Windows pipeline of section 16.
10. Hardware proof and the remaining spikes on real machines, feeding corrections back into Platform.

Future directions, in the order planning left them, none of them promised: energy driven cut density, which would let the song's energy curve steer how densely cuts fall, parked by decision for a distant version; the agent facing interface over the command model; a mobile front end as a someday possibility, served by the same engine modules if it ever becomes a real goal.

## 22. Rules a plan must not violate

A concise checklist, extracted from the whole document, for whoever plans next.

1. Local disk import only. No external media service, no accounts, no uploads, ever.
2. Composition is owned; translation is rented behind traits; no codec work outside the Platform module; no external process decoding or encoding.
3. All edit maths in fixed integer ticks at 254,016,000,000 per second. No floating point seconds in positions or durations.
4. Every edit is a command applied by clone, validate, commit. Invalid timelines must be unconstructable.
5. Analysis is staged: probe on import, quick pass across the whole pool, then one full decode per file, reported per chunk. Rough cut early; refinement never touches a clip the user has touched.
6. Graphics first, never graphics only. Every graphics path has a correct software path, and the software compositor is the oracle the graphics compositor must match.
7. Hardware capability is proven by trials with real frames and real encodes, never by names or probes alone, and the resolved encoder name is always reported.
8. One project file, readable JSON, schema versioned with migrations; atomic saves with a backup; media identified by content fingerprint; all derived data in one disposable cache home with a user facing purge.
9. The interface is a shell over commands. Nothing below it depends on its toolkit. Page tabs live only in the footer. No control may pretend to work.
10. Do not reopen the refused list in section 5, and do not silently decide the open list in section 20. Both belong to the owner.