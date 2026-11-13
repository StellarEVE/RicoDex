//! Project persistence: one readable JSON file, written atomically, with
//! a backup of the previous save kept beside it.
//!
//! Atomic save means the new content is written to a temporary file in
//! the same directory and then renamed over the target, so a crash mid
//! save can never leave a half written project. Before the rename, the
//! existing project file is copied to the same name plus a .bak suffix,
//! so the previous save survives every save.

use std::path::{Path, PathBuf};

use crate::model::{Project, SCHEMA_VERSION};

#[derive(Debug)]
pub enum PersistenceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The file on disk belongs to a newer version of the program.
    NewerSchema { found: u32, supported: u32 },
    /// The file loaded, but the project in it breaks a model invariant.
    Invalid(crate::model::ModelError),
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistenceError::Io(e) => write!(f, "project file error: {e}"),
            PersistenceError::Json(e) => write!(f, "project file is not valid project JSON: {e}"),
            PersistenceError::NewerSchema { found, supported } => write!(
                f,
                "project schema {found} is newer than this build supports ({supported}); \
                 open it with a newer version of the program"
            ),
            PersistenceError::Invalid(e) => write!(f, "project file breaks a model rule: {e}"),
        }
    }
}

impl std::error::Error for PersistenceError {}

impl From<std::io::Error> for PersistenceError {
    fn from(e: std::io::Error) -> Self {
        PersistenceError::Io(e)
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(e: serde_json::Error) -> Self {
        PersistenceError::Json(e)
    }
}

/// Path of the backup kept beside a project file: the same path with
/// ".bak" appended to the file name.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

impl Project {
    /// Serialise as pretty printed JSON.
    pub fn to_json(&self) -> Result<String, PersistenceError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse project JSON, checking the schema version and the model
    /// invariants before handing the project to anyone.
    pub fn from_json(text: &str) -> Result<Project, PersistenceError> {
        // Peek at the schema first, so a newer file gets its own clear
        // error instead of a confusing parse or validation failure.
        #[derive(serde::Deserialize)]
        struct SchemaPeek {
            schema_version: u32,
        }
        let peek: SchemaPeek = serde_json::from_str(text)?;
        if peek.schema_version > SCHEMA_VERSION {
            return Err(PersistenceError::NewerSchema {
                found: peek.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        let mut project: Project = serde_json::from_str(text)?;
        // Schema 1 did not persist source trims or the selected song.
        // Serde defaults the new fields; infer a song only when the legacy
        // project contains exactly one audio asset.
        if peek.schema_version < SCHEMA_VERSION {
            if project.song_asset_id.is_none() {
                let audio_ids: Vec<_> = project
                    .assets
                    .iter()
                    .filter(|asset| asset.kind == crate::model::AssetKind::Audio)
                    .map(|asset| asset.id)
                    .collect();
                if audio_ids.len() == 1 {
                    project.song_asset_id = Some(audio_ids[0]);
                }
            }
            project.schema_version = SCHEMA_VERSION;
        }
        project.validate().map_err(PersistenceError::Invalid)?;
        Ok(project)
    }

    /// Save atomically, keeping the previous save as a .bak beside the
    /// new one.
    pub fn save(&self, path: &Path) -> Result<(), PersistenceError> {
        self.validate().map_err(PersistenceError::Invalid)?;
        let text = self.to_json()?;

        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string());
        let temp_path = directory.join(format!(".{file_name}.tmp-{}", std::process::id()));

        std::fs::write(&temp_path, text)?;
        if path.exists() {
            std::fs::copy(path, backup_path(path))?;
        }
        match std::fs::rename(&temp_path, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                Err(PersistenceError::Io(e))
            }
        }
    }

    /// Load a project file saved by [`Project::save`].
    pub fn load(path: &Path) -> Result<Project, PersistenceError> {
        let text = std::fs::read_to_string(path)?;
        Project::from_json(&text)
    }
}
