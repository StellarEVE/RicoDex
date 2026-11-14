//! Shared rational frame scheduling and timeline sampling primitives.
//!
//! Preview and export should ask this module which timeline instant a frame
//! represents. It deliberately contains no decoder or encoder dependencies.

use std::collections::HashSet;

use ricodex_core::{ItemTransition, Project, Tick, TrackId, TrackKind, TICKS_PER_SECOND};

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

/// One transition after its source interval has been mapped onto the shorter
/// output timeline. The source interval is twice as long as the output region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineTransitionSegment {
    pub track_id: TrackId,
    pub outgoing_item_id: u64,
    pub incoming_item_id: u64,
    pub kind: ricodex_core::TransitionKind,
    pub params: ricodex_core::TransitionParams,
    pub source_start: Tick,
    pub cut_time: Tick,
    pub source_end: Tick,
    pub output_start: Tick,
    pub duration: Tick,
}

/// Source coordinates needed to render one frame inside a transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineTransitionFrame {
    pub track_id: TrackId,
    pub outgoing_item_id: u64,
    pub incoming_item_id: u64,
    pub kind: ricodex_core::TransitionKind,
    pub params: ricodex_core::TransitionParams,
    pub outgoing_timeline_time: Tick,
    pub incoming_timeline_time: Tick,
    pub progress: f32,
}

/// Failure to construct an unambiguous transition-aware timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineTransitionMapError {
    InvalidRange,
    UnknownTrack { track_id: TrackId },
    InvalidTransition { incoming_item_id: u64 },
    OverlappingRegions {
        first_incoming_item_id: u64,
        second_incoming_item_id: u64,
    },
    ArithmeticOverflow,
}

impl std::fmt::Display for TimelineTransitionMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRange => write!(f, "transition map has an invalid timeline range"),
            Self::UnknownTrack { track_id } => write!(f, "transition map references unknown track {track_id}"),
            Self::InvalidTransition { incoming_item_id } => {
                write!(f, "incoming transition on item {incoming_item_id} is not valid for rendering")
            }
            Self::OverlappingRegions { first_incoming_item_id, second_incoming_item_id } => {
                write!(f, "transition regions for incoming items {first_incoming_item_id} and {second_incoming_item_id} overlap")
            }
            Self::ArithmeticOverflow => write!(f, "transition timeline arithmetic overflowed Tick range"),
        }
    }
}

impl std::error::Error for TimelineTransitionMapError {}

/// Maps an edited project timeline to the shorter rendered output timeline.
///
/// For an incoming transition of duration D at cut C, the outgoing tail
/// [C-D, C) and incoming head [C, C+D) form one output region of duration D.
/// The full source region [C-D, C+D) is compressed by D ticks. During that
/// region, tracks not undergoing the transition advance through source time
/// at 2x speed. The transitioning track blends its outgoing and incoming
/// sources at normal speed.
///
/// Each transition removes exactly D ticks from output duration. Overlapping
/// transition source regions across selected video tracks are rejected:
/// concurrent transitions do not have an unambiguous global time mapping.
#[derive(Clone, Debug)]
pub struct TimelineTransitionMap {
    start_time: Tick,
    output_end_time: Tick,
    total_compression: Tick,
    segments: Vec<TimelineTransitionSegment>,
}

