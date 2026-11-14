//! Edit commands as data, and the one way they are applied: clone the
//! project, apply the command to the clone, validate the clone, and only
//! then commit it. A rejected command changes nothing, which is what makes
//! snapshot undo trustworthy.

use serde::{Deserialize, Serialize};

use crate::model::{Item, ItemId, ItemProvenance, ItemTransition, ModelError, Project, TrackId};
use crate::time::Tick;

/// One user edit, expressed as data so every front end (app, CLI, and any
/// future driver) performs edits in exactly the same way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Insert an item onto a track. The item group is kept sorted by start.
    InsertItem { track_id: TrackId, item: Item },
    /// Remove an item and ripple later items on its track left by its duration.
    RippleRemoveItem { item_id: ItemId },
    /// Remove an item from whichever track holds it, without shifting others.
    RemoveItem { item_id: ItemId },
    /// Split one item at an absolute timeline position. The right-hand item
    /// continues from the matching source position.
    SplitItem {
        item_id: ItemId,
        at: Tick,
        new_item_id: ItemId,
    },
    /// Reorder an item within its track, placing it immediately before the
    /// target item (or at the end when None). Reflow the track into a
    /// contiguous sequence while preserving each item's source range.
    ReorderItem {
        item_id: ItemId,
        before_item_id: Option<ItemId>,
    },
    /// Move an item to a new timeline start, optionally onto another track.
    MoveItem {
        item_id: ItemId,
        to_track: Option<TrackId>,
        new_start: Tick,
    },
    /// Trim an item: change its start, its source in point, or its duration.
    /// Fields left as None keep their current values.
    TrimItem {
        item_id: ItemId,
        new_start: Option<Tick>,
        new_source_in: Option<Tick>,
        new_duration: Option<Tick>,
    },
    /// Trim source/timeline duration and shift all later items by the same
    /// duration delta, preserving their relative spacing (ripple trim).
    RippleTrimItem {
        item_id: ItemId,
        new_source_in: Option<Tick>,
        new_duration: Option<Tick>,
    },
    SetItemEnabled { item_id: ItemId, enabled: bool },
    /// Pin or unpin a clip without changing its generated/manual origin.
    SetItemPinned { item_id: ItemId, pinned: bool },
    /// Set or clear the transition from the previous clip into this clip.
    SetItemTransition { item_id: ItemId, transition: Option<ItemTransition> },
    /// Mark an explicitly inserted clip as user-authored and protect it.
    MarkItemManual { item_id: ItemId },
    SetTrackMuted { track_id: TrackId, muted: bool },
    SetTrackLocked { track_id: TrackId, locked: bool },
}

/// Why a command was rejected. A rejection never changes the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    UnknownItem { item_id: ItemId },
    UnknownTrack { track_id: TrackId },
    TrackLocked { track_id: TrackId },
    InvalidSplitPoint { item_id: ItemId },
    InvalidReorderTarget { item_id: ItemId, target_item_id: ItemId },
    Invalid(ModelError),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::UnknownItem { item_id } => write!(f, "unknown item {item_id}"),
            CommandError::UnknownTrack { track_id } => write!(f, "unknown track {track_id}"),
            CommandError::TrackLocked { track_id } => {
                write!(f, "track {track_id} is locked")
            }
            CommandError::InvalidSplitPoint { item_id } => {
                write!(f, "split point must be inside item {item_id}")
            }
            CommandError::InvalidReorderTarget { item_id, target_item_id } => {
                write!(f, "item {item_id} cannot be reordered relative to item {target_item_id} on another track")
            }
            CommandError::Invalid(e) => write!(f, "command would make the project invalid: {e}"),
        }
    }
}

impl std::error::Error for CommandError {}

fn mark_manual(item: &mut Item) {
    item.provenance = ItemProvenance::Manual;
    item.pinned = true;
}

impl Project {
    /// Apply one command with clone, apply, validate, commit semantics.
    /// On error, self is returned untouched, alongside the error.
    pub fn apply(self, command: &Command) -> Result<Project, (Project, CommandError)> {
        let mut next = self.clone();
        match next.apply_in_place(command) {
            Ok(()) => match next.validate() {
                Ok(()) => Ok(next),
                Err(e) => Err((self, CommandError::Invalid(e))),
            },
            Err(e) => Err((self, e)),
        }
    }

    fn find_item_track(&self, item_id: ItemId) -> Option<TrackId> {
        self.track_items
            .iter()
            .find(|g| g.items.iter().any(|i| i.id == item_id))
            .map(|g| g.track_id)
    }

