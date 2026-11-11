//! Edit commands as data, and the one way they are applied: clone the
//! project, apply the command to the clone, validate the clone, and only
//! then commit it. A rejected command changes nothing, which is what makes
//! snapshot undo trustworthy.

use serde::{Deserialize, Serialize};

use crate::model::{Item, ItemId, ModelError, Project, TrackId};
use crate::time::Tick;

/// One user edit, expressed as data so every front end (app, CLI, and any
/// future driver) performs edits in exactly the same way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Insert an item onto a track. The item group is kept sorted by start.
    InsertItem { track_id: TrackId, item: Item },
    /// Remove an item from whichever track holds it.
    RemoveItem { item_id: ItemId },
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
    SetItemEnabled { item_id: ItemId, enabled: bool },
    SetTrackMuted { track_id: TrackId, muted: bool },
    SetTrackLocked { track_id: TrackId, locked: bool },
}

/// Why a command was rejected. A rejection never changes the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    UnknownItem { item_id: ItemId },
    UnknownTrack { track_id: TrackId },
    TrackLocked { track_id: TrackId },
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
            CommandError::Invalid(e) => write!(f, "command would make the project invalid: {e}"),
        }
    }
}

impl std::error::Error for CommandError {}

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
                // A start change can disturb sorting; restore it here, and
                // let validation judge overlaps and durations.
                items.sort_by_key(|i| i.start);
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
