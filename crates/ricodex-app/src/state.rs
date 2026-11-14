//! App state and the state transitions of the staged pipeline, kept
//! free of the interface toolkit so they can be unit tested without a
//! window. The drawing code in `ui.rs` reads this state and feeds it
//! messages; the worker threads in `jobs.rs` produce those messages.
//!
//! The pipeline implemented here is the staged analysis pipeline
//! locked under Decision 2: Stage 0 probing on import, song analysis
//! first, Stage 1 for every video (breadth before depth), a rough
//! cut from Stage 1 data, then Stage 2 per video with the plan
//! refined after each one, pinned clips never replaced.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ricodex_audio::SongAnalysis;
use ricodex_core::{
    Asset, AssetId, AssetKind, Command, Fingerprint, History, ItemId, ItemProvenance, Project,
    Tick, Track, TrackKind,
};
use ricodex_engine::{ClipState, Plan, PlannedClip, PlannerSettings, SourceInfo};
use ricodex_media::{MediaProbe, SourceAnalysis};

/// The one video track the Cut page builds on in this slice.
pub const VIDEO_TRACK_ID: u64 = 1;

// ---------------------------------------------------------------- pages

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Media,
    Cut,
    Edit,
    Export,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Media, Page::Cut, Page::Edit, Page::Export];

    pub fn label(self) -> &'static str {
        match self {
            Page::Media => "Media",
            Page::Cut => "Cut",
            Page::Edit => "Edit",
            Page::Export => "Export",
        }
    }

    /// Parse the RICODEX_PAGE environment value.
    pub fn from_env(value: &str) -> Option<Page> {
        match value.trim().to_ascii_lowercase().as_str() {
            "media" => Some(Page::Media),
            "cut" => Some(Page::Cut),
            "edit" => Some(Page::Edit),
            "export" => Some(Page::Export),
            _ => None,
        }
    }
}

// -------------------------------------------------------------- job state

/// The state of one background job, driving the footer status and the
/// per page progress displays.
#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Idle,
    Running { label: String, fraction: f32 },
    Done,
    Failed { message: String },
    Cancelled,
}

impl JobState {
    pub fn is_running(&self) -> bool {
        matches!(self, JobState::Running { .. })
    }

    /// Short footer text for this job.
    pub fn summary(&self) -> String {
        match self {
            JobState::Idle => "idle".to_string(),
            JobState::Running { label, fraction } => {
                format!("{label} {:.0}%", fraction * 100.0)
            }
            JobState::Done => "done".to_string(),
            JobState::Failed { message } => format!("failed: {message}"),
            JobState::Cancelled => "cancelled".to_string(),
        }
    }
}

// ----------------------------------------------------------------- pool

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
}

impl MediaKind {
    /// Classify an imported path by extension. Anything unknown is
    /// treated as video; its probe will fail honestly if it is not.
    pub fn from_path(path: &std::path::Path) -> MediaKind {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("wav" | "mp3" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus") => {
                MediaKind::Audio
            }
            _ => MediaKind::Video,
        }
    }
}

/// One imported file in the media pool.
#[derive(Clone, Debug)]
pub struct PoolEntry {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub asset_id: AssetId,
    /// Stage 0 is in flight for this entry.
    pub probing: bool,
    /// Stage 0 probe data, video files only.
    pub probe: Option<MediaProbe>,
    /// Duration once known, from the probe or from audio decoding.
    pub duration: Option<Tick>,
    pub probe_error: Option<String>,
    /// Per video intro and outro trims, respected by generation.
    pub intro_trim: Tick,
    pub outro_trim: Tick,
    pub stage1: Option<SourceAnalysis>,
    pub stage2: Option<SourceAnalysis>,
    /// Live Stage 2 progress for this entry, 0.0 to 1.0.
    pub stage2_fraction: f32,
}

impl PoolEntry {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

}

/// Build the engine's `SourceInfo` for one pool entry from its best
/// analysis, applying the intro and outro trims: scenes and black
/// ranges are clipped to the trimmed window and shifted so the
/// trimmed start is zero. Returns None before any analysis stage has
/// landed for the entry.
pub fn trimmed_source(
    asset_id: AssetId,
    analysis: &SourceAnalysis,
    intro: Tick,
    outro: Tick,
) -> SourceInfo {
    let window_start = intro;
    let window_end = analysis.duration - outro;
    let usable = window_end - window_start;
    let duration = if usable > Tick::ZERO { usable } else { Tick::ZERO };

    let clip_range = |start: Tick, len: Tick| -> Option<(Tick, Tick)> {
        let end = start + len;
        let s = start.max(window_start);
        let e = end.min(window_end);
        if e > s {
            Some((s - window_start, e - s))
        } else {
            None
        }
    };

    SourceInfo {
        asset_id,
        duration,
        scenes: analysis
            .scenes
            .iter()
            .filter_map(|scene| {
                clip_range(scene.start, scene.duration).map(|(start, len)| {
                    ricodex_engine::SceneInfo {
                        start,
                        duration: len,
                        motion: scene.motion,
                        best_moment: scene.best_moment,
                        coarse: scene.coarse,
                    }
                })
            })
            .collect(),
        black_ranges: analysis
            .black_ranges
            .iter()
            .filter_map(|range| {
                clip_range(range.start, range.duration).map(|(start, len)| {
                    ricodex_engine::BlackRange {
                        start,
                        duration: len,
                    }
                })
            })
            .collect(),
    }
}

// ------------------------------------------------------------- messages

/// The result of one Stage 0 probe job, sent back over a channel.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub index: usize,
    pub fingerprint: Option<Fingerprint>,
    pub duration: Option<Tick>,
    pub probe: Option<MediaProbe>,
    pub error: Option<String>,
}

/// Probe result while verifying or locating media referenced by a saved project.
#[derive(Clone, Debug)]
pub struct RelinkResult {
    pub index: usize,
    pub path: PathBuf,
    pub fingerprint: Option<Fingerprint>,
    pub duration: Option<Tick>,
    pub probe: Option<MediaProbe>,
    pub error: Option<String>,
}

/// One event from the pipeline worker thread.
#[derive(Clone, Debug)]
pub enum PipelineMsg {
    SongFinished { result: Result<SongAnalysis, String> },
    Stage1Started { index: usize },
    Stage1Finished {
        index: usize,
        result: Result<SourceAnalysis, String>,
    },
    Stage2Started { index: usize },
    Stage2Progress { index: usize, fraction: f32 },
    Stage2Finished {
        index: usize,
        result: Result<SourceAnalysis, String>,
    },
    PlanUpdated { plan: Plan },
    Finished,
    Cancelled,
    Failed { message: String },
}

/// One event from the export worker thread.
#[derive(Clone, Debug)]
pub enum ExportMsg {
    Progress { fraction: f32 },
    Finished {
        encoder_name: String,
        frames_written: u64,
        output_path: PathBuf,
    },
    Failed { message: String },
}

/// Which pipeline stage the Cut page is in, for progress and status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStage {
    Song,
    Stage1,
    Stage2,
}