    fn apply_in_place(&mut self, command: &Command) -> Result<(), CommandError> {
        match command {
            Command::InsertItem { track_id, item } => {
                let track = self
                    .track(*track_id)
                    .ok_or(CommandError::UnknownTrack { track_id: *track_id })?;
                if track.locked {
                    return Err(CommandError::TrackLocked { track_id: *track_id });
                }
                let items = self
                    .items_mut(*track_id)
                    .ok_or(CommandError::UnknownTrack { track_id: *track_id })?;
                let position = items
                    .iter()
                    .position(|i| i.start > item.start)
                    .unwrap_or(items.len());
                items.insert(position, item.clone());
                Ok(())
            }
            Command::RippleRemoveItem { item_id } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|t| t.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                let items = self.items_mut(track_id).expect("track found above");
                let position = items
                    .iter()
                    .position(|item| item.id == *item_id)
                    .expect("item found above");
                let removed = items.remove(position);
                for item in items.iter_mut().skip(position) {
                    item.start = item.start - removed.duration;
                    // Ripple removal changes these clips' timeline placement.
                    // Protect their current source content during later refinement.
                    mark_manual(item);
                }
                Ok(())
            }
            Command::RemoveItem { item_id } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|t| t.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                let items = self.items_mut(track_id).expect("track found above");
                items.retain(|i| i.id != *item_id);
                Ok(())
            }
            Command::SplitItem {
                item_id,
                at,
                new_item_id,
            } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|t| t.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                if self.item(*new_item_id).is_some() {
                    return Err(CommandError::Invalid(ModelError::DuplicateItemId {
                        id: *new_item_id,
                    }));
                }
                let original = self
                    .items(track_id)
                    .and_then(|items| items.iter().find(|item| item.id == *item_id))
                    .cloned()
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if *at <= original.start || *at >= original.end() {
                    return Err(CommandError::InvalidSplitPoint { item_id: *item_id });
                }

                let left_duration = *at - original.start;
                let source_offset = *at - original.start;
                let mut right = Item::new(
                    *new_item_id,
                    original.asset_id,
                    *at,
                    original.end() - *at,
                    original.source_in + source_offset,
                );
                right.enabled = original.enabled;

