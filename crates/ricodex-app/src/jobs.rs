//! Background job plumbing: the worker threads behind the app.
//!
//! Every heavy operation runs on its own thread and reports back
//! over std channels; the UI thread only ever drains channels and
//! updates state, so it never blocks on a decode or an analysis.
//! Cancellation is a shared atomic flag the workers check at stage
//! boundaries (between files, between stages): a single Stage 2
//! pass or a single render cannot be interrupted mid flight, and
//! when a cancel lands during one, the workers stop at the next
//! boundary and report Cancelled instead of the stage result.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use ricodex_core::{AssetKind, ItemId, Project};
use ricodex_engine::{BeatGrid, Planner, SourceInfo};
use ricodex_media::AnalysisCache;

use crate::state::{
    trimmed_source, ExportMsg, MediaKind, PipelineMsg, PipelineRequest,
    ProbeResult, RelinkResult, VIDEO_TRACK_ID,
};

/// Stage 0 for one imported file: the content fingerprint, plus the
/// container probe for video, or a duration from decoding for audio
/// (ricodex-audio has no header only probe, so the duration of a song is
/// read by decoding it once here; the song analysis decodes it again
/// later, which is accepted for this slice).
pub fn run_probe(index: usize, path: PathBuf, kind: MediaKind, tx: Sender<ProbeResult>) {
    let fingerprint = ricodex_media::compute_fingerprint(&path).ok();
    let (duration, probe, error) = match kind {
        MediaKind::Video => match ricodex_media::probe(&path) {
            Ok(probe) => (Some(probe.duration), Some(probe), None),
            Err(e) => (None, None, Some(format!("probe failed: {e}"))),
        },
        MediaKind::Audio => match ricodex_audio::decode_file(&path) {
            Ok(decoded) => (Some(decoded.duration()), None, None),
            Err(e) => (None, None, Some(format!("audio read failed: {e}"))),
        },
    };
    let _ = tx.send(ProbeResult {
        index,
        fingerprint,
        duration,
        probe,
        error,
    });
}

/// Verify a media path restored from a project or selected by the user.
/// Content identity is checked by AppState before the path is accepted.
pub fn run_relink_probe(
    index: usize,
    path: PathBuf,
    kind: MediaKind,
    tx: Sender<RelinkResult>,
) {
    let fingerprint = ricodex_media::compute_fingerprint(&path).ok();
    let (duration, probe, error) = match kind {
        MediaKind::Video => match ricodex_media::probe(&path) {
            Ok(probe) => (Some(probe.duration), Some(probe), None),
            Err(e) => (None, None, Some(format!("probe failed: {e}"))),
        },
        MediaKind::Audio => match ricodex_audio::decode_file(&path) {
            Ok(decoded) => (Some(decoded.duration()), None, None),
            Err(e) => (None, None, Some(format!("audio read failed: {e}"))),
        },
    };
    let _ = tx.send(RelinkResult {
        index,
        path,
        fingerprint,
        duration,
        probe,
        error,
    });
}

