//! hyperdex-core: the timeline model and edit commands for the HyperDex
//! (working title) editor.
//!
//! This crate is the bottom of the workspace. It knows nothing about
//! decoding, rendering or any user interface: it holds the project as
//! plain data, enforces the model invariants, applies edits as commands
//! with clone, validate, commit semantics, keeps snapshot undo history,
//! and reads and writes the single readable JSON project file.

pub mod command;
pub mod history;
pub mod model;
pub mod persistence;
pub mod settings;
pub mod time;

pub use command::{Command, CommandError};
pub use history::History;
pub use model::{
    Asset, AssetId, AssetKind, Fingerprint, Item, ItemId, ItemProvenance, ItemTransition,
    ModelError, Project, Track, TrackId, TrackItems, TrackKind, TransitionKind,
    TransitionParams, SCHEMA_VERSION,
};
pub use persistence::{backup_path, PersistenceError};
pub use settings::{ExportSettings, GenerationSettings};
pub use time::{Tick, TICKS_PER_SECOND};