/// Everything the pipeline worker needs, cloned out of the state at
/// start so the worker never touches the state itself.
#[derive(Clone, Debug)]
pub struct PipelineRequest {
    pub song_path: PathBuf,
    pub videos: Vec<PipelineVideo>,
    pub planner_settings: PlannerSettings,
    /// Current timeline snapshot used as the starting point for refinement.
    /// Pinned clips remain fixed; unpinned generated clips may be reconsidered.
    pub initial_plan: Option<Plan>,
    pub cache_home: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PipelineVideo {
    pub index: usize,
    pub asset_id: AssetId,
    pub path: PathBuf,
    pub intro_trim: Tick,
    pub outro_trim: Tick,
}

/// ETA from an overall fraction and the elapsed time: the remaining
/// share of the elapsed time, scaled. None when there is nothing
/// meaningful to estimate from yet.
pub fn eta_seconds(fraction: f32, elapsed_s: f64) -> Option<f64> {
    if !(0.02..1.0).contains(&fraction) || elapsed_s <= 0.0 {
        return None;
    }
    Some(elapsed_s * (1.0 - fraction as f64) / fraction as f64)
}

// ----------------------------------------------------------------- state

pub struct AppState {
    pub project: Project,
    /// Core command history shared by every editing control.
    pub history: History,
    /// True when the project contains manually edited or pinned clips.
    /// These clips are saved and can be protected during later refinement.
    pub has_manual_edits: bool,
    /// Current project file path, when the project has been saved or opened.
    pub project_path: Option<PathBuf>,
    /// True when project metadata or timeline contents differ from disk.
    pub is_dirty: bool,
    pub pool: Vec<PoolEntry>,
    pub page: Page,
    pub song_index: Option<usize>,
    pub song_analysis: Option<SongAnalysis>,
    pub plan: Option<Plan>,
    /// Clips the user has pinned; refinement must never replace them.
    /// Shared with the pipeline worker, which snapshots it per refine.
    pub pinned: Arc<Mutex<HashSet<ItemId>>>,
    pub selected_item: Option<ItemId>,
    pub probe_job: JobState,
    pub pipeline_stage: Option<PipelineStage>,
    pub pipeline_job: JobState,
    /// Pool indices taking part in the current pipeline run.
    pub pipeline_videos: Vec<usize>,
    /// The pool index whose analysis stage is in flight right now.
    pub stage_current: Option<usize>,
    pub pipeline_started: Option<Instant>,
    pub pipeline_cancel: Arc<AtomicBool>,
    pub export_job: JobState,
    pub export_path: Option<PathBuf>,
    pub export_result: Option<(String, u64, PathBuf)>,
    pub export_cancel: Arc<AtomicBool>,
    /// A small line of user facing feedback (a rejected plan, a
    /// failed file), shown on the page it belongs to.
    pub notice: Option<String>,
    next_asset_id: AssetId,
}

impl AppState {
    pub fn new() -> AppState {
        let mut project = Project::new();
        project.add_track(Track::new(VIDEO_TRACK_ID, TrackKind::Video, "V1"));
        let history = History::new(project.clone());
        AppState {
            project,
            history,
            has_manual_edits: false,
            project_path: None,
            is_dirty: false,
            pool: Vec::new(),
            page: Page::Media,
            song_index: None,
            song_analysis: None,
            plan: None,
            pinned: Arc::new(Mutex::new(HashSet::new())),
            selected_item: None,
            probe_job: JobState::Idle,
            pipeline_stage: None,
            pipeline_job: JobState::Idle,
            pipeline_videos: Vec::new(),
            stage_current: None,
            pipeline_started: None,
            pipeline_cancel: Arc::new(AtomicBool::new(false)),
            export_job: JobState::Idle,
            export_path: None,
            export_result: None,
            export_cancel: Arc::new(AtomicBool::new(false)),
            notice: None,
            next_asset_id: 1,
        }
    }

    // ------------------------------------------------------------ pool

    /// Register an imported file in the pool and in the project.
    /// Returns the pool index. Importing the same path twice returns
    /// the existing entry. This only registers; the caller spawns
    /// the Stage 0 probe job for the returned index.
    pub fn import_path(&mut self, path: PathBuf) -> usize {
        if let Some(index) = self.pool.iter().position(|e| e.path == path) {
            return index;
        }
        let kind = MediaKind::from_path(&path);
        let asset_id = self.next_asset_id;
        self.next_asset_id += 1;
        self.project.assets.push(Asset {
            id: asset_id,
            fingerprint: Fingerprint::new(0, 0, 0, 0),
            path_hint: path.to_string_lossy().into_owned(),
            kind: match kind {
                MediaKind::Video => AssetKind::Video,
                MediaKind::Audio => AssetKind::Audio,
            },
            duration: Tick::ZERO,
            intro_trim: Tick::ZERO,
            outro_trim: Tick::ZERO,
        });
        self.pool.push(PoolEntry {
            path,
            kind,
            asset_id,
            probing: true,
            probe: None,
            duration: None,
            probe_error: None,
            intro_trim: Tick::ZERO,
            outro_trim: Tick::ZERO,
            stage1: None,
            stage2: None,
            stage2_fraction: 0.0,
        });
        // Import changes project metadata outside the edit-command history.
        self.history = History::new(self.project.clone());
        self.is_dirty = true;
        if !self.probe_job.is_running() {
            self.probe_job = JobState::Running {
                label: "probing".to_string(),
                fraction: 0.0,
            };
        }
        self.pool.len() - 1
    }

    pub fn handle_probe_result(&mut self, result: ProbeResult) {
        let Some(entry) = self.pool.get_mut(result.index) else {
            return;
        };
        entry.probing = false;
        entry.duration = result.duration;
        entry.probe = result.probe;
        entry.probe_error = result.error;
        let asset_id = entry.asset_id;
        let fingerprint = result.fingerprint;
        let duration = result.duration;
        if let Some(asset) = self.project.assets.iter_mut().find(|a| a.id == asset_id) {
            if let Some(fingerprint) = fingerprint {
                asset.fingerprint = fingerprint;
            }
            if let Some(duration) = duration {
                asset.duration = duration;
            }
        }
        // Probe metadata is part of the project snapshot as well.
        self.history = History::new(self.project.clone());
        self.is_dirty = true;
        if !self.pool.iter().any(|e| e.probing) {
            self.probe_job = JobState::Done;
        }
    }

    /// Rebuild the Media pool from a loaded project. Existing files are
    /// fingerprint-checked on worker threads; missing files remain in the
    /// pool with an explicit Locate action instead of being discarded.
    pub fn restore_project(
        &mut self,
        project: Project,
        project_path: PathBuf,
    ) -> Vec<(usize, PathBuf, MediaKind)> {
        self.project = project;
        self.project_path = Some(project_path);
        self.is_dirty = false;
        self.pool.clear();
        self.plan = None;
        self.song_analysis = None;
        self.pipeline_stage = None;
        self.pipeline_videos.clear();
        self.stage_current = None;
        self.pipeline_job = JobState::Idle;
        self.export_job = JobState::Idle;
        self.export_result = None;
        self.selected_item = None;
        self.pinned = Arc::new(Mutex::new(HashSet::new()));

        self.song_index = self.project.song_asset_id.and_then(|song_id| {
            self.project.assets.iter().position(|asset| asset.id == song_id)
        });
        self.refresh_clip_flags();
        self.next_asset_id = self
            .project
            .assets
            .iter()
            .map(|asset| asset.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);

        let mut probes = Vec::new();
        for asset in &self.project.assets {
            let index = self.pool.len();
            let path = PathBuf::from(&asset.path_hint);
            let kind = match asset.kind {
                AssetKind::Audio => MediaKind::Audio,
                AssetKind::Video | AssetKind::Image => MediaKind::Video,
            };
            let exists = path.is_file();
            self.pool.push(PoolEntry {
                path: path.clone(),
                kind,
                asset_id: asset.id,
                probing: exists,
                probe: None,
                duration: (asset.duration > Tick::ZERO).then_some(asset.duration),
                probe_error: (!exists).then(|| "media file is missing; locate the original file".to_string()),
                intro_trim: asset.intro_trim,
                outro_trim: asset.outro_trim,
                stage1: None,
                stage2: None,
                stage2_fraction: 0.0,
            });
            if exists {
                probes.push((index, path, kind));
            }
        }

        self.refresh_clip_flags();
        self.history = History::new(self.project.clone());
        self.page = Page::Media;
        self.probe_job = if probes.is_empty() {
            JobState::Done
        } else {
            JobState::Running {
                label: "verifying media".to_string(),
                fraction: 0.0,
            }
        };
        probes
    }

    pub fn begin_relink_probe(&mut self, index: usize) -> Option<MediaKind> {
        let entry = self.pool.get_mut(index)?;
        if entry.probing {
            return None;
        }
        entry.probing = true;
        entry.probe_error = None;
        self.probe_job = JobState::Running {
            label: "verifying media".to_string(),
            fraction: 0.0,
        };
        Some(entry.kind)
    }

