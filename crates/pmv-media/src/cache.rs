//! The analysis cache: Stage 1 and Stage 2 results as JSON in one
//! known cache home, keyed by content fingerprint. This is derived
//! data under Decision 5: it lives outside any project file, can be
//! deleted at any time, and is rebuilt by reanalysis. A cache hit
//! returns results without touching the decoder at all.

use std::path::{Path, PathBuf};

use pmv_core::Fingerprint;

use crate::analysis::{analyse_stage1, analyse_stage2, SourceAnalysis, Stage1Output};
use crate::error::MediaError;
use crate::fingerprint::{compute_fingerprint, fingerprint_key};

/// A Stage 1 result served from, or stored into, the cache: the
/// analysis plus the poster and filmstrip paths inside the cache.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedStage1 {
    pub analysis: SourceAnalysis,
    pub poster_path: PathBuf,
    pub filmstrip_path: PathBuf,
}

/// The analysis cache rooted at one directory.
pub struct AnalysisCache {
    home: PathBuf,
}

impl AnalysisCache {
    pub fn new(home: impl Into<PathBuf>) -> AnalysisCache {
        AnalysisCache { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn dir_for(&self, fingerprint: &Fingerprint) -> PathBuf {
        self.home.join(fingerprint_key(fingerprint))
    }

    /// Stage 1 with caching: a hit returns the stored analysis and
    /// image paths without decoding; a miss runs the pass, writes the
    /// images and the JSON, and returns the same shape.
    pub fn stage1(&self, path: &Path) -> Result<CachedStage1, MediaError> {
        let fingerprint = compute_fingerprint(path)?;
        if let Some(hit) = self.get_stage1(&fingerprint) {
            return Ok(hit);
        }
        let dir = self.dir_for(&fingerprint);
        let output: Stage1Output = analyse_stage1(path, &dir)?;
        write_json(&dir.join("stage1.json"), &output.analysis)?;
        Ok(CachedStage1 {
            analysis: output.analysis,
            poster_path: output.poster_path,
            filmstrip_path: output.filmstrip_path,
        })
    }

    /// The cached Stage 1 result for a fingerprint, when present and
    /// complete (JSON plus both images). Reads no media file.
    pub fn get_stage1(&self, fingerprint: &Fingerprint) -> Option<CachedStage1> {
        let dir = self.dir_for(fingerprint);
        let analysis: SourceAnalysis = read_json(&dir.join("stage1.json")).ok()?;
        let poster_path = dir.join("poster.png");
        let filmstrip_path = dir.join("filmstrip.png");
        if poster_path.is_file() && filmstrip_path.is_file() {
            Some(CachedStage1 {
                analysis,
                poster_path,
                filmstrip_path,
            })
        } else {
            None
        }
    }

    /// Stage 2 with caching: a hit reports progress complete at once
    /// and returns the stored analysis without decoding; a miss runs
    /// the full pass with live progress and stores the result.
    pub fn stage2(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(f32),
    ) -> Result<SourceAnalysis, MediaError> {
        let fingerprint = compute_fingerprint(path)?;
        if let Some(hit) = self.get_stage2(&fingerprint) {
            progress(1.0);
            return Ok(hit);
        }
        let analysis = analyse_stage2(path, progress)?;
        let dir = self.dir_for(&fingerprint);
        std::fs::create_dir_all(&dir)?;
        write_json(&dir.join("stage2.json"), &analysis)?;
        Ok(analysis)
    }

    /// The cached Stage 2 result for a fingerprint, when present.
    /// Reads no media file.
    pub fn get_stage2(&self, fingerprint: &Fingerprint) -> Option<SourceAnalysis> {
        read_json(&self.dir_for(fingerprint).join("stage2.json")).ok()
    }

    /// Empty the cache home: every stored analysis and image goes,
    /// the home directory itself stays.
    pub fn purge(&self) -> std::io::Result<()> {
        if !self.home.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.home)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

fn write_json(path: &Path, analysis: &SourceAnalysis) -> Result<(), MediaError> {
    let text = serde_json::to_string_pretty(analysis)
        .map_err(|e| MediaError::Analysis { reason: e.to_string() })?;
    std::fs::write(path, text)?;
    Ok(())
}

fn read_json(path: &Path) -> Result<SourceAnalysis, MediaError> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| MediaError::Analysis {
        reason: e.to_string(),
    })
}