                let items = self.items_mut(track_id).expect("track found above");
                let position = items
                    .iter()
                    .position(|item| item.id == *item_id)
                    .expect("item found above");
                items[position].duration = left_duration;
                mark_manual(&mut items[position]);
                mark_manual(&mut right);
                items.insert(position + 1, right);
                Ok(())
            }
            Command::ReorderItem { item_id, before_item_id } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|track| track.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                if let Some(target_item_id) = before_item_id {
                    if *target_item_id == *item_id {
                        return Err(CommandError::InvalidReorderTarget {
                            item_id: *item_id,
                            target_item_id: *target_item_id,
                        });
                    }
                    let target_track = self
                        .find_item_track(*target_item_id)
                        .ok_or(CommandError::UnknownItem { item_id: *target_item_id })?;
                    if target_track != track_id {
                        return Err(CommandError::InvalidReorderTarget {
                            item_id: *item_id,
                            target_item_id: *target_item_id,
                        });
                    }
                }

                let items = self.items_mut(track_id).expect("track found above");
                let origin = items.first().map(|item| item.start).unwrap_or(Tick::ZERO);
                let position = items
                    .iter()
                    .position(|item| item.id == *item_id)
                    .expect("item found above");
                let item = items.remove(position);
                let insert_at = before_item_id
                    .and_then(|target| items.iter().position(|entry| entry.id == target))
                    .unwrap_or(items.len());
                items.insert(insert_at, item);

                let mut cursor = origin;
                for entry in items.iter_mut() {
                    mark_manual(entry);
                    entry.start = cursor;
                    cursor = cursor + entry.duration;
                }
                Ok(())
            }
            Command::MoveItem {
                item_id,
                to_track,
                new_start,
            } => {
                let from_track = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                let target_track = to_track.unwrap_or(from_track);
                for track_id in [from_track, target_track] {
                    match self.track(track_id) {
                        None => return Err(CommandError::UnknownTrack { track_id }),
                        Some(t) if t.locked => {
                            return Err(CommandError::TrackLocked { track_id })
                        }
                        _ => {}
                    }
                }
                let items = self.items_mut(from_track).expect("track found above");
                let position = items
                    .iter()
                    .position(|i| i.id == *item_id)
                    .expect("item found above");
                let mut item = items.remove(position);
                item.start = *new_start;
                mark_manual(&mut item);
                let items = self.items_mut(target_track).expect("track found above");
                let position = items
                    .iter()
                    .position(|i| i.start > item.start)
                    .unwrap_or(items.len());
                items.insert(position, item);
                Ok(())
            }
            Command::TrimItem {
                item_id,
                new_start,
                new_source_in,
                new_duration,
            } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|t| t.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                let items = self.items_mut(track_id).expect("track found above");
                let item = items
                    .iter_mut()
                    .find(|i| i.id == *item_id)
                    .expect("item found above");
                if let Some(v) = new_start {
                    item.start = *v;
                }
                if let Some(v) = new_source_in {
                    item.source_in = *v;
                }
                if let Some(v) = new_duration {
                    item.duration = *v;
                }
                mark_manual(item);
                // A start change can disturb sorting; restore it here, and
                // let validation judge overlaps and durations.
                items.sort_by_key(|i| i.start);
                Ok(())
            }
            Command::RippleTrimItem {
                item_id,
                new_source_in,
                new_duration,
            } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|t| t.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                let items = self.items_mut(track_id).expect("track found above");
                let position = items
                    .iter()
                    .position(|item| item.id == *item_id)
                    .expect("item found above");
                let old_duration = items[position].duration;
                let next_duration = (*new_duration).unwrap_or(old_duration);
                let delta = next_duration - old_duration;
                if let Some(source_in) = new_source_in {
                    items[position].source_in = *source_in;
                }
                if new_duration.is_some() {
                    items[position].duration = next_duration;
                }
                mark_manual(&mut items[position]);
                for item in items.iter_mut().skip(position + 1) {
                    item.start = item.start + delta;
                    // The user's ripple trim also moves every following clip.
                    mark_manual(item);
                }
                Ok(())
            }
            Command::SetItemEnabled { item_id, enabled } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                let items = self.items_mut(track_id).expect("track found above");
                let item = items
                    .iter_mut()
                    .find(|i| i.id == *item_id)
                    .expect("item found above");
                item.enabled = *enabled;
                mark_manual(item);
                Ok(())
            }
            Command::SetItemPinned { item_id, pinned } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                let items = self.items_mut(track_id).expect("track found above");
                let item = items
                    .iter_mut()
                    .find(|item| item.id == *item_id)
                    .expect("item found above");
                item.pinned = *pinned;
                Ok(())
            }
            Command::SetItemTransition { item_id, transition } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                if self.track(track_id).map(|track| track.locked).unwrap_or(false) {
                    return Err(CommandError::TrackLocked { track_id });
                }
                let items = self.items_mut(track_id).expect("track found above");
                let item = items
                    .iter_mut()
                    .find(|item| item.id == *item_id)
                    .expect("item found above");
                item.transition_in = *transition;
                mark_manual(item);
                Ok(())
            }
            Command::MarkItemManual { item_id } => {
                let track_id = self
                    .find_item_track(*item_id)
                    .ok_or(CommandError::UnknownItem { item_id: *item_id })?;
                let items = self.items_mut(track_id).expect("track found above");
                let item = items
                    .iter_mut()
                    .find(|item| item.id == *item_id)
                    .expect("item found above");
                mark_manual(item);
                Ok(())
            }
            Command::SetTrackMuted { track_id, muted } => {
                let track = self
                    .track_mut(*track_id)
                    .ok_or(CommandError::UnknownTrack { track_id: *track_id })?;
                track.muted = *muted;
                Ok(())
            }
            Command::SetTrackLocked { track_id, locked } => {
                let track = self
                    .track_mut(*track_id)
                    .ok_or(CommandError::UnknownTrack { track_id: *track_id })?;
                track.locked = *locked;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::model::{Asset, AssetKind, Fingerprint, Track, TrackKind};

    fn project_with_one_clip() -> Project {
        let mut project = Project::new();
        project.assets.push(Asset {
            id: 1,
            fingerprint: Fingerprint::new(1024, 1, 2, 3),
            path_hint: "clip.mp4".to_string(),
            kind: AssetKind::Video,
            duration: Tick::from_seconds(20.0),
            intro_trim: Tick::ZERO,
            outro_trim: Tick::ZERO,
        });
        project.add_track(Track::new(1, TrackKind::Video, "V1"));
        project.items_mut(1).unwrap().push(Item::new(
            10,
            1,
            Tick::from_seconds(2.0),
            Tick::from_seconds(8.0),
            Tick::from_seconds(1.0),
        ));
        project.validate().unwrap();
        project
    }

    #[test]
    fn ripple_remove_closes_the_removed_clip_duration() {
        let mut project = project_with_one_clip();
        project.items_mut(1).unwrap().push(Item::new(
            12,
            1,
            Tick::from_seconds(10.0),
            Tick::from_seconds(3.0),
            Tick::from_seconds(8.0),
        ));
        project.validate().unwrap();
        let result = project.apply(&Command::RippleRemoveItem { item_id: 10 }).unwrap();
        let remaining = result.items(1).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 12);
        assert_eq!(remaining[0].start, Tick::from_seconds(2.0));
        assert_eq!(remaining[0].provenance, ItemProvenance::Manual);
        assert!(remaining[0].pinned);
        result.validate().unwrap();
    }

    #[test]
    fn ripple_trim_preserves_spacing_of_later_items() {
        let mut project = project_with_one_clip();
        project.items_mut(1).unwrap().push(Item::new(
            12,
            1,
            Tick::from_seconds(10.0),
            Tick::from_seconds(3.0),
            Tick::from_seconds(8.0),
        ));
        project.validate().unwrap();
        let shortened = project.clone().apply(&Command::RippleTrimItem {
            item_id: 10,
            new_source_in: None,
            new_duration: Some(Tick::from_seconds(6.0)),
        }).unwrap();
        assert_eq!(shortened.item(12).unwrap().start, Tick::from_seconds(8.0));
        assert_eq!(shortened.item(10).unwrap().duration, Tick::from_seconds(6.0));
        assert_eq!(shortened.item(12).unwrap().provenance, ItemProvenance::Manual);
        assert!(shortened.item(12).unwrap().pinned);
        shortened.validate().unwrap();

        let lengthened = project.apply(&Command::RippleTrimItem {
            item_id: 10,
            new_source_in: None,
            new_duration: Some(Tick::from_seconds(10.0)),
        }).unwrap();
        assert_eq!(lengthened.item(12).unwrap().start, Tick::from_seconds(12.0));
        lengthened.validate().unwrap();
    }

    #[test]
    fn ripple_trim_and_remove_participate_in_history() {
        let mut project = project_with_one_clip();
        project.items_mut(1).unwrap().push(Item::new(
            12,
            1,
            Tick::from_seconds(10.0),
            Tick::from_seconds(3.0),
            Tick::from_seconds(8.0),
        ));
        let original = project.clone();
        let mut history = History::new(project);
        history.apply(&Command::RippleTrimItem {
            item_id: 10,
            new_source_in: None,
            new_duration: Some(Tick::from_seconds(6.0)),
        }).unwrap();
        assert_eq!(history.project().item(12).unwrap().start, Tick::from_seconds(8.0));
        assert!(history.undo());
        assert_eq!(history.project().item(12).unwrap().start, Tick::from_seconds(10.0));
        assert!(history.redo());
        history.apply(&Command::RippleRemoveItem { item_id: 10 }).unwrap();
        assert_eq!(history.project().items(1).unwrap().len(), 1);
        assert!(history.undo());
        assert_eq!(history.project().item(10).unwrap().duration, Tick::from_seconds(6.0));
        assert_eq!(history.project().item(12).unwrap().start, Tick::from_seconds(8.0));
        assert_ne!(history.project(), &original);
    }

    #[test]
    fn reorder_moves_an_item_before_the_target_and_compacts_the_track() {
        let mut project = project_with_one_clip();
        project.items_mut(1).unwrap().push(Item::new(
            12,
            1,
            Tick::from_seconds(10.0),
            Tick::from_seconds(3.0),
            Tick::from_seconds(8.0),
        ));
        project.items_mut(1).unwrap().push(Item::new(
            13,
            1,
            Tick::from_seconds(13.0),
            Tick::from_seconds(4.0),
            Tick::from_seconds(2.0),
        ));
        project.validate().unwrap();

        let reordered = project.apply(&Command::ReorderItem {
            item_id: 13,
            before_item_id: Some(10),
        }).unwrap();
        let items = reordered.items(1).unwrap();
        assert_eq!(items.iter().map(|item| item.id).collect::<Vec<_>>(), vec![13, 10, 12]);
        assert_eq!(items[0].start, Tick::from_seconds(2.0));
        assert_eq!(items[1].start, Tick::from_seconds(6.0));
        assert_eq!(items[2].start, Tick::from_seconds(14.0));
        assert_eq!(items[0].source_in, Tick::from_seconds(2.0));
        assert_eq!(items[1].source_in, Tick::from_seconds(1.0));
        assert_eq!(items[2].source_in, Tick::from_seconds(8.0));
        reordered.validate().unwrap();
    }

    #[test]
    fn reorder_participates_in_history() {
        let mut project = project_with_one_clip();
        project.items_mut(1).unwrap().push(Item::new(
            12,
            1,
            Tick::from_seconds(10.0),
            Tick::from_seconds(3.0),
            Tick::from_seconds(8.0),
        ));
        let original = project.clone();
        let mut history = History::new(project);
        history.apply(&Command::ReorderItem {
            item_id: 12,
            before_item_id: Some(10),
        }).unwrap();
        assert_eq!(history.project().items(1).unwrap()[0].id, 12);
        assert!(history.undo());
        assert_eq!(history.project(), &original);
        assert!(history.redo());
        assert_eq!(history.project().items(1).unwrap()[0].id, 12);
    }

    #[test]
    fn reorder_rejects_items_on_other_tracks() {
        let mut project = project_with_one_clip();
        project.add_track(Track::new(2, TrackKind::Video, "V2"));
        project.items_mut(2).unwrap().push(Item::new(
            20,
            1,
            Tick::ZERO,
            Tick::from_seconds(2.0),
            Tick::ZERO,
        ));
        project.validate().unwrap();
        let (unchanged, error) = project.apply(&Command::ReorderItem {
            item_id: 10,
            before_item_id: Some(20),
        }).unwrap_err();
        assert_eq!(error, CommandError::InvalidReorderTarget {
            item_id: 10,
            target_item_id: 20,
        });
        assert_eq!(unchanged.items(1).unwrap()[0].id, 10);
    }


    #[test]
    fn pin_changes_participate_in_history() {
        let mut history = History::new(project_with_one_clip());
        history.apply(&Command::SetItemPinned {
            item_id: 10,
            pinned: true,
        }).unwrap();
        assert!(history.project().item(10).unwrap().pinned);
        assert!(history.undo());
        assert!(!history.project().item(10).unwrap().pinned);
        assert!(history.redo());
        assert!(history.project().item(10).unwrap().pinned);
    }


    #[test]
    fn mark_item_manual_and_pin() {
        let project = project_with_one_clip()
            .apply(&Command::MarkItemManual { item_id: 10 })
            .map_err(|(_, error)| error)
            .unwrap();
        let item = project.item(10).unwrap();
        assert_eq!(item.provenance, ItemProvenance::Manual);
        assert!(item.pinned);
    }

    #[test]
    fn split_preserves_timeline_and_source_continuity() {
        let split_at = Tick::from_seconds(6.0);
        let result = project_with_one_clip()
            .apply(&Command::SplitItem {
                item_id: 10,
                at: split_at,
                new_item_id: 11,
            })
            .unwrap();
        let items = result.items(1).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].start, Tick::from_seconds(2.0));
        assert_eq!(items[0].duration, Tick::from_seconds(4.0));
        assert_eq!(items[0].source_in, Tick::from_seconds(1.0));
        assert_eq!(items[1].start, Tick::from_seconds(6.0));
        assert_eq!(items[1].duration, Tick::from_seconds(4.0));
        assert_eq!(items[1].source_in, Tick::from_seconds(5.0));
        assert_eq!(items[0].end(), items[1].start);
        result.validate().unwrap();
    }

    #[test]
    fn split_rejects_points_at_or_outside_item_edges() {
        for split_at in [
            Tick::from_seconds(2.0),
            Tick::from_seconds(10.0),
            Tick::from_seconds(1.0),
        ] {
            let error = project_with_one_clip()
                .apply(&Command::SplitItem {
                    item_id: 10,
                    at: split_at,
                    new_item_id: 11,
                })
                .unwrap_err()
                .1;
            assert_eq!(error, CommandError::InvalidSplitPoint { item_id: 10 });
        }
    }

    #[test]
    fn split_rejects_duplicate_new_item_ids() {
        let error = project_with_one_clip()
            .apply(&Command::SplitItem {
                item_id: 10,
                at: Tick::from_seconds(6.0),
                new_item_id: 10,
            })
            .unwrap_err()
            .1;
        assert_eq!(
            error,
            CommandError::Invalid(ModelError::DuplicateItemId { id: 10 })
        );
    }

    #[test]
    fn split_participates_in_snapshot_undo_and_redo() {
        let mut history = History::new(project_with_one_clip());
        history
            .apply(&Command::SplitItem {
                item_id: 10,
                at: Tick::from_seconds(6.0),
                new_item_id: 11,
            })
            .unwrap();
        assert_eq!(history.project().items(1).unwrap().len(), 2);
        assert!(history.undo());
        assert_eq!(history.project().items(1).unwrap().len(), 1);
        assert!(history.redo());
        assert_eq!(history.project().items(1).unwrap().len(), 2);
    }
}