    /// Accept a located file only when its content fingerprint matches the
    /// asset saved in the project. Paths may change; source identity may not.
    pub fn handle_relink_result(&mut self, result: RelinkResult) {
        let Some(entry) = self.pool.get_mut(result.index) else {
            return;
        };
        let asset_id = entry.asset_id;
        let expected = self
            .project
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .map(|asset| asset.fingerprint);
        let blank = Fingerprint::new(0, 0, 0, 0);
        let matches = match (expected, result.fingerprint) {
            (Some(saved), Some(actual)) if saved != blank => saved == actual,
            (Some(saved), Some(_)) if saved == blank => result.path == entry.path,
            _ => false,
        };

        entry.probing = false;
        if !matches {
            entry.probe = None;
            entry.probe_error = Some(if expected == Some(blank) {
                "project asset has no saved media fingerprint; restore the source to its original saved path".to_string()
            } else {
                "selected file does not match the saved media fingerprint".to_string()
            });
        } else {
            let previous_path = entry.path.clone();
            entry.path = result.path.clone();
            entry.duration = result.duration.or(entry.duration);
            entry.probe = result.probe;
            entry.probe_error = result.error;
            if let Some(asset) = self.project.assets.iter_mut().find(|asset| asset.id == asset_id) {
                asset.path_hint = result.path.to_string_lossy().into_owned();
                if asset.fingerprint == blank {
                    if let Some(fingerprint) = result.fingerprint {
                        asset.fingerprint = fingerprint;
                    }
                }
                if let Some(duration) = result.duration {
                    asset.duration = duration;
                }
            }
            if previous_path != result.path || expected == Some(blank) {
                self.is_dirty = true;
                self.history = History::new(self.project.clone());
            }
        }
        if !self.pool.iter().any(|pool_entry| pool_entry.probing) {
            self.probe_job = JobState::Done;
        }
    }

    pub fn save_project_to(&mut self, path: PathBuf) -> Result<(), String> {
        self.project.save(&path).map_err(|error| error.to_string())?;
        self.project_path = Some(path);
        self.is_dirty = false;
        Ok(())
    }

    /// Whether this asset has a saved content fingerprint that can
    /// safely verify a replacement file during relinking.
    pub fn can_relink_asset(&self, index: usize) -> bool {
        let Some(entry) = self.pool.get(index) else {
            return false;
        };
        let blank = Fingerprint::new(0, 0, 0, 0);
        self.project
            .assets
            .iter()
            .find(|asset| asset.id == entry.asset_id)
            .map(|asset| asset.fingerprint != blank)
            .unwrap_or(false)
    }

    pub fn video_indices(&self) -> Vec<usize> {
        self.pool
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == MediaKind::Video)
            .map(|(i, _)| i)
            .collect()
    }

    /// Choose which imported audio file is the song. Rejects video
    /// entries and out of range indices.
    pub fn set_song(&mut self, index: usize) -> bool {
        let Some(entry) = self.pool.get(index) else {
            return false;
        };
        if entry.kind != MediaKind::Audio || entry.probing || entry.probe_error.is_some() {
            return false;
        }
        if self.has_manual_edits && self.project.song_asset_id.is_some() {
            return self.song_index == Some(index);
        }
        self.song_index = Some(index);
        self.project.song_asset_id = Some(entry.asset_id);
        self.history = History::new(self.project.clone());
        self.is_dirty = true;
        true
    }

    /// Set the intro and outro trims of one video, in seconds,
    /// clamped so the pair never eats the whole file once its
    /// duration is known.
    pub fn set_trims_seconds(&mut self, index: usize, intro_s: f64, outro_s: f64) {
        if !self.can_edit_source_trims() {
            return;
        }
        let Some(entry) = self.pool.get_mut(index) else {
            return;
        };
        let mut intro = Tick::from_seconds(intro_s.max(0.0));
        let mut outro = Tick::from_seconds(outro_s.max(0.0));
        if let Some(duration) = entry.duration {
            // Keep at least a tenth of a second of usable footage.
            let budget = duration - Tick::from_seconds(0.1);
            if budget > Tick::ZERO && intro + outro > budget {
                let excess = (intro + outro - budget).to_seconds();
                let cut = excess / 2.0;
                intro = Tick::from_seconds((intro.to_seconds() - cut).max(0.0));
                outro = Tick::from_seconds((outro.to_seconds() - cut).max(0.0));
            }
        }
        entry.intro_trim = intro;
        entry.outro_trim = outro;
        let asset_id = entry.asset_id;
        if let Some(asset) = self.project.assets.iter_mut().find(|asset| asset.id == asset_id) {
            asset.intro_trim = intro;
            asset.outro_trim = outro;
        }
        self.history = History::new(self.project.clone());
        self.is_dirty = true;
    }

    // ---------------------------------------------------------- pinning

    pub fn is_pinned(&self, item_id: ItemId) -> bool {
        self.project.item(item_id).map(|item| item.pinned).unwrap_or(false)
    }

    pub fn toggle_pin(&mut self, item_id: ItemId) {
        let Some(item) = self.project.item(item_id) else {
            return;
        };
        let command = Command::SetItemPinned {
            item_id,
            pinned: !item.pinned,
        };
        if let Err(error) = self.apply_edit(&command) {
            self.notice = Some(error);
        }
    }

    /// Rebuild the shared worker pin set and the UI summary from persisted
    /// item flags. This is called after loading, edits, undo/redo and
    /// planner updates so disk state remains authoritative.
    fn refresh_clip_flags(&mut self) {
        let mut pinned = HashSet::new();
        let mut has_manual_edits = false;
        for item in self.project.track_items.iter().flat_map(|group| group.items.iter()) {
            if item.pinned {
                pinned.insert(item.id);
            }
            if item.pinned || item.provenance == ItemProvenance::Manual {
                has_manual_edits = true;
            }
        }
        self.has_manual_edits = has_manual_edits;
        if let Ok(mut shared) = self.pinned.lock() {
            *shared = pinned;
        }
    }

    // --------------------------------------------------------- pipeline

    pub fn can_begin_analysis(&self) -> bool {
        let song_ready = self
            .song_index
            .and_then(|index| self.pool.get(index))
            .map(|entry| !entry.probing && entry.probe_error.is_none() && entry.duration.map(|d| d > Tick::ZERO).unwrap_or(false))
            .unwrap_or(false);
        let video_ready = self.video_indices().iter().any(|index| {
            self.pool
                .get(*index)
                .map(|entry| !entry.probing && entry.probe_error.is_none() && entry.duration.map(|d| d > Tick::ZERO).unwrap_or(false))
                .unwrap_or(false)
        });
        song_ready
            && video_ready
            && !self.pipeline_job.is_running()
    }

    /// Clone out everything the pipeline worker needs. Also resets
    /// the per run counters and marks the song stage as running.
    /// Returns None when analysis cannot begin.
    pub fn begin_analysis(&mut self, cache_home: PathBuf) -> Option<PipelineRequest> {
        if !self.can_begin_analysis() {
            return None;
        }
        let song_index = self.song_index?;
        let videos: Vec<PipelineVideo> = self
            .video_indices()
            .into_iter()
            .filter_map(|index| {
                let entry = self.pool.get(index)?;
                (!entry.probing && entry.probe_error.is_none() && entry.duration.map(|d| d > Tick::ZERO).unwrap_or(false))
                    .then(|| PipelineVideo {
                        index,
                        asset_id: entry.asset_id,
                        path: entry.path.clone(),
                        intro_trim: entry.intro_trim,
                        outro_trim: entry.outro_trim,
                    })
            })
            .collect();
        let current_items = self
            .project
            .items(VIDEO_TRACK_ID)
            .cloned()
            .unwrap_or_default();
        let initial_plan = if current_items.is_empty() {
            None
        } else {
            let clips: Vec<PlannedClip> = current_items
                .iter()
                .map(|item| PlannedClip {
                    item_id: item.id,
                    asset_id: item.asset_id,
                    start: item.start,
                    duration: item.duration,
                    source_in: item.source_in,
                    state: if item.pinned {
                        ClipState::Settled
                    } else {
                        ClipState::Provisional
                    },
                })
                .collect();
            let commands = current_items
                .iter()
                .map(|item| Command::InsertItem {
                    track_id: VIDEO_TRACK_ID,
                    item: item.clone(),
                })
                .collect();
            Some(Plan {
                track_id: VIDEO_TRACK_ID,
                clips,
                commands,
            })
        };
        self.pipeline_videos = videos.iter().map(|v| v.index).collect();
        for index in &self.pipeline_videos {
            if let Some(entry) = self.pool.get_mut(*index) {
                entry.stage2_fraction = 0.0;
            }
        }
        self.pipeline_cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.pipeline_stage = Some(PipelineStage::Song);
        self.stage_current = None;
        self.pipeline_started = Some(Instant::now());
        self.pipeline_job = JobState::Running {
            label: "analysing song".to_string(),
            fraction: 0.0,
        };
        self.page = Page::Cut;
        Some(PipelineRequest {
            song_path: self.pool[song_index].path.clone(),
            videos,
            planner_settings: PlannerSettings {
                generation: self.project.generation_settings.clone(),
                ..PlannerSettings::default()
            },
            initial_plan,
            cache_home,
        })
    }

