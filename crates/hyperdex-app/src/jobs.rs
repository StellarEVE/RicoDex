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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use hyperdex_core::{AssetKind, ItemId, Project};
use hyperdex_engine::{BeatGrid, Planner, SourceInfo};
use hyperdex_media::AnalysisCache;

use crate::state::{
    trimmed_source, ExportMsg, MediaKind, PipelineMsg, PipelineRequest,
    ProbeResult, RelinkResult, VIDEO_TRACK_ID,
};

/// Stage 0 for one imported file: the content fingerprint, plus the
/// container probe for video, or a duration from decoding for audio
/// (hyperdex-audio has no header only probe, so the duration of a song is
/// read by decoding it once here; the song analysis decodes it again
/// later, which is accepted for this slice).
pub fn run_probe(index: usize, path: PathBuf, kind: MediaKind, tx: Sender<ProbeResult>) {
    let fingerprint = hyperdex_media::compute_fingerprint(&path).ok();
    let (duration, probe, error) = match kind {
        MediaKind::Video => match hyperdex_media::probe(&path) {
            Ok(probe) => (Some(probe.duration), Some(probe), None),
            Err(e) => (None, None, Some(format!("probe failed: {e}"))),
        },
        MediaKind::Audio => match hyperdex_audio::decode_file(&path) {
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
    let fingerprint = hyperdex_media::compute_fingerprint(&path).ok();
    let (duration, probe, error) = match kind {
        MediaKind::Video => match hyperdex_media::probe(&path) {
            Ok(probe) => (Some(probe.duration), Some(probe), None),
            Err(e) => (None, None, Some(format!("probe failed: {e}"))),
        },
        MediaKind::Audio => match hyperdex_audio::decode_file(&path) {
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
    let song = match hyperdex_audio::analyse_file(&request.song_path, &hyperdex_audio::AnalysisSettings::default())
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
    let pins: HashSet<ItemId> = pinned
        .lock()
        .map(|set| set.iter().copied().collect())
        .unwrap_or_default();
    let mut plan = match request.initial_plan.as_ref() {
        Some(existing) => planner.refine(
            existing,
            &grid,
            &source_list,
            &request.planner_settings,
            &pins,
        ),
        None => planner.plan(
            &grid,
            &source_list,
            &request.planner_settings,
            VIDEO_TRACK_ID,
            1,
        ),
    };
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

/// Export every visible video track in project stacking order through
/// hyperdex-render, reporting progress per frame and the resolved encoder
/// name at the end. A cancel requested mid render cannot stop the encoder,
/// so it is honoured by the state when the finish message lands: the state
/// marks the job Cancelled and keeps no result.
pub fn run_export(project: Project, output_path: PathBuf, tx: Sender<ExportMsg>) {
    let mut progress = |fraction: f32| {
        let _ = tx.send(ExportMsg::Progress { fraction });
    };

    if let Err(error) = project.validate() {
        let _ = tx.send(ExportMsg::Failed {
            message: format!("project is invalid: {error}"),
        });
        return;
    }

    // Decode each audio asset used by the selected song or audible Audio
    // track exactly once. Keep decoding at the App boundary and pass only
    // plain stereo PCM into Render.
    let assets_by_id: HashMap<_, _> = project.assets.iter().map(|asset| (asset.id, asset)).collect();
    let mut asset_ids = BTreeSet::new();
    let mut has_audio_track_items = false;
    if let Some(song_id) = project.song_asset_id {
        asset_ids.insert(song_id);
    }
    for track in project.tracks.iter().filter(|track| {
        track.kind == hyperdex_core::TrackKind::Audio && !track.hidden && !track.muted
    }) {
        if let Some(items) = project.items(track.id) {
            for item in items.iter().filter(|item| item.enabled) {
                has_audio_track_items = true;
                asset_ids.insert(item.asset_id);
            }
        }
    }

    let mut decoded_assets = Vec::with_capacity(asset_ids.len());
    for asset_id in asset_ids {
        let Some(asset) = assets_by_id.get(&asset_id).copied() else {
            let message = if project.song_asset_id == Some(asset_id) {
                format!("selected song asset {asset_id} is missing from the project")
            } else {
                format!("audio item references missing asset {asset_id}")
            };
            let _ = tx.send(ExportMsg::Failed { message });
            return;
        };
        if asset.kind != AssetKind::Audio {
            let message = if project.song_asset_id == Some(asset_id) {
                "selected song asset is not audio".to_string()
            } else {
                format!("audio track asset {asset_id} is not audio")
            };
            let _ = tx.send(ExportMsg::Failed { message });
            return;
        }
        match hyperdex_audio::decode_stereo_file(&PathBuf::from(&asset.path_hint)) {
            Ok(decoded) => decoded_assets.push((asset_id, decoded)),
            Err(error) => {
                let label = if project.song_asset_id == Some(asset_id) {
                    "selected song"
                } else {
                    "audio track asset"
                };
                let _ = tx.send(ExportMsg::Failed {
                    message: format!("{label} {asset_id} could not be decoded: {error}"),
                });
                return;
            }
        }
    }

    let audio_track_inputs: Vec<_> = decoded_assets
        .iter()
        .map(|(asset_id, decoded)| hyperdex_render::StereoAudioAssetInput {
            asset_id: *asset_id,
            interleaved_samples: &decoded.samples,
            sample_rate: decoded.sample_rate,
        })
        .collect();
    let song_input = project.song_asset_id.and_then(|song_id| {
        decoded_assets
            .iter()
            .find(|(asset_id, _)| *asset_id == song_id)
            .map(|(_, decoded)| hyperdex_render::StereoAudioInput {
                interleaved_samples: &decoded.samples,
                sample_rate: decoded.sample_rate,
            })
    });
    let result = if song_input.is_some() || has_audio_track_items {
        hyperdex_render::render_timeline_with_audio_items(
            &project,
            &output_path,
            song_input,
            &audio_track_inputs,
            &mut progress,
        )
    } else {
        hyperdex_render::render_timeline(&project, &output_path, &mut progress)
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

#[cfg(test)]
mod tests {
    use super::*;
    use hyperdex_core::{Asset, Fingerprint, Item, Tick, Track, TrackKind};

    fn run_ffmpeg(args: &[&str]) {
        let executable = std::env::var_os("HYPERDEX_TEST_FFMPEG")
            .unwrap_or_else(|| "ffmpeg".into());
        let status = std::process::Command::new(executable)
            .args(args)
            .status()
            .expect("run ffmpeg fixture generator");
        assert!(status.success(), "ffmpeg fixture generation failed: {args:?}");
    }

    #[test]
    fn export_worker_mixes_audio_track_items_into_the_mp4() {
        let dir = std::env::temp_dir().join(format!(
            "hyperdex-app-export-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video_path = dir.join("video.mp4");
        let audio_path = dir.join("audio.wav");
        let output_path = dir.join("export.mp4");
        run_ffmpeg(&[
            "-y", "-v", "error", "-f", "lavfi", "-i",
            "color=c=blue:s=64x64:r=10:d=1", "-c:v", "libx264", "-pix_fmt",
            "yuv420p", "-an", video_path.to_str().unwrap(),
        ]);
        run_ffmpeg(&[
            "-y", "-v", "error", "-f", "lavfi", "-i",
            "sine=frequency=880:sample_rate=44100:duration=1", "-ac", "2",
            "-c:a", "pcm_s16le", audio_path.to_str().unwrap(),
        ]);

        let mut project = Project::new();
        project.export_settings.width = 64;
        project.export_settings.height = 64;
        project.export_settings.frame_rate_num = 10;
        project.assets.extend([
            Asset {
                id: 1,
                fingerprint: Fingerprint::new(1, 1, 1, 1),
                path_hint: video_path.to_string_lossy().into_owned(),
                kind: AssetKind::Video,
                duration: Tick::from_seconds(1.0),
                intro_trim: Tick::ZERO,
                outro_trim: Tick::ZERO,
            },
            Asset {
                id: 2,
                fingerprint: Fingerprint::new(2, 2, 2, 2),
                path_hint: audio_path.to_string_lossy().into_owned(),
                kind: AssetKind::Audio,
                duration: Tick::from_seconds(1.0),
                intro_trim: Tick::ZERO,
                outro_trim: Tick::ZERO,
            },
        ]);
        project.add_track(Track::new(1, TrackKind::Video, "V1"));
        project.add_track(Track::new(2, TrackKind::Audio, "A1"));
        project.items_mut(1).unwrap().push(Item::new(
            1,
            1,
            Tick::ZERO,
            Tick::from_seconds(1.0),
            Tick::ZERO,
        ));
        project.items_mut(2).unwrap().push(Item::new(
            2,
            2,
            Tick::from_seconds(0.25),
            Tick::from_seconds(0.5),
            Tick::ZERO,
        ));

        let (tx, rx) = std::sync::mpsc::channel();
        run_export(project, output_path.clone(), tx);
        loop {
            match rx.recv().unwrap() {
                ExportMsg::Progress { .. } => {}
                ExportMsg::Finished { .. } => break,
                ExportMsg::Failed { message } => panic!("app export failed: {message}"),
            }
        }

        let exported = hyperdex_audio::decode_stereo_file(&output_path)
            .expect("app export should contain decodable AAC audio");
        assert_eq!(exported.sample_rate, 48_000);
        let rms = |start: usize, end: usize| {
            let samples = &exported.samples[start * 2..end * 2];
            (samples.iter().map(|sample| sample * sample).sum::<f32>()
                / samples.len() as f32)
                .sqrt()
        };
        let before_item = rms(0, 4_800);
        let during_item = rms(14_400, 19_200);
        assert!(before_item < 0.01, "unexpected audio before item: {before_item}");
        assert!(during_item > 0.05, "audio track item was missing: {during_item}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