impl TimelineTransitionMap {
    /// Build a global transition map for the exact video tracks selected for
    /// export. Hidden tracks and disabled transition endpoints are ignored.
    pub fn from_tracks(
        project: &Project,
        track_ids: &[TrackId],
        start_time: Tick,
        end_time: Tick,
    ) -> Result<Self, TimelineTransitionMapError> {
        if start_time < Tick::ZERO || end_time < start_time {
            return Err(TimelineTransitionMapError::InvalidRange);
        }

        let mut seen_tracks = HashSet::new();
        let mut segments = Vec::new();
        for track_id in track_ids {
            if !seen_tracks.insert(*track_id) {
                continue;
            }
            let track = project.track(*track_id)
                .ok_or(TimelineTransitionMapError::UnknownTrack { track_id: *track_id })?;
            if track.hidden || track.kind != TrackKind::Video {
                continue;
            }
            let items = project.items(*track_id)
                .ok_or(TimelineTransitionMapError::UnknownTrack { track_id: *track_id })?;

            for pair in items.windows(2) {
                let outgoing = &pair[0];
                let incoming = &pair[1];
                let transition: ItemTransition = match incoming.transition_in {
                    Some(value) if outgoing.enabled && incoming.enabled => value,
                    _ => continue,
                };
                if outgoing.end() != incoming.start
                    || transition.duration <= Tick::ZERO
                    || transition.duration >= outgoing.duration
                    || transition.duration >= incoming.duration
                {
                    return Err(TimelineTransitionMapError::InvalidTransition {
                        incoming_item_id: incoming.id,
                    });
                }

                let source_start = incoming.start.checked_sub(transition.duration)
                    .ok_or(TimelineTransitionMapError::ArithmeticOverflow)?;
                let source_end = incoming.start.checked_add(transition.duration)
                    .ok_or(TimelineTransitionMapError::ArithmeticOverflow)?;
                if source_start < start_time || source_end > end_time {
                    return Err(TimelineTransitionMapError::InvalidRange);
                }

                segments.push(TimelineTransitionSegment {
                    track_id: *track_id,
                    outgoing_item_id: outgoing.id,
                    incoming_item_id: incoming.id,
                    kind: transition.kind,
                    params: transition.params,
                    source_start,
                    cut_time: incoming.start,
                    source_end,
                    output_start: Tick::ZERO,
                    duration: transition.duration,
                });
            }
        }

        segments.sort_by_key(|segment| (segment.source_start, segment.source_end, segment.track_id));
        for pair in segments.windows(2) {
            if pair[1].source_start < pair[0].source_end {
                return Err(TimelineTransitionMapError::OverlappingRegions {
                    first_incoming_item_id: pair[0].incoming_item_id,
                    second_incoming_item_id: pair[1].incoming_item_id,
                });
            }
        }

        let mut total_compression = Tick::ZERO;
        for segment in &mut segments {
            segment.output_start = segment.source_start.checked_sub(total_compression)
                .ok_or(TimelineTransitionMapError::ArithmeticOverflow)?;
            total_compression = total_compression.checked_add(segment.duration)
                .ok_or(TimelineTransitionMapError::ArithmeticOverflow)?;
        }
        let output_end_time = end_time.checked_sub(total_compression)
            .ok_or(TimelineTransitionMapError::ArithmeticOverflow)?;
        if output_end_time < start_time {
            return Err(TimelineTransitionMapError::InvalidRange);
        }

        Ok(Self { start_time, output_end_time, total_compression, segments })
    }

    pub fn start_time(&self) -> Tick {
        self.start_time
    }

    pub fn output_end_time(&self) -> Tick {
        self.output_end_time
    }

    pub fn output_duration(&self) -> Tick {
        self.output_end_time - self.start_time
    }

    pub fn total_compression(&self) -> Tick {
        self.total_compression
    }

    pub fn segments(&self) -> &[TimelineTransitionSegment] {
        &self.segments
    }

    /// Build the rational output schedule over the compressed output range.
    pub fn frame_schedule(&self, frame_rate_num: u32, frame_rate_den: u32) -> Option<TimelineFrameSchedule> {
        TimelineFrameSchedule::for_range(
            self.start_time,
            self.output_end_time,
            frame_rate_num,
            frame_rate_den,
        )
    }

    /// Convert an absolute output timestamp into source project time for all
    /// tracks that are not themselves undergoing the active transition.
    /// During a transition this maps continuously through its full 2D source
    /// region at 2x speed. The output range is start-inclusive, end-exclusive.
    pub fn source_time_at(&self, output_time: Tick) -> Option<Tick> {
        if output_time < self.start_time || output_time >= self.output_end_time {
            return None;
        }

        let mut compression_before = Tick::ZERO;
        for segment in &self.segments {
            let output_region_end = segment.output_start.checked_add(segment.duration)?;
            if output_time < segment.output_start {
                return output_time.checked_add(compression_before);
            }
            if output_time < output_region_end {
                let elapsed = output_time.checked_sub(segment.output_start)?;
                return segment.source_start.checked_add(elapsed.checked_mul(2)?);
            }
            compression_before = compression_before.checked_add(segment.duration)?;
        }
        output_time.checked_add(compression_before)
    }

    /// Return both source timestamps and normalized blend progress if an
    /// output timestamp falls inside a transition region.
    pub fn transition_frame_at(&self, output_time: Tick) -> Option<TimelineTransitionFrame> {
        for segment in &self.segments {
            let output_region_end = segment.output_start.checked_add(segment.duration)?;
            if output_time >= segment.output_start && output_time < output_region_end {
                let elapsed = output_time.checked_sub(segment.output_start)?;
                let progress = elapsed.as_ticks() as f32 / segment.duration.as_ticks() as f32;
                return Some(TimelineTransitionFrame {
                    track_id: segment.track_id,
                    outgoing_item_id: segment.outgoing_item_id,
                    incoming_item_id: segment.incoming_item_id,
                    kind: segment.kind,
                    params: segment.params,
                    outgoing_timeline_time: segment.source_start.checked_add(elapsed)?,
                    incoming_timeline_time: segment.cut_time.checked_add(elapsed)?,
                    progress,
                });
            }
        }
        None
    }
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
    use ricodex_core::{
        Asset, AssetKind, Fingerprint, Item, ItemTransition, Track, TransitionKind,
        TransitionParams,
    };

