//! The timeline model: assets, tracks, items, and the project that holds
//! them. Everything here is plain data. All rules about what makes a
//! project valid live in [`Project::validate`], and every edit goes
//! through the command layer, which validates before it commits.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::settings::{ExportSettings, GenerationSettings};
use crate::time::Tick;

/// The project file schema version written by this build.
pub const SCHEMA_VERSION: u32 = 3;

pub type AssetId = u64;
pub type TrackId = u64;
pub type ItemId = u64;

/// Identifies a media file by what it is, not by where it lives: the file
/// size plus hashes of samples taken at the head, middle and tail of the
/// file. The path stored on the asset is only a hint for finding it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size_bytes: u64,
    pub head_hash: u64,
    pub middle_hash: u64,
    pub tail_hash: u64,
}

impl Fingerprint {
    /// Build a fingerprint from explicit values. The media crate will
    /// compute these from a real file; the model only carries them.
    pub fn new(size_bytes: u64, head_hash: u64, middle_hash: u64, tail_hash: u64) -> Fingerprint {
        Fingerprint {
            size_bytes,
            head_hash,
            middle_hash,
            tail_hash,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Video,
    Audio,
    Image,
}

/// A piece of media the project knows about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub fingerprint: Fingerprint,
    /// A hint for locating the file, never its identity.
    pub path_hint: String,
    pub kind: AssetKind,
    /// Media duration, when known. Assets are probed before use, and a
    /// zero duration marks an asset that has not been probed yet.
    pub duration: Tick,
    /// Source trims applied before footage analysis. Defaulted when
    /// migrating schema-1 projects.
    #[serde(default)]
    pub intro_trim: Tick,
    #[serde(default)]
    pub outro_trim: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

/// One timeline track. Items live on the project, grouped by track id, so
/// track order here is the stacking and playback order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub locked: bool,
    pub hidden: bool,
}

impl Track {
    pub fn new(id: TrackId, kind: TrackKind, name: impl Into<String>) -> Track {
        Track {
            id,
            kind,
            name: name.into(),
            muted: false,
            locked: false,
            hidden: false,
        }
    }
}

/// Records whether a clip came from the planner or was directly edited by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemProvenance {
    Generated,
    Manual,
}

impl Default for ItemProvenance {
    fn default() -> Self {
        // Missing provenance in old project files is ambiguous. Default safely.
        Self::Manual
    }
}

/// One placed clip: a range of a source asset placed on a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub asset_id: AssetId,
    /// Start on the timeline.
    pub start: Tick,
    /// Length on the timeline. Must be positive.
    pub duration: Tick,
    /// Where in the source asset this clip begins.
    pub source_in: Tick,
    pub enabled: bool,
    /// Generated clips may be reconsidered by the planner. Manual clips are
    /// retained as user-authored project content.
    #[serde(default)]
    pub provenance: ItemProvenance,
    /// A pinned clip is not replaced during planner refinement.
    #[serde(default)]
    pub pinned: bool,
}

impl Item {
    pub fn new(id: ItemId, asset_id: AssetId, start: Tick, duration: Tick, source_in: Tick) -> Item {
        Item {
            id,
            asset_id,
            start,
            duration,
            source_in,
            enabled: true,
            provenance: ItemProvenance::Generated,
            pinned: false,
        }
    }

    /// End on the timeline, exclusive.
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// The items placed on one track, kept sorted by start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackItems {
    pub track_id: TrackId,
    pub items: Vec<Item>,
}

/// A whole project: one readable JSON document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub assets: Vec<Asset>,
    /// The audio asset selected as the generation song, if one is selected.
    #[serde(default)]
    pub song_asset_id: Option<AssetId>,
    pub tracks: Vec<Track>,
    /// Items grouped per track. Every track has exactly one entry here.
    pub track_items: Vec<TrackItems>,
    pub export_settings: ExportSettings,
    pub generation_settings: GenerationSettings,
}

