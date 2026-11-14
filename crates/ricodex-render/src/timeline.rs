//! Shared rational frame scheduling and timeline sampling primitives.
//!
//! Preview and export should ask this module which timeline instant a frame
//! represents. It deliberately contains no decoder or encoder dependencies.

use ricodex_core::{Project, Tick, TrackKind, TICKS_PER_SECOND};

/// One output frame's exact timeline timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineFrame {
    pub index: u64,
    pub time: Tick,
}

/// A media item contributing at a timeline instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineLayer {
    pub track_id: u64,
    pub track_kind: TrackKind,
    pub item_id: u64,
    pub asset_id: u64,
    pub source_time: Tick,
}

/// A deterministic frame schedule shared by preview and export.
///
/// Frame timestamps are calculated from the integer frame index and rational
/// rate, never by repeatedly adding a rounded frame duration.
#[derive(Clone, Debug)]
pub struct TimelineFrameSchedule {
    frame_rate_num: u32,
    frame_rate_den: u32,
    start_time: Tick,
    frame_count: u64,
}

impl TimelineFrameSchedule {
    pub fn new(project: &Project, frame_rate_num: u32, frame_rate_den: u32) -> Option<Self> {
        if frame_rate_num == 0 || frame_rate_den == 0 {
            return None;
        }
        let end = project.track_items.iter()
            .flat_map(|group| group.items.iter())
            .map(|item| item.end())
            .max()
            .unwrap_or(Tick::ZERO);
        Self::for_range(Tick::ZERO, end, frame_rate_num, frame_rate_den)
    }

    /// Build a schedule for a bounded timeline range. Frame timestamps
    /// remain absolute project times, while the frame index starts at zero.
    pub fn for_range(
        start_time: Tick,
        end_time: Tick,
        frame_rate_num: u32,
        frame_rate_den: u32,
    ) -> Option<Self> {
        if frame_rate_num == 0 || frame_rate_den == 0 || end_time < start_time {
            return None;
        }
        let duration = end_time - start_time;
        let numerator = duration.as_ticks() as i128 * frame_rate_num as i128;
        let denominator = TICKS_PER_SECOND as i128 * frame_rate_den as i128;
        let frame_count = ((numerator + denominator / 2) / denominator).max(0) as u64;
        Some(Self { frame_rate_num, frame_rate_den, start_time, frame_count })
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn frame_time(&self, index: u64) -> Option<TimelineFrame> {
        if index >= self.frame_count {
            return None;
        }
        let numerator = index as i128 * TICKS_PER_SECOND as i128 * self.frame_rate_den as i128;
        let time = (numerator + self.frame_rate_num as i128 / 2) / self.frame_rate_num as i128;
        Some(TimelineFrame { index, time: self.start_time + Tick::from_ticks(time as i64) })
    }

    pub fn layers_at(&self, project: &Project, time: Tick) -> Vec<TimelineLayer> {
        let mut layers = Vec::new();
        for track in &project.tracks {
            if track.hidden || (track.kind == TrackKind::Audio && track.muted) {
                continue;
            }
            if let Some(items) = project.items(track.id) {
                for item in items.iter().filter(|item| item.enabled && item.start <= time && time < item.end()) {
                    layers.push(TimelineLayer {
                        track_id: track.id,
                        track_kind: track.kind,
                        item_id: item.id,
                        asset_id: item.asset_id,
                        source_time: item.source_in + (time - item.start),
                    });
                }
            }
        }
        layers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricodex_core::{Asset, AssetKind, Fingerprint, Item, Track};

    fn project_with_timeline() -> Project {
        let mut project = Project::new();
        project.assets.push(Asset {
            id: 1,
            fingerprint: Fingerprint::new(10, 1, 2, 3),
            path_hint: "clip.mp4".into(),
            kind: AssetKind::Video,
            duration: Tick::from_seconds(10.0),
            intro_trim: Tick::ZERO,
            outro_trim: Tick::ZERO,
        });
        project.add_track(Track::new(1, TrackKind::Video, "V1"));
        project.items_mut(1).unwrap().push(Item::new(
            7, 1, Tick::from_seconds(1.0), Tick::from_seconds(2.0), Tick::from_seconds(3.0),
        ));
        project
    }

    #[test]
    fn rational_schedule_does_not_accumulate_frame_rounding() {
        let mut project = project_with_timeline();
        // Exercise a long timeline so this checks accumulated rounding across
        // tens of thousands of frames rather than indexing beyond a short clip.
        project.assets[0].duration = Tick::from_seconds(1_100.0);
        project.items_mut(1).unwrap()[0].duration = Tick::from_seconds(1_000.0);
        let schedule = TimelineFrameSchedule::new(&project, 30_000, 1_001).unwrap();
        let frame = schedule.frame_time(29_999).unwrap();
        let expected = ((29_999i128 * TICKS_PER_SECOND as i128 * 1_001 + 15_000) / 30_000) as i64;
        assert_eq!(frame.time.as_ticks(), expected);
        assert!(schedule.frame_time(schedule.frame_count()).is_none());
    }

    #[test]
    fn bounded_schedule_keeps_absolute_project_timestamps() {
        let schedule = TimelineFrameSchedule::for_range(
            Tick::from_seconds(10.0),
            Tick::from_seconds(12.0),
            30,
            1,
        ).unwrap();
        assert_eq!(schedule.frame_count(), 60);
        assert_eq!(schedule.frame_time(0).unwrap().time, Tick::from_seconds(10.0));
        assert_eq!(schedule.frame_time(30).unwrap().time, Tick::from_seconds(11.0));
        assert!(TimelineFrameSchedule::for_range(
            Tick::from_seconds(2.0), Tick::from_seconds(1.0), 30, 1
        ).is_none());
    }

    #[test]
    fn timeline_sampling_preserves_gaps_and_source_coordinates() {
        let project = project_with_timeline();
        let schedule = TimelineFrameSchedule::new(&project, 30, 1).unwrap();
        assert!(schedule.layers_at(&project, Tick::from_seconds(0.5)).is_empty());
        let layer = schedule.layers_at(&project, Tick::from_seconds(1.5))[0];
        assert_eq!(layer.item_id, 7);
        assert_eq!(layer.source_time, Tick::from_seconds(3.5));
        assert!(schedule.layers_at(&project, Tick::from_seconds(3.0)).is_empty());
    }

    #[test]
    fn disabled_items_do_not_contribute_layers() {
        let mut project = project_with_timeline();
        project.items_mut(1).unwrap()[0].enabled = false;
        let schedule = TimelineFrameSchedule::new(&project, 30, 1).unwrap();
        assert!(schedule.layers_at(&project, Tick::from_seconds(1.5)).is_empty());
    }
}