    fn seconds(value: i64) -> Tick {
        Tick::from_ticks(value * TICKS_PER_SECOND)
    }

    fn project_with_timeline() -> Project {
        let mut project = Project::new();
        project.assets.push(Asset {
            id: 1,
            fingerprint: Fingerprint::new(10, 1, 2, 3),
            path_hint: "clip.mp4".into(),
            kind: AssetKind::Video,
            duration: seconds(20),
            intro_trim: Tick::ZERO,
            outro_trim: Tick::ZERO,
        });
        project.add_track(Track::new(1, TrackKind::Video, "V1"));
        project.items_mut(1).unwrap().push(Item::new(7, 1, seconds(1), seconds(2), seconds(3)));
        project
    }

    fn transition(kind: TransitionKind, duration: Tick) -> ItemTransition {
        ItemTransition { kind, params: TransitionParams::default(), duration }
    }

    #[test]
    fn rational_schedule_does_not_accumulate_frame_rounding() {
        let mut project = project_with_timeline();
        project.assets[0].duration = seconds(1_200);
        project.items_mut(1).unwrap()[0].duration = seconds(1_100);
        let schedule = TimelineFrameSchedule::new(&project, 30_000, 1_001).unwrap();
        let frame = schedule.frame_time(29_999).unwrap();
        let expected = ((29_999i128 * TICKS_PER_SECOND as i128 * 1_001 + 15_000) / 30_000) as i64;
        assert_eq!(frame.time.as_ticks(), expected);
        assert!(schedule.frame_time(schedule.frame_count()).is_none());
    }

    #[test]
    fn bounded_schedule_keeps_absolute_project_timestamps() {
        let schedule = TimelineFrameSchedule::for_range(seconds(10), seconds(12), 30, 1).unwrap();
        assert_eq!(schedule.frame_count(), 60);
        assert_eq!(schedule.frame_time(0).unwrap().time, seconds(10));
        assert_eq!(schedule.frame_time(30).unwrap().time, seconds(11));
        assert!(TimelineFrameSchedule::for_range(seconds(2), seconds(1), 30, 1).is_none());
    }

    #[test]
    fn timeline_sampling_preserves_gaps_and_source_coordinates() {
        let project = project_with_timeline();
        let schedule = TimelineFrameSchedule::new(&project, 30, 1).unwrap();
        assert!(schedule.layers_at(&project, seconds(0)).is_empty());
        let layer = schedule.layers_at(&project, seconds(2))[0];
        assert_eq!(layer.item_id, 7);
        assert_eq!(layer.source_time, seconds(4));
        assert!(schedule.layers_at(&project, seconds(3)).is_empty());
    }

    #[test]
    fn disabled_items_do_not_contribute_layers() {
        let mut project = project_with_timeline();
        project.items_mut(1).unwrap()[0].enabled = false;
        let schedule = TimelineFrameSchedule::new(&project, 30, 1).unwrap();
        assert!(schedule.layers_at(&project, seconds(2)).is_empty());
    }

    #[test]
    fn one_transition_compresses_two_source_halves_into_one_output_region() {
        let mut project = project_with_timeline();
        let items = project.items_mut(1).unwrap();
        items[0].start = seconds(0);
        items[0].duration = seconds(4);
        items[0].source_in = seconds(0);
        let mut incoming = Item::new(8, 1, seconds(4), seconds(4), seconds(4));
        incoming.transition_in = Some(transition(TransitionKind::CrossDissolve, seconds(1)));
        items.push(incoming);

        let map = TimelineTransitionMap::from_tracks(&project, &[1], seconds(0), seconds(8)).unwrap();
        assert_eq!(map.total_compression(), seconds(1));
        assert_eq!(map.output_end_time(), seconds(7));
        assert_eq!(map.segments().len(), 1);
        assert_eq!(map.segments()[0].source_start, seconds(3));
        assert_eq!(map.segments()[0].source_end, seconds(5));
        assert_eq!(map.segments()[0].output_start, seconds(3));
        assert_eq!(map.source_time_at(seconds(2)), Some(seconds(2)));
        assert_eq!(map.source_time_at(seconds(3)), Some(seconds(3)));
        assert_eq!(map.source_time_at(seconds(3) + Tick::from_ticks(TICKS_PER_SECOND / 2)), Some(seconds(4)));
        assert_eq!(map.source_time_at(seconds(4)), Some(seconds(5)));
        assert_eq!(map.source_time_at(seconds(6)), Some(seconds(7)));
        assert_eq!(map.source_time_at(seconds(7)), None);

        let frame = map.transition_frame_at(seconds(3) + Tick::from_ticks(TICKS_PER_SECOND / 2)).unwrap();
        assert_eq!(frame.outgoing_timeline_time, seconds(3) + Tick::from_ticks(TICKS_PER_SECOND / 2));
        assert_eq!(frame.incoming_timeline_time, seconds(4) + Tick::from_ticks(TICKS_PER_SECOND / 2));
        assert!((frame.progress - 0.5).abs() < 0.0001);
        assert_eq!(map.transition_frame_at(seconds(2)), None);
        assert_eq!(map.frame_schedule(30, 1).unwrap().frame_count(), 210);
    }