impl Project {
    pub fn new() -> Project {
        Project {
            schema_version: SCHEMA_VERSION,
            assets: Vec::new(),
            song_asset_id: None,
            tracks: Vec::new(),
            track_items: Vec::new(),
            export_settings: ExportSettings::default(),
            generation_settings: GenerationSettings::default(),
        }
    }

    /// Add a track and its (empty) item group, keeping the two in step.
    pub fn add_track(&mut self, track: Track) {
        let id = track.id;
        self.tracks.push(track);
        self.track_items.push(TrackItems {
            track_id: id,
            items: Vec::new(),
        });
    }

    pub fn track(&self, track_id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == track_id)
    }

    pub fn track_mut(&mut self, track_id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == track_id)
    }

    pub fn items(&self, track_id: TrackId) -> Option<&Vec<Item>> {
        self.track_items
            .iter()
            .find(|g| g.track_id == track_id)
            .map(|g| &g.items)
    }

    pub fn items_mut(&mut self, track_id: TrackId) -> Option<&mut Vec<Item>> {
        self.track_items
            .iter_mut()
            .find(|g| g.track_id == track_id)
            .map(|g| &mut g.items)
    }

    pub fn item(&self, item_id: ItemId) -> Option<&Item> {
        self.track_items
            .iter()
            .flat_map(|g| g.items.iter())
            .find(|i| i.id == item_id)
    }

    pub fn asset_known(&self, asset_id: AssetId) -> bool {
        self.assets.iter().any(|a| a.id == asset_id)
    }

    /// Check every model invariant. Returns the first violation found as a
    /// typed error. A project that fails validation must never be saved or
    /// committed by the command layer.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ModelError::UnsupportedSchema {
                found: self.schema_version,
            });
        }

        let mut asset_ids = HashSet::new();
        for asset in &self.assets {
            if !asset_ids.insert(asset.id) {
                return Err(ModelError::DuplicateAssetId { id: asset.id });
            }
            if asset.intro_trim < Tick::ZERO || asset.outro_trim < Tick::ZERO {
                return Err(ModelError::NegativeAssetTrim { asset_id: asset.id });
            }
            if asset.duration > Tick::ZERO
                && asset.intro_trim + asset.outro_trim >= asset.duration
            {
                return Err(ModelError::AssetTrimExceedsDuration { asset_id: asset.id });
            }
        }

        if let Some(song_id) = self.song_asset_id {
            match self.assets.iter().find(|asset| asset.id == song_id) {
                None => return Err(ModelError::UnknownSongAsset { asset_id: song_id }),
                Some(asset) if asset.kind != AssetKind::Audio => {
                    return Err(ModelError::SongAssetNotAudio { asset_id: song_id });
                }
                Some(_) => {}
            }
        }

        let mut track_ids = HashSet::new();
        for track in &self.tracks {
            if !track_ids.insert(track.id) {
                return Err(ModelError::DuplicateTrackId { id: track.id });
            }
        }

        // Every track has exactly one item group, and no group is orphaned.
        if self.track_items.len() != self.tracks.len() {
            return Err(ModelError::TrackItemsMismatch);
        }
        for group in &self.track_items {
            if !track_ids.contains(&group.track_id) {
                return Err(ModelError::TrackItemsMismatch);
            }
        }

        let mut item_ids = HashSet::new();
        for group in &self.track_items {
            let items = &group.items;
            for (index, item) in items.iter().enumerate() {
                if !item_ids.insert(item.id) {
                    return Err(ModelError::DuplicateItemId { id: item.id });
                }
                if item.duration <= Tick::ZERO {
                    return Err(ModelError::NonPositiveDuration { id: item.id });
                }
                if item.start < Tick::ZERO {
                    return Err(ModelError::NegativeStart { id: item.id });
                }
                if item.source_in < Tick::ZERO {
                    return Err(ModelError::NegativeSourceIn { id: item.id });
                }
                if !asset_ids.contains(&item.asset_id) {
                    return Err(ModelError::UnknownAsset {
                        item_id: item.id,
                        asset_id: item.asset_id,
                    });
                }
                if let Some(asset) = self.assets.iter().find(|asset| asset.id == item.asset_id) {
                    if asset.duration > Tick::ZERO {
                        let source_end = asset.duration - asset.outro_trim;
                        if item.source_in < asset.intro_trim
                            || item.source_in > source_end
                            || item.duration > source_end - item.source_in
                        {
                            return Err(ModelError::ItemOutsideAssetTrimWindow {
                                item_id: item.id,
                                asset_id: item.asset_id,
                            });
                        }
                    }
                }
                if index > 0 {
                    let previous = &items[index - 1];
                    if item.start < previous.start {
                        return Err(ModelError::ItemsNotSorted {
                            track_id: group.track_id,
                        });
                    }
                    if item.start < previous.end() {
                        return Err(ModelError::ItemsOverlap {
                            track_id: group.track_id,
                            first: previous.id,
                            second: item.id,
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for Project {
    fn default() -> Self {
        Project::new()
    }
}

/// One specific way a project can be invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    UnsupportedSchema { found: u32 },
    DuplicateAssetId { id: AssetId },
    UnknownSongAsset { asset_id: AssetId },
    SongAssetNotAudio { asset_id: AssetId },
    NegativeAssetTrim { asset_id: AssetId },
    AssetTrimExceedsDuration { asset_id: AssetId },
    ItemOutsideAssetTrimWindow { item_id: ItemId, asset_id: AssetId },
    DuplicateTrackId { id: TrackId },
    DuplicateItemId { id: ItemId },
    TrackItemsMismatch,
    NonPositiveDuration { id: ItemId },
    NegativeStart { id: ItemId },
    NegativeSourceIn { id: ItemId },
    UnknownAsset { item_id: ItemId, asset_id: AssetId },
    ItemsNotSorted { track_id: TrackId },
    ItemsOverlap { track_id: TrackId, first: ItemId, second: ItemId },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::UnsupportedSchema { found } => {
                write!(f, "unsupported schema version {found}")
            }
            ModelError::DuplicateAssetId { id } => write!(f, "duplicate asset id {id}"),
            ModelError::UnknownSongAsset { asset_id } => write!(f, "song selection references unknown asset {asset_id}"),
            ModelError::SongAssetNotAudio { asset_id } => write!(f, "selected song asset {asset_id} is not audio"),
            ModelError::NegativeAssetTrim { asset_id } => write!(f, "asset {asset_id} has a negative source trim"),
            ModelError::AssetTrimExceedsDuration { asset_id } => write!(f, "asset {asset_id} source trims leave no usable duration"),
            ModelError::ItemOutsideAssetTrimWindow { item_id, asset_id } => write!(f, "item {item_id} uses source frames outside asset {asset_id}'s trimmed source window"),
            ModelError::DuplicateTrackId { id } => write!(f, "duplicate track id {id}"),
            ModelError::DuplicateItemId { id } => write!(f, "duplicate item id {id}"),
            ModelError::TrackItemsMismatch => {
                write!(f, "track item groups do not match the track list")
            }
            ModelError::NonPositiveDuration { id } => {
                write!(f, "item {id} has a non positive duration")
            }
            ModelError::NegativeStart { id } => write!(f, "item {id} has a negative start"),
            ModelError::NegativeSourceIn { id } => {
                write!(f, "item {id} has a negative source in point")
            }
            ModelError::UnknownAsset { item_id, asset_id } => {
                write!(f, "item {item_id} points at unknown asset {asset_id}")
            }
            ModelError::ItemsNotSorted { track_id } => {
                write!(f, "items on track {track_id} are not sorted by start")
            }
            ModelError::ItemsOverlap { track_id, first, second } => {
                write!(f, "items {first} and {second} overlap on track {track_id}")
            }
        }
    }
}

impl std::error::Error for ModelError {}