/// The staged pipeline, end to end, on one worker thread:
///
/// 1. Song analysis (fast, CPU).
/// 2. Stage 1 for every video, breadth before depth, through the
///    fingerprint keyed analysis cache.
/// 3. The rough cut, planned from Stage 1 data and sent to the UI.
/// 4. Stage 2 per video in pool order; after each one, the plan is
///    refined against the updated sources and sent again. Pinned
///    clips and settled clips are carried by the planner's refine
///    untouched; the pin set is snapshotted from the shared set at
///    each refine, so pins toggled mid run take effect at the next
///    refined video.
pub fn run_pipeline(
    request: PipelineRequest,
    pinned: Arc<Mutex<HashSet<ItemId>>>,
    cancel: Arc<AtomicBool>,
    tx: Sender<PipelineMsg>,
) {
    let cancelled = |tx: &Sender<PipelineMsg>| {
        let _ = tx.send(PipelineMsg::Cancelled);
    };

    // 1. Song.
    let song = match ricodex_audio::analyse_file(&request.song_path, &ricodex_audio::AnalysisSettings::default())
    {
        Ok(analysis) => analysis,
        Err(e) => {
            // The state turns a failed song into the failed job.
            let _ = tx.send(PipelineMsg::SongFinished {
                result: Err(format!("song analysis failed: {e}")),
            });
            return;
        }
    };
    let _ = tx.send(PipelineMsg::SongFinished {
        result: Ok(song.clone()),
    });
    if cancel.load(Ordering::SeqCst) {
        cancelled(&tx);
        return;
    }
    let grid = BeatGrid::from_analysis(&song);
    let cache = AnalysisCache::new(request.cache_home.clone());
    let planner = Planner::new(0xA9_2026);

    // 2. Stage 1, breadth before depth.
    let mut sources: Vec<(usize, SourceInfo)> = Vec::new();
    for video in &request.videos {
        if cancel.load(Ordering::SeqCst) {
            cancelled(&tx);
            return;
        }
        let _ = tx.send(PipelineMsg::Stage1Started { index: video.index });
        match cache.stage1(&video.path) {
            Ok(cached) => {
                sources.push((
                    video.index,
                    trimmed_source(
                        video.asset_id,
                        &cached.analysis,
                        video.intro_trim,
                        video.outro_trim,
                    ),
                ));
                let _ = tx.send(PipelineMsg::Stage1Finished {
                    index: video.index,
                    result: Ok(cached.analysis),
                });
            }
            Err(e) => {
                let _ = tx.send(PipelineMsg::Stage1Finished {
                    index: video.index,
                    result: Err(e.to_string()),
                });
            }
        }
    }
    if sources.is_empty() {
        let _ = tx.send(PipelineMsg::Failed {
            message: "no video could be analysed".to_string(),
        });
        return;
    }

    // 3. The rough cut.
    let source_list: Vec<SourceInfo> = sources.iter().map(|(_, s)| s.clone()).collect();
    let mut plan = planner.plan(
        &grid,
        &source_list,
        &request.planner_settings,
        VIDEO_TRACK_ID,
        1,
    );
    let _ = tx.send(PipelineMsg::PlanUpdated { plan: plan.clone() });

    // 4. Stage 2 per video, refining after each.
    for video in &request.videos {
        if cancel.load(Ordering::SeqCst) {
            cancelled(&tx);
            return;
        }
        if !sources.iter().any(|(index, _)| *index == video.index) {
            continue; // Stage 1 failed for this one; nothing to refine.
        }
        let _ = tx.send(PipelineMsg::Stage2Started { index: video.index });
        let tx_progress = tx.clone();
        let index = video.index;
        let mut progress = move |fraction: f32| {
            let _ = tx_progress.send(PipelineMsg::Stage2Progress {
                index,
                fraction,
            });
        };
        match cache.stage2(&video.path, &mut progress) {
            Ok(analysis) => {
                if let Some(slot) = sources.iter_mut().find(|(i, _)| *i == video.index) {
                    slot.1 = trimmed_source(
                        video.asset_id,
                        &analysis,
                        video.intro_trim,
                        video.outro_trim,
                    );
                }
                let _ = tx.send(PipelineMsg::Stage2Finished {
                    index: video.index,
                    result: Ok(analysis),
                });
                let pins: HashSet<ItemId> = pinned
                    .lock()
                    .map(|set| set.iter().copied().collect())
                    .unwrap_or_default();
                let updated: Vec<SourceInfo> =
                    sources.iter().map(|(_, s)| s.clone()).collect();
                plan = planner.refine(&plan, &grid, &updated, &request.planner_settings, &pins);
                let _ = tx.send(PipelineMsg::PlanUpdated { plan: plan.clone() });
            }
            Err(e) => {
                let _ = tx.send(PipelineMsg::Stage2Finished {
                    index: video.index,
                    result: Err(e.to_string()),
                });
            }
        }
        if cancel.load(Ordering::SeqCst) {
            cancelled(&tx);
            return;
        }
    }

    let _ = tx.send(PipelineMsg::Finished);
}

/// One export job: the sequential hard cut render of the current
/// timeline through ricodex-render, reporting progress per frame and
/// the resolved encoder name at the end. A cancel requested mid
/// render cannot stop the encoder, so it is honoured by the state
/// when the finish message lands: the state marks the job
/// Cancelled and keeps no result.
pub fn run_export(project: Project, output_path: PathBuf, tx: Sender<ExportMsg>) {
    let mut progress = |fraction: f32| {
        let _ = tx.send(ExportMsg::Progress { fraction });
    };

    // The selected song is the soundtrack for this export slice. Decode
    // it here, at the App orchestration boundary, then pass plain stereo
    // PCM to Render. Projects without a selected song remain video-only.
    let result = match project.song_asset_id {
        Some(song_id) => match project.assets.iter().find(|asset| asset.id == song_id) {
            Some(asset) if asset.kind == AssetKind::Audio => {
                let path = PathBuf::from(&asset.path_hint);
                match ricodex_audio::decode_stereo_file(&path) {
                    Ok(decoded) => ricodex_render::render_sequential_with_audio(
                        &project,
                        VIDEO_TRACK_ID,
                        &output_path,
                        Some(ricodex_render::StereoAudioInput {
                            interleaved_samples: &decoded.samples,
                            sample_rate: decoded.sample_rate,
                        }),
                        &mut progress,
                    ),
                    Err(error) => {
                        let _ = tx.send(ExportMsg::Failed {
                            message: format!("selected song could not be decoded: {error}"),
                        });
                        return;
                    }
                }
            }
            Some(_) => {
                let _ = tx.send(ExportMsg::Failed {
                    message: "selected song asset is not audio".to_string(),
                });
                return;
            }
            None => {
                let _ = tx.send(ExportMsg::Failed {
                    message: format!("selected song asset {song_id} is missing from the project"),
                });
                return;
            }
        },
        None => ricodex_render::render_sequential(
            &project,
            VIDEO_TRACK_ID,
            &output_path,
            &mut progress,
        ),
    };

    match result {
        Ok(outcome) => {
            let _ = tx.send(ExportMsg::Finished {
                encoder_name: outcome.encoder_name,
                frames_written: outcome.frames_written,
                output_path: outcome.output_path,
            });
        }
        Err(e) => {
            let _ = tx.send(ExportMsg::Failed {
                message: e.to_string(),
            });
        }
    }
}