    /// Ask the running pipeline to stop at the next stage boundary.
    /// The worker confirms by sending Cancelled; until then the job
    /// still shows as running.
    pub fn request_pipeline_cancel(&self) {
        self.pipeline_cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn pipeline_running(&self) -> bool {
        self.pipeline_job.is_running()
    }

    fn stage1_done_count(&self) -> usize {
        self.pipeline_videos
            .iter()
            .filter(|i| self.pool[**i].stage1.is_some())
            .count()
    }

    fn stage2_done_count(&self) -> usize {
        self.pipeline_videos
            .iter()
            .filter(|i| self.pool[**i].stage2.is_some())
            .count()
    }

    /// Overall pipeline fraction: song 0 to 0.1, Stage 1 breadth
    /// 0.1 to 0.4, Stage 2 depth 0.4 to 1.0.
    pub fn pipeline_fraction(&self) -> f32 {
        match self.pipeline_stage {
            None => {
                if self.plan.is_some() {
                    1.0
                } else {
                    0.0
                }
            }
            Some(PipelineStage::Song) => 0.05,
            Some(PipelineStage::Stage1) => {
                let total = self.pipeline_videos.len().max(1) as f32;
                0.1 + 0.3 * (self.stage1_done_count() as f32 / total)
            }
            Some(PipelineStage::Stage2) => {
                let total = self.pipeline_videos.len().max(1) as f32;
                let progress: f32 = self
                    .pipeline_videos
                    .iter()
                    .map(|i| {
                        let entry = &self.pool[*i];
                        if entry.stage2.is_some() {
                            1.0
                        } else {
                            entry.stage2_fraction
                        }
                    })
                    .sum();
                0.4 + 0.6 * (progress / total)
            }
        }
    }

    pub fn pipeline_eta_seconds(&self) -> Option<f64> {
        if !self.pipeline_running() {
            return None;
        }
        let started = self.pipeline_started?;
        eta_seconds(self.pipeline_fraction(), started.elapsed().as_secs_f64())
    }

    /// The Cut page status line, in the locked wording family.
    pub fn cut_status_line(&self) -> String {
        if let JobState::Failed { message } = &self.pipeline_job {
            return format!("analysis failed: {message}");
        }
        if matches!(self.pipeline_job, JobState::Cancelled) {
            return "analysis cancelled".to_string();
        }
        match self.pipeline_stage {
            Some(PipelineStage::Song) => "analysing song".to_string(),
            Some(PipelineStage::Stage1) => format!(
                "quick pass {} of {}",
                self.stage1_done_count(),
                self.pipeline_videos.len()
            ),
            Some(PipelineStage::Stage2) => {
                if self.plan.is_some() {
                    format!(
                        "rough cut ready, refining {} of {} videos",
                        self.stage2_done_count(),
                        self.pipeline_videos.len()
                    )
                } else {
                    format!(
                        "refining {} of {} videos",
                        self.stage2_done_count(),
                        self.pipeline_videos.len()
                    )
                }
            }
            None => {
                if self.pipeline_job == JobState::Done {
                    "analysis complete".to_string()
                } else if self.plan.is_some() {
                    "rough cut ready".to_string()
                } else {
                    "no analysis yet, begin it from the Media page".to_string()
                }
            }
        }
    }

    fn refresh_pipeline_job(&mut self) {
        if !self.pipeline_job.is_running() {
            return;
        }
        let label = match self.pipeline_stage {
            Some(PipelineStage::Song) => "analysing song",
            Some(PipelineStage::Stage1) => "quick pass",
            Some(PipelineStage::Stage2) => "refining",
            None => "analysis",
        };
        self.pipeline_job = JobState::Running {
            label: label.to_string(),
            fraction: self.pipeline_fraction(),
        };
    }

    pub fn handle_pipeline_msg(&mut self, msg: PipelineMsg) {
        match msg {
            PipelineMsg::SongFinished { result } => match result {
                Ok(analysis) => {
                    self.song_analysis = Some(analysis);
                    self.pipeline_stage = Some(PipelineStage::Stage1);
                }
                Err(message) => {
                    self.pipeline_job = JobState::Failed { message };
                    self.pipeline_stage = None;
                    self.stage_current = None;
                    return;
                }
            },
            PipelineMsg::Stage1Started { index } => {
                self.stage_current = Some(index);
            }
            PipelineMsg::Stage1Finished { index, result } => {
                if let Ok(analysis) = result {
                    if let Some(entry) = self.pool.get_mut(index) {
                        entry.stage1 = Some(analysis);
                    }
                } else if let Err(message) = result {
                    if let Some(entry) = self.pool.get_mut(index) {
                        entry.probe_error = Some(format!("quick pass failed: {message}"));
                    }
                }
                // The stage stays Stage 1 until the rough cut lands:
                // PlanUpdated is what moves the run into refinement.
            }
            PipelineMsg::Stage2Started { index } => {
                self.stage_current = Some(index);
            }
            PipelineMsg::Stage2Progress { index, fraction } => {
                if let Some(entry) = self.pool.get_mut(index) {
                    entry.stage2_fraction = fraction.clamp(0.0, 1.0);
                }
            }
            PipelineMsg::Stage2Finished { index, result } => match result {
                Ok(analysis) => {
                    if let Some(entry) = self.pool.get_mut(index) {
                        entry.stage2 = Some(analysis);
                        entry.stage2_fraction = 1.0;
                    }
                }
                Err(message) => {
                    if let Some(entry) = self.pool.get_mut(index) {
                        entry.probe_error = Some(format!("full pass failed: {message}"));
                    }
                }
            },
            PipelineMsg::PlanUpdated { plan } => {
                if self.pipeline_stage == Some(PipelineStage::Stage1) {
                    self.pipeline_stage = Some(PipelineStage::Stage2);
                }
                self.install_plan(plan);
            }
            PipelineMsg::Finished => {
                self.pipeline_stage = None;
                self.stage_current = None;
                self.pipeline_job = JobState::Done;
                return;
            }
            PipelineMsg::Cancelled => {
                self.pipeline_stage = None;
                self.stage_current = None;
                self.pipeline_job = JobState::Cancelled;
                return;
            }
            PipelineMsg::Failed { message } => {
                self.pipeline_job = JobState::Failed { message };
                self.pipeline_stage = None;
                self.stage_current = None;
                return;
            }
        }
        self.refresh_pipeline_job();
    }

    fn install_plan(&mut self, mut plan: Plan) {
        // Planner SourceInfo coordinates are rebased to the trimmed source
        // window. Core Items and the renderer use absolute positions in the
        // original media file, so restore the asset intro offset here.
        let intro_offset = |asset_id| {
            self.project
                .assets
                .iter()
                .find(|asset| asset.id == asset_id)
                .map(|asset| asset.intro_trim)
                .unwrap_or(Tick::ZERO)
        };
        for clip in &mut plan.clips {
            clip.source_in = clip.source_in + intro_offset(clip.asset_id);
        }
        for command in &mut plan.commands {
            if let Command::InsertItem { item, .. } = command {
                item.source_in = item.source_in + intro_offset(item.asset_id);
            }
        }

        let previous_metadata: HashMap<ItemId, (ItemProvenance, bool)> = self
            .project
            .items(VIDEO_TRACK_ID)
            .into_iter()
            .flatten()
            .map(|item| (item.id, (item.provenance, item.pinned)))
            .collect();
        let mut base = self.project.clone();
        if let Some(items) = base.items_mut(VIDEO_TRACK_ID) {
            items.clear();
        }
        match plan.apply(base) {
            Ok(mut project) => {
                if let Some(items) = project.items_mut(VIDEO_TRACK_ID) {
                    for item in items {
                        if let Some((provenance, pinned)) = previous_metadata.get(&item.id) {
                            item.provenance = *provenance;
                            item.pinned = *pinned;
                        }
                    }
                }
                self.project = project;
                self.refresh_clip_flags();
                self.is_dirty = true;
                // The newest generated plan is the baseline for future edits.
                self.history = History::new(self.project.clone());
                self.plan = Some(plan);
                if let Some(selected) = self.selected_item {
                    if self.project.item(selected).is_none() {
                        self.selected_item = None;
                    }
                }
            }
            Err(e) => {
                self.notice = Some(format!("planner output rejected: {e}"));
            }
        }
    }

    // ------------------------------------------------------------ clips

    /// The current project clips, in timeline order. The project is the
    /// source of truth after user edits; planner state only decorates clips
    /// that still correspond to generated items.
    pub fn timeline_clips(&self) -> Vec<PlannedClipView> {
        self.project
            .items(VIDEO_TRACK_ID)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let state = self
                            .plan
                            .as_ref()
                            .and_then(|plan| plan.clips.iter().find(|clip| clip.item_id == item.id))
                            .map(|clip| clip.state)
                            .unwrap_or(ClipState::Settled);
                        PlannedClipView {
                            item_id: item.id,
                            asset_id: item.asset_id,
                            start: item.start,
                            duration: item.duration,
                            source_in: item.source_in,
                            state,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Apply a UI edit through the shared Core command and history path.
    /// Core validates the candidate before the project snapshot is replaced.
    pub fn apply_edit(&mut self, command: &Command) -> Result<(), String> {
        if self.pipeline_running() {
            return Err("editing is disabled while analysis is running".to_string());
        }
        self.history.apply(command).map_err(|error| error.to_string())?;
        if let Command::InsertItem { item, .. } = command {
            // Explicit app-level inserts are user content, not planner output.
            self.history
                .apply(&Command::SetItemPinned { item_id: item.id, pinned: true })
                .map_err(|error| error.to_string())?;
        }
        self.project = self.history.project().clone();
        self.refresh_clip_flags();
        self.is_dirty = true;

        match command {
            Command::RippleRemoveItem { item_id } | Command::RemoveItem { item_id } => {
                if self.selected_item == Some(*item_id) {
                    self.selected_item = None;
                }
            }
            _ => {}
        }
        self.notice = None;
        Ok(())
    }

    pub fn can_undo_edit(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo_edit(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_edit(&mut self) -> bool {
        if self.pipeline_running() || !self.history.undo() {
            return false;
        }
        self.project = self.history.project().clone();
        if self.selected_item.map(|id| self.project.item(id).is_none()).unwrap_or(false) {
            self.selected_item = None;
        }
        self.refresh_clip_flags();
        self.is_dirty = true;
        true
    }

    pub fn redo_edit(&mut self) -> bool {
        if self.pipeline_running() || !self.history.redo() {
            return false;
        }
        self.project = self.history.project().clone();
        if self.selected_item.map(|id| self.project.item(id).is_none()).unwrap_or(false) {
            self.selected_item = None;
        }
        self.refresh_clip_flags();
        self.is_dirty = true;
        true
    }

    pub fn asset_path(&self, asset_id: AssetId) -> Option<&std::path::Path> {
        self.project
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .map(|asset| std::path::Path::new(&asset.path_hint))
    }

    pub fn asset_duration(&self, asset_id: AssetId) -> Option<Tick> {
        self.project
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
            .map(|asset| asset.duration)
    }

    /// The usable source interval after intro/outro trims. Unknown duration
    /// returns None so the UI can retain an unconstrained provisional range.
    pub fn asset_source_window(&self, asset_id: AssetId) -> Option<(Tick, Tick)> {
        let asset = self.project.assets.iter().find(|asset| asset.id == asset_id)?;
        if asset.duration <= Tick::ZERO {
            return None;
        }
        Some((asset.intro_trim, asset.duration - asset.outro_trim))
    }

    /// Source trims can change only before any timeline items exist.
    pub fn can_edit_source_trims(&self) -> bool {
        !self.pipeline_running()
            && !self.has_manual_edits
            && self.project.track_items.iter().all(|group| group.items.is_empty())
    }

    pub fn next_item_id(&self) -> Option<ItemId> {
        self.project
            .track_items
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.id))
            .max()
            .unwrap_or(0)
            .checked_add(1)
    }

    /// Total timeline length covered by the current clips.
    pub fn timeline_total(&self) -> Tick {
        self.timeline_clips()
            .iter()
            .map(|c| c.start + c.duration)
            .max()
            .unwrap_or(Tick::ZERO)
    }

    pub fn asset_name(&self, asset_id: AssetId) -> String {
        self.pool
            .iter()
            .find(|e| e.asset_id == asset_id)
            .map(|e| e.name())
            .unwrap_or_else(|| format!("asset {asset_id}"))
    }

    // ----------------------------------------------------------- export

    pub fn can_export(&self) -> bool {
        !self.export_job.is_running()
            && self
                .project
                .items(VIDEO_TRACK_ID)
                .map(|items| items.iter().any(|i| i.enabled))
                .unwrap_or(false)
    }

    /// Mark the export as started at `path`; the caller spawns the
    /// worker with a clone of the project.
    pub fn begin_export(&mut self, path: PathBuf) {
        self.export_cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.export_path = Some(path);
        self.export_result = None;
        self.export_job = JobState::Running {
            label: "exporting".to_string(),
            fraction: 0.0,
        };
    }

    pub fn handle_export_msg(&mut self, msg: ExportMsg) {
        match msg {
            ExportMsg::Progress { fraction } => {
                self.export_job = JobState::Running {
                    label: "exporting".to_string(),
                    fraction: fraction.clamp(0.0, 1.0),
                };
            }
            ExportMsg::Finished {
                encoder_name,
                frames_written,
                output_path,
            } => {
                if self
                    .export_cancel
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.export_job = JobState::Cancelled;
                } else {
                    self.export_result = Some((encoder_name, frames_written, output_path));
                    self.export_job = JobState::Done;
                }
            }
            ExportMsg::Failed { message } => {
                self.export_job = JobState::Failed { message };
            }
        }
    }

    // ----------------------------------------------------------- footer

    /// Left footer reading: what the current page holds.
    pub fn footer_left(&self) -> String {
        match self.page {
            Page::Media => {
                let song = if self.song_index.is_some() {
                    "song set"
                } else {
                    "no song"
                };
                format!("{} files in pool, {song}", self.pool.len())
            }
            Page::Cut => self.cut_status_line(),
            Page::Edit => match self.selected_item {
                Some(id) => format!("clip {id} selected"),
                None => "no clip selected".to_string(),
            },
            Page::Export => match &self.export_job {
                JobState::Done => "export finished".to_string(),
                JobState::Running { .. } => "export running".to_string(),
                _ => "export not started".to_string(),
            },
        }
    }

    /// Right footer reading: the busiest job right now.
    pub fn footer_right(&self) -> String {
        if self.export_job.is_running() {
            return self.export_job.summary();
        }
        if self.pipeline_job.is_running() {
            return self.pipeline_job.summary();
        }
        if self.probe_job.is_running() {
            return self.probe_job.summary();
        }
        match (&self.export_job, &self.pipeline_job) {
            (JobState::Done, _) => "export done".to_string(),
            (_, JobState::Done) => "analysis done".to_string(),
            (JobState::Failed { .. }, _) => self.export_job.summary(),
            (_, JobState::Failed { .. }) => self.pipeline_job.summary(),
            _ => "idle".to_string(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

/// One clip as the timeline strips draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedClipView {
    pub item_id: ItemId,
    pub asset_id: AssetId,
    pub start: Tick,
    pub duration: Tick,
    pub source_in: Tick,
    pub state: ClipState,
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use ricodex_audio::{Beat, SongAnalysis};
    use ricodex_engine::{BeatGrid, PlannedClip, Planner, SceneInfo, SourceInfo};
    use ricodex_media::{MediaBlackRange, MediaScene};

    fn fake_analysis(duration_s: f64) -> SourceAnalysis {
        SourceAnalysis {
            duration: Tick::from_seconds(duration_s),
            scenes: vec![MediaScene {
                start: Tick::ZERO,
                duration: Tick::from_seconds(duration_s),
                motion: Some(0.5),
                best_moment: false,
                coarse: true,
            }],
            black_ranges: Vec::new(),
            stage: 1,
        }
    }

    fn fake_song() -> SongAnalysis {
        SongAnalysis {
            duration: Tick::from_seconds(8.0),
            sample_rate: 44_100,
            bpm: 120.0,
            beats_per_bar: 4,
            onsets: Vec::new(),
            beats: (0..16)
                .map(|i| Beat {
                    time: Tick::from_seconds(i as f64 * 0.5),
                    strength: 0.8,
                })
                .collect(),
            bars: Vec::new(),
            phrases: Vec::new(),
            change_points: Vec::new(),
        }
    }

    fn state_with_media() -> AppState {
        let mut state = AppState::new();
        let song = state.import_path(PathBuf::from("/tmp/song.wav"));
        let video = state.import_path(PathBuf::from("/tmp/clip.mp4"));
        state.handle_probe_result(ProbeResult {
            index: song,
            fingerprint: None,
            duration: Some(Tick::from_seconds(8.0)),
            probe: None,
            error: None,
        });
        state.handle_probe_result(ProbeResult {
            index: video,
            fingerprint: None,
            duration: Some(Tick::from_seconds(20.0)),
            probe: None,
            error: None,
        });
        state
    }

    #[test]
    fn installed_plan_restores_intro_trim_to_absolute_source_coordinates() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let asset_id = state.pool[video_index].asset_id;
        state.set_trims_seconds(video_index, 2.0, 3.0);

        let source_in = Tick::from_seconds(1.0);
        let duration = Tick::from_seconds(4.0);
        let plan = Plan {
            track_id: VIDEO_TRACK_ID,
            clips: vec![PlannedClip {
                item_id: 50,
                asset_id,
                start: Tick::ZERO,
                duration,
                source_in,
                state: ClipState::Settled,
            }],
            commands: vec![Command::InsertItem {
                track_id: VIDEO_TRACK_ID,
                item: ricodex_core::Item::new(
                    50,
                    asset_id,
                    Tick::ZERO,
                    duration,
                    source_in,
                ),
            }],
        };

        state.install_plan(plan);

        assert_eq!(state.project.item(50).unwrap().source_in, Tick::from_seconds(3.0));
        assert_eq!(
            state.plan.as_ref().unwrap().clips[0].source_in,
            Tick::from_seconds(3.0)
        );
        assert!(!state.can_edit_source_trims());
    }

    #[test]
    fn source_trims_cannot_change_after_a_timeline_exists() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        state.set_trims_seconds(video_index, 2.0, 3.0);
        let asset_id = state.pool[video_index].asset_id;
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().push(ricodex_core::Item::new(
            51,
            asset_id,
            Tick::ZERO,
            Tick::from_seconds(4.0),
            Tick::from_seconds(2.0),
        ));
        state.history = History::new(state.project.clone());

        assert!(!state.can_edit_source_trims());
        state.set_trims_seconds(video_index, 5.0, 1.0);
        let asset = state.project.assets.iter().find(|asset| asset.id == asset_id).unwrap();
        assert_eq!(asset.intro_trim, Tick::from_seconds(2.0));
        assert_eq!(asset.outro_trim, Tick::from_seconds(3.0));
    }

    #[test]
    fn app_edits_use_core_history_and_refresh_timeline_views() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let asset_id = state.pool[video_index].asset_id;
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().push(ricodex_core::Item::new(
            10,
            asset_id,
            Tick::ZERO,
            Tick::from_seconds(8.0),
            Tick::ZERO,
        ));
        state.history = History::new(state.project.clone());
        state.selected_item = Some(10);
        let song_index = state
            .pool
            .iter()
            .position(|entry| entry.kind == MediaKind::Audio)
            .unwrap();
        state.set_song(song_index);
        assert!(state.can_begin_analysis());

        state
            .apply_edit(&Command::SplitItem {
                item_id: 10,
                at: Tick::from_seconds(4.0),
                new_item_id: 11,
            })
            .unwrap();

        assert_eq!(state.project.items(VIDEO_TRACK_ID).unwrap().len(), 2);
        assert_eq!(state.timeline_clips().len(), 2);
        assert_eq!(state.selected_item, Some(10));
        assert!(state.is_pinned(10));
        assert!(state.is_pinned(11));
        assert!(state.has_manual_edits);
        assert!(state.can_begin_analysis());

        assert!(state.undo_edit());
        assert_eq!(state.project.items(VIDEO_TRACK_ID).unwrap().len(), 1);
        assert_eq!(state.timeline_clips().len(), 1);
        assert_eq!(state.selected_item, Some(10));
        assert!(!state.has_manual_edits);
        assert!(state.can_begin_analysis());

        assert!(state.redo_edit());
        assert_eq!(state.project.items(VIDEO_TRACK_ID).unwrap().len(), 2);
        assert_eq!(state.timeline_clips().len(), 2);
        assert!(state.has_manual_edits);
        assert!(state.can_begin_analysis());
    }


    #[test]
    fn saved_timeline_can_be_refined_without_replacing_pinned_clips() {
        let mut state = state_with_media();
        let song_index = state.pool.iter().position(|entry| entry.kind == MediaKind::Audio).unwrap();
        assert!(state.set_song(song_index));
        let video_index = state.video_indices()[0];
        state.set_trims_seconds(video_index, 1.0, 1.0);
        let video_asset_id = state.pool[video_index].asset_id;

        let mut manual = ricodex_core::Item::new(
            10, video_asset_id, Tick::ZERO, Tick::from_seconds(4.0), Tick::from_seconds(1.0),
        );
        manual.provenance = ItemProvenance::Manual;
        manual.pinned = true;
        let generated = ricodex_core::Item::new(
            11, video_asset_id, Tick::from_seconds(4.0),
            Tick::from_seconds(4.0), Tick::from_seconds(5.0),
        );
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().extend([manual, generated]);
        state.history = History::new(state.project.clone());
        state.refresh_clip_flags();

        assert!(state.has_manual_edits);
        assert!(state.can_begin_analysis());

        // Importing more media remains possible after a manual edit.
        let prior_assets = state.project.assets.len();
        let new_index = state.import_path(PathBuf::from("/tmp/extra-footage.mp4"));
        assert_eq!(state.pool[new_index].kind, MediaKind::Video);
        assert_eq!(state.project.assets.len(), prior_assets + 1);
        // The pre-existing ready video still makes analysis available.
        let request = state.begin_analysis(PathBuf::from("/tmp/cache")).unwrap();
        let initial = request.initial_plan.expect("saved timeline seeds refinement");
        assert_eq!(initial.clips.len(), 2);
        assert_eq!(initial.clips[0].item_id, 10);
        assert_eq!(initial.clips[0].state, ClipState::Settled);
        assert_eq!(initial.clips[0].source_in, Tick::ZERO);
        assert_eq!(initial.clips[1].item_id, 11);
        assert_eq!(initial.clips[1].state, ClipState::Provisional);
        assert_eq!(initial.clips[1].source_in, Tick::from_seconds(4.0));
    }

    #[test]
    fn planner_update_retains_persisted_provenance_and_pin_state() {
        let mut state = state_with_media();
        let video_asset_id = state.pool[state.video_indices()[0]].asset_id;
        let mut manual = ricodex_core::Item::new(
            10, video_asset_id, Tick::ZERO, Tick::from_seconds(4.0), Tick::ZERO,
        );
        manual.provenance = ItemProvenance::Manual;
        manual.pinned = true;
        manual.enabled = false;
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().push(manual);
        state.history = History::new(state.project.clone());

        let plan = Plan {
            track_id: VIDEO_TRACK_ID,
            clips: vec![PlannedClip {
                item_id: 10,
                asset_id: video_asset_id,
                start: Tick::ZERO,
                duration: Tick::from_seconds(4.0),
                source_in: Tick::ZERO,
                state: ClipState::Settled,
            }],
            commands: vec![Command::InsertItem {
                track_id: VIDEO_TRACK_ID,
                item: ricodex_core::Item::new(
                    10, video_asset_id, Tick::ZERO, Tick::from_seconds(4.0), Tick::ZERO,
                ),
            }],
        };
        state.install_plan(plan);
        let installed = state.project.item(10).unwrap();
        assert_eq!(installed.provenance, ItemProvenance::Manual);
        assert!(installed.pinned);
        assert!(!installed.enabled);
        assert!(state.is_pinned(10));
        assert!(state.has_manual_edits);
    }

    #[test]
    fn app_rejects_edits_while_analysis_is_running() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let asset_id = state.pool[video_index].asset_id;
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().push(ricodex_core::Item::new(
            10,
            asset_id,
            Tick::ZERO,
            Tick::from_seconds(4.0),
            Tick::ZERO,
        ));
        state.history = History::new(state.project.clone());
        state.pipeline_job = JobState::Running {
            label: "analysing".to_string(),
            fraction: 0.5,
        };

        let before = state.project.clone();
        let result = state.apply_edit(&Command::TrimItem {
            item_id: 10,
            new_start: None,
            new_source_in: Some(Tick::from_seconds(1.0)),
            new_duration: None,
        });
        assert!(result.is_err());
        assert_eq!(state.project, before);
    }

    #[test]
    fn save_restore_preserves_song_timeline_and_source_trims() {
        let mut state = state_with_media();
        let audio_index = state
            .pool
            .iter()
            .position(|entry| entry.kind == MediaKind::Audio)
            .unwrap();
        assert!(state.set_song(audio_index));

        let video_index = state.video_indices()[0];
        let video_asset_id = state.pool[video_index].asset_id;
        state.set_trims_seconds(video_index, 1.25, 2.0);
        let mut saved_clip = ricodex_core::Item::new(
            42,
            video_asset_id,
            Tick::ZERO,
            Tick::from_seconds(5.0),
            Tick::from_seconds(1.5),
        );
        saved_clip.provenance = ItemProvenance::Manual;
        saved_clip.pinned = true;
        state.project.items_mut(VIDEO_TRACK_ID).unwrap().push(saved_clip);

        let path = std::env::temp_dir().join(format!(
            "ricodex-app-project-{}.rdx",
            std::process::id()
        ));
        state.save_project_to(path.clone()).unwrap();
        assert!(!state.is_dirty);

        let loaded = Project::load(&path).unwrap();
        assert_eq!(loaded.song_asset_id, Some(state.pool[audio_index].asset_id));
        assert_eq!(loaded.items(VIDEO_TRACK_ID).unwrap().len(), 1);
        assert_eq!(loaded.assets.iter().find(|asset| asset.id == video_asset_id).unwrap().intro_trim,
                   Tick::from_seconds(1.25));
        assert_eq!(loaded.assets.iter().find(|asset| asset.id == video_asset_id).unwrap().outro_trim,
                   Tick::from_seconds(2.0));

        let mut restored = AppState::new();
        let probes = restored.restore_project(loaded, path.clone());
        assert!(probes.is_empty(), "the test fixtures deliberately reference missing media paths");
        assert_eq!(restored.song_index, Some(audio_index));
        assert_eq!(restored.timeline_clips().len(), 1);
        assert!(restored.has_manual_edits);
        assert!(restored.is_pinned(42));
        assert!(restored.pool.iter().all(|entry| entry.probe_error.is_some()));
        assert!(!restored.can_begin_analysis());
        assert_eq!(restored.project_path, Some(path.clone()));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn relinking_requires_the_saved_content_fingerprint() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let asset_id = state.pool[video_index].asset_id;
        let expected = Fingerprint::new(100, 1, 2, 3);
        state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id == asset_id)
            .unwrap()
            .fingerprint = expected;
        state.history = History::new(state.project.clone());

        let original_path = state.pool[video_index].path.clone();
        assert_eq!(state.begin_relink_probe(video_index), Some(MediaKind::Video));
        state.handle_relink_result(RelinkResult {
            index: video_index,
            path: PathBuf::from("/tmp/unrelated.mp4"),
            fingerprint: Some(Fingerprint::new(200, 4, 5, 6)),
            duration: Some(Tick::from_seconds(20.0)),
            probe: None,
            error: None,
        });
        assert_eq!(state.pool[video_index].path, original_path);
        assert!(state.pool[video_index]
            .probe_error
            .as_deref()
            .unwrap()
            .contains("does not match"));

        assert_eq!(state.begin_relink_probe(video_index), Some(MediaKind::Video));
        state.handle_relink_result(RelinkResult {
            index: video_index,
            path: PathBuf::from("/tmp/matching-copy.mp4"),
            fingerprint: Some(expected),
            duration: Some(Tick::from_seconds(20.0)),
            probe: None,
            error: None,
        });
        assert_eq!(
            state.pool[video_index].path,
            PathBuf::from("/tmp/matching-copy.mp4")
        );
        assert_eq!(
            state.project.assets.iter().find(|asset| asset.id == asset_id).unwrap().path_hint,
            "/tmp/matching-copy.mp4"
        );
        assert!(state.is_dirty);
    }