    #[test]
    fn sequential_non_overlapping_transitions_accumulate_compression_once() {
        let mut project = project_with_timeline();
        let items = project.items_mut(1).unwrap();
        items[0].start = seconds(0);
        items[0].duration = seconds(3);
        items[0].source_in = seconds(0);
        let mut middle = Item::new(8, 1, seconds(3), seconds(3), seconds(3));
        middle.transition_in = Some(transition(TransitionKind::CrossDissolve, Tick::from_ticks(TICKS_PER_SECOND / 2)));
        let mut last = Item::new(9, 1, seconds(6), seconds(3), seconds(6));
        last.transition_in = Some(transition(TransitionKind::DipToColor, Tick::from_ticks(TICKS_PER_SECOND / 2)));
        items.extend([middle, last]);

        let map = TimelineTransitionMap::from_tracks(&project, &[1], seconds(0), seconds(9)).unwrap();
        assert_eq!(map.total_compression(), seconds(1));
        assert_eq!(map.output_end_time(), seconds(8));
        assert_eq!(map.segments()[0].output_start, Tick::from_ticks(5 * TICKS_PER_SECOND / 2));
        assert_eq!(map.segments()[1].output_start, seconds(5));
        assert_eq!(map.source_time_at(seconds(5)), Some(Tick::from_ticks(11 * TICKS_PER_SECOND / 2)));
        assert_eq!(map.source_time_at(seconds(6)), Some(seconds(7)));
    }

    #[test]
    fn overlapping_cross_track_transition_regions_are_rejected() {
        let mut project = project_with_timeline();
        project.add_track(Track::new(2, TrackKind::Video, "V2"));
        let first = project.items_mut(1).unwrap();
        first[0].start = seconds(0);
        first[0].duration = seconds(4);
        first[0].source_in = seconds(0);
        let mut first_incoming = Item::new(8, 1, seconds(4), seconds(4), seconds(4));
        first_incoming.transition_in = Some(transition(TransitionKind::CrossDissolve, seconds(2)));
        first.push(first_incoming);

        let second = project.items_mut(2).unwrap();
        second.push(Item::new(10, 1, seconds(0), seconds(4), seconds(0)));
        let mut second_incoming = Item::new(11, 1, seconds(4), seconds(4), seconds(4));
        second_incoming.transition_in = Some(transition(TransitionKind::DipToColor, seconds(2)));
        second.push(second_incoming);

        let error = TimelineTransitionMap::from_tracks(&project, &[1, 2], seconds(0), seconds(8)).unwrap_err();
        assert!(matches!(error, TimelineTransitionMapError::OverlappingRegions { .. }));
    }

    #[test]
    fn hidden_tracks_and_disabled_transition_endpoints_do_not_compress_output() {
        let mut project = project_with_timeline();
        project.add_track(Track::new(2, TrackKind::Video, "V2"));
        let first = project.items_mut(1).unwrap();
        first[0].start = seconds(0);
        first[0].duration = seconds(4);
        first[0].source_in = seconds(0);
        let mut incoming = Item::new(8, 1, seconds(4), seconds(4), seconds(4));
        incoming.transition_in = Some(transition(TransitionKind::CrossDissolve, seconds(1)));
        first.push(incoming);

        project.tracks.iter_mut().find(|track| track.id == 2).unwrap().hidden = true;
        let hidden = project.items_mut(2).unwrap();
        hidden.push(Item::new(20, 1, seconds(0), seconds(4), seconds(0)));
        let mut hidden_incoming = Item::new(21, 1, seconds(4), seconds(4), seconds(4));
        hidden_incoming.transition_in = Some(transition(TransitionKind::CrossDissolve, seconds(1)));
        hidden.push(hidden_incoming);

        let hidden_map = TimelineTransitionMap::from_tracks(&project, &[1, 2], seconds(0), seconds(8)).unwrap();
        assert_eq!(hidden_map.segments().len(), 1);
        assert_eq!(hidden_map.total_compression(), seconds(1));

        project.items_mut(1).unwrap()[1].enabled = false;
        let disabled_map = TimelineTransitionMap::from_tracks(&project, &[1], seconds(0), seconds(8)).unwrap();
        assert_eq!(disabled_map.segments().len(), 0);
        assert_eq!(disabled_map.output_end_time(), seconds(8));
    }
}