    #[test]
    fn relinking_refuses_assets_without_a_saved_fingerprint() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let original_path = state.pool[video_index].path.clone();
        assert!(!state.can_relink_asset(video_index));
        assert_eq!(state.begin_relink_probe(video_index), Some(MediaKind::Video));
        state.handle_relink_result(RelinkResult {
            index: video_index,
            path: PathBuf::from("/tmp/unverifiable-replacement.mp4"),
            fingerprint: Some(Fingerprint::new(500, 1, 2, 3)),
            duration: Some(Tick::from_seconds(20.0)),
            probe: None,
            error: None,
        });
        assert_eq!(state.pool[video_index].path, original_path);
        assert!(state.pool[video_index]
            .probe_error
            .as_deref()
            .unwrap()
            .contains("no saved media fingerprint"));
        assert!(!state.can_relink_asset(video_index));
    }

    #[test]
    fn original_path_can_establish_a_missing_fingerprint() {
        let mut state = state_with_media();
        let video_index = state.video_indices()[0];
        let asset_id = state.pool[video_index].asset_id;
        let original_path = state.pool[video_index].path.clone();
        let fingerprint = Fingerprint::new(700, 7, 8, 9);
        assert!(!state.can_relink_asset(video_index));
        assert_eq!(state.begin_relink_probe(video_index), Some(MediaKind::Video));
        state.handle_relink_result(RelinkResult {
            index: video_index,
            path: original_path.clone(),
            fingerprint: Some(fingerprint),
            duration: Some(Tick::from_seconds(20.0)),
            probe: None,
            error: None,
        });
        assert_eq!(state.pool[video_index].path, original_path);
        assert!(state.can_relink_asset(video_index));
        assert_eq!(
            state.project.assets.iter().find(|asset| asset.id == asset_id).unwrap().fingerprint,
            fingerprint
        );
    }

    #[test]
    fn import_registers_assets_and_dedupes() {
        let mut state = AppState::new();
        let a = state.import_path(PathBuf::from("/tmp/a.mp4"));
        let b = state.import_path(PathBuf::from("/tmp/a.mp4"));
        assert_eq!(a, b);
        assert_eq!(state.pool.len(), 1);
        assert_eq!(state.project.assets.len(), 1);
        assert!(state.pool[a].probing);
    }

    #[test]
    fn song_must_be_audio() {
        let mut state = state_with_media();
        let video = state.video_indices()[0];
        assert!(!state.set_song(video));
        assert!(state.song_index.is_none());
        let audio = (0..state.pool.len())
            .find(|i| state.pool[*i].kind == MediaKind::Audio)
            .unwrap();
        assert!(state.set_song(audio));
        assert_eq!(state.song_index, Some(audio));
    }

    #[test]
    fn begin_analysis_requires_song_and_video() {
        let mut state = AppState::new();
        let video = state.import_path(PathBuf::from("/tmp/clip.mp4"));
        let _ = video;
        assert!(!state.can_begin_analysis());
        assert!(state.begin_analysis(PathBuf::from("/tmp/cache")).is_none());

        let mut state = state_with_media();
        let audio = (0..state.pool.len())
            .find(|i| state.pool[*i].kind == MediaKind::Audio)
            .unwrap();
        assert!(state.set_song(audio));
        assert!(state.can_begin_analysis());
        let request = state.begin_analysis(PathBuf::from("/tmp/cache")).unwrap();
        assert_eq!(request.videos.len(), 1);
        assert_eq!(state.page, Page::Cut);
        assert_eq!(state.cut_status_line(), "analysing song");
    }

    #[test]
    fn pipeline_messages_drive_the_status_line() {
        let mut state = state_with_media();
        let audio = (0..state.pool.len())
            .find(|i| state.pool[*i].kind == MediaKind::Audio)
            .unwrap();
        state.set_song(audio);
        let video = state.video_indices()[0];
        state.begin_analysis(PathBuf::from("/tmp/cache")).unwrap();

        state.handle_pipeline_msg(PipelineMsg::SongFinished {
            result: Ok(fake_song()),
        });
        assert_eq!(state.cut_status_line(), "quick pass 0 of 1");

        state.handle_pipeline_msg(PipelineMsg::Stage1Finished {
            index: video,
            result: Ok(fake_analysis(20.0)),
        });

        // Build the rough cut the way the worker does and feed it in.
        let grid = BeatGrid::from_analysis(&state.song_analysis.clone().unwrap());
        let sources = vec![trimmed_source(
            state.pool[video].asset_id,
            state.pool[video].stage1.as_ref().unwrap(),
            Tick::ZERO,
            Tick::ZERO,
        )];
        let plan = Planner::new(7).plan(
            &grid,
            &sources,
            &PlannerSettings {
                generation: state.project.generation_settings.clone(),
                ..PlannerSettings::default()
            },
            VIDEO_TRACK_ID,
            1,
        );
        assert!(!plan.clips.is_empty(), "a rough cut should exist");
        state.handle_pipeline_msg(PipelineMsg::PlanUpdated { plan });
        assert_eq!(
            state.cut_status_line(),
            "rough cut ready, refining 0 of 1 videos"
        );
        assert_eq!(state.project.items(VIDEO_TRACK_ID).unwrap().len(),
                   state.plan.as_ref().unwrap().clips.len());

        state.handle_pipeline_msg(PipelineMsg::Stage2Progress {
            index: video,
            fraction: 0.5,
        });
        assert!(state.pipeline_fraction() > 0.4 && state.pipeline_fraction() < 1.0);

        state.handle_pipeline_msg(PipelineMsg::Stage2Finished {
            index: video,
            result: Ok(fake_analysis(20.0)),
        });
        state.handle_pipeline_msg(PipelineMsg::Finished);
        assert_eq!(state.cut_status_line(), "analysis complete");
        assert_eq!(state.pipeline_job, JobState::Done);
    }

    #[test]
    fn failure_and_cancel_reach_the_status_line() {
        let mut state = state_with_media();
        let audio = (0..state.pool.len())
            .find(|i| state.pool[*i].kind == MediaKind::Audio)
            .unwrap();
        state.set_song(audio);
        state.begin_analysis(PathBuf::from("/tmp/cache")).unwrap();
        state.handle_pipeline_msg(PipelineMsg::SongFinished {
            result: Err("no beats found".to_string()),
        });
        assert_eq!(state.cut_status_line(), "analysis failed: no beats found");

        let mut state = state_with_media();
        let audio = (0..state.pool.len())
            .find(|i| state.pool[*i].kind == MediaKind::Audio)
            .unwrap();
        state.set_song(audio);
        state.begin_analysis(PathBuf::from("/tmp/cache")).unwrap();
        state.handle_pipeline_msg(PipelineMsg::Cancelled);
        assert_eq!(state.cut_status_line(), "analysis cancelled");
    }

    #[test]
    fn pins_toggle() {
        let mut state = AppState::new();
        assert!(!state.is_pinned(5));
        state.toggle_pin(5);
        assert!(state.is_pinned(5));
        state.toggle_pin(5);
        assert!(!state.is_pinned(5));
    }

    #[test]
    fn trims_are_clamped_to_the_file() {
        let mut state = state_with_media();
        let video = state.video_indices()[0];
        state.set_trims_seconds(video, 15.0, 15.0);
        let entry = &state.pool[video];
        assert!(entry.intro_trim + entry.outro_trim <= entry.duration.unwrap());
        assert!(entry.intro_trim >= Tick::ZERO && entry.outro_trim >= Tick::ZERO);
    }

    #[test]
    fn trimmed_source_shifts_and_clips_scenes() {
        let mut analysis = fake_analysis(20.0);
        analysis.scenes = vec![
            MediaScene {
                start: Tick::ZERO,
                duration: Tick::from_seconds(5.0),
                motion: Some(0.2),
                best_moment: false,
                coarse: true,
            },
            MediaScene {
                start: Tick::from_seconds(5.0),
                duration: Tick::from_seconds(10.0),
                motion: Some(0.6),
                best_moment: true,
                coarse: true,
            },
            MediaScene {
                start: Tick::from_seconds(15.0),
                duration: Tick::from_seconds(5.0),
                motion: Some(0.4),
                best_moment: false,
                coarse: true,
            },
        ];
        analysis.black_ranges = vec![MediaBlackRange {
            start: Tick::from_seconds(18.0),
            duration: Tick::from_seconds(2.0),
        }];
        let source = trimmed_source(
            9,
            &analysis,
            Tick::from_seconds(2.0),
            Tick::from_seconds(2.0),
        );
        assert_eq!(source.duration, Tick::from_seconds(16.0));
        // Scene 1 clips to 2..5 of the window, scene 2 shifts whole,
        // scene 3 clips to 15..18.
        assert_eq!(source.scenes.len(), 3);
        assert_eq!(source.scenes[0].start, Tick::ZERO);
        assert_eq!(source.scenes[0].duration, Tick::from_seconds(3.0));
        assert_eq!(source.scenes[1].start, Tick::from_seconds(3.0));
        assert_eq!(source.scenes[2].duration, Tick::from_seconds(3.0));
        // The black range at 18..20 falls outside the 2..18 window end.
        assert!(source.black_ranges.is_empty());
        let _: &SceneInfo = &source.scenes[0];
        let _: SourceInfo = source;
    }

    #[test]
    fn eta_needs_a_meaningful_fraction() {
        assert_eq!(eta_seconds(0.0, 10.0), None);
        assert_eq!(eta_seconds(1.0, 10.0), None);
        assert_eq!(eta_seconds(0.5, 10.0), Some(10.0));
        assert_eq!(eta_seconds(0.25, 8.0), Some(24.0));
    }

    #[test]
    fn export_messages_track_encoder_and_cancel() {
        let mut state = state_with_media();
        state.begin_export(PathBuf::from("/tmp/out.mp4"));
        state.handle_export_msg(ExportMsg::Progress { fraction: 0.25 });
        assert!(state.export_job.is_running());
        state.handle_export_msg(ExportMsg::Finished {
            encoder_name: "libx264".to_string(),
            frames_written: 90,
            output_path: PathBuf::from("/tmp/out.mp4"),
        });
        assert_eq!(state.export_job, JobState::Done);
        let (encoder, frames, _) = state.export_result.clone().unwrap();
        assert_eq!(encoder, "libx264");
        assert_eq!(frames, 90);

        // A cancel requested mid export turns the finish into a
        // cancellation, and the result is not kept.
        let mut state = state_with_media();
        state.begin_export(PathBuf::from("/tmp/out.mp4"));
        state
            .export_cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        state.handle_export_msg(ExportMsg::Finished {
            encoder_name: "libx264".to_string(),
            frames_written: 90,
            output_path: PathBuf::from("/tmp/out.mp4"),
        });
        assert_eq!(state.export_job, JobState::Cancelled);
        assert!(state.export_result.is_none());
    }
}
