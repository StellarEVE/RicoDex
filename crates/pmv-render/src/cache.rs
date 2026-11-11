//! The content addressed render cache for transition regions.
//! Same pattern as the pmv-media analysis cache, per Decision 5:
//! derived data in one known cache home, outside any project file,
//! disposable at any time, rebuilt by rendering again.
//!
//! The cache key is a hash of everything that determines the unit's
//! pixels, and nothing else:
//!
//! 1. both source fingerprints (content identity, not paths),
//! 2. the source range of each side that the transition draws on
//!    (start tick and duration tick),
//! 3. the transition kind and its parameters (dip colour,
//!    direction),
//! 4. the transition duration in ticks,
//! 5. the output size and frame rate,
//! 6. the canonical description of the effect stack applied to the
//!    frames before compositing.
//!
//! Change any of those and the key changes; change nothing and a
//! lookup returns the stored frames without rendering. The unit is
//! stored as raw RGBA frames plus a small JSON sidecar carrying the
//! dimensions and the canonical key description, so a human can see
//! what a cache entry is.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use pmv_core::{Fingerprint, Tick};
use serde::{Deserialize, Serialize};

use crate::error::RenderError;
use crate::frame::Frame;
use crate::transition::{TransitionKind, TransitionParams};

/// Everything that determines one transition region's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionUnitKey {
    pub fingerprint_a: Fingerprint,
    pub fingerprint_b: Fingerprint,
    /// (source in, duration) of the outgoing side inside the region.
    pub range_a: (Tick, Tick),
    /// (source in, duration) of the incoming side inside the region.
    pub range_b: (Tick, Tick),
    pub transition: TransitionKind,
    pub params: TransitionParams,
    pub transition_duration: Tick,
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    /// Canonical effect stack description (see effects::describe_stack).
    pub effects: String,
}

impl TransitionUnitKey {
    /// The canonical description the key hashes. Field order is
    /// fixed; equal units describe equal, byte for byte.
    pub fn canonical(&self) -> String {
        let fp = |f: &Fingerprint| {
            format!(
                "{:016x}-{:016x}-{:016x}-{:016x}",
                f.size_bytes, f.head_hash, f.middle_hash, f.tail_hash
            )
        };
        format!(
            "transition-unit-v1|a={}|b={}|range_a={},{}|range_b={},{}|kind={}|params={}|dur={}|size={}x{}|fps={}/{}|effects={}",
            fp(&self.fingerprint_a),
            fp(&self.fingerprint_b),
            self.range_a.0.as_ticks(),
            self.range_a.1.as_ticks(),
            self.range_b.0.as_ticks(),
            self.range_b.1.as_ticks(),
            self.transition.name(),
            self.params.describe(),
            self.transition_duration.as_ticks(),
            self.width,
            self.height,
            self.frame_rate_num,
            self.frame_rate_den,
            self.effects,
        )
    }

    /// The cache key: FNV-1a 64 over the canonical description, the
    /// same stable hash the media fingerprints use, formatted as
    /// fixed width hex so keys are stable file names.
    pub fn cache_key(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in self.canonical().as_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

#[derive(Serialize, Deserialize)]
struct UnitMeta {
    width: u32,
    height: u32,
    frame_count: usize,
    canonical: String,
}

/// The render cache rooted at one directory.
pub struct RenderCache {
    home: PathBuf,
    renders: Cell<usize>,
}

impl RenderCache {
    pub fn new(home: impl Into<PathBuf>) -> RenderCache {
        RenderCache {
            home: home.into(),
            renders: Cell::new(0),
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// How many units this cache instance has actually rendered
    /// (cache misses). A second identical request that leaves this
    /// count unchanged was served from the cache: the proof the
    /// render job tests use.
    pub fn renders_performed(&self) -> usize {
        self.renders.get()
    }

    fn dir_for(&self, key: &TransitionUnitKey) -> PathBuf {
        self.home.join(key.cache_key())
    }

    /// The stored unit for a key, when present and complete.
    /// Reads no media and renders nothing.
    pub fn get_unit(&self, key: &TransitionUnitKey) -> Option<Vec<Frame>> {
        let dir = self.dir_for(key);
        let meta: UnitMeta =
            serde_json::from_str(&std::fs::read_to_string(dir.join("unit.json")).ok()?).ok()?;
        if meta.width != key.width || meta.height != key.height {
            return None;
        }
        let raw = std::fs::read(dir.join("unit.frames")).ok()?;
        let frame_bytes = key.width as usize * key.height as usize * 4;
        if raw.len() != frame_bytes * meta.frame_count {
            return None;
        }
        let mut frames = Vec::with_capacity(meta.frame_count);
        for chunk in raw.chunks_exact(frame_bytes) {
            frames.push(Frame {
                width: key.width,
                height: key.height,
                pixels: chunk.to_vec(),
            });
        }
        Some(frames)
    }

    /// Get a unit, rendering and storing it on a miss. Returns the
    /// frames and whether they came from the cache.
    pub fn render_unit(
        &self,
        key: &TransitionUnitKey,
        render: impl FnOnce() -> Result<Vec<Frame>, RenderError>,
    ) -> Result<(Vec<Frame>, bool), RenderError> {
        if let Some(hit) = self.get_unit(key) {
            return Ok((hit, true));
        }
        let frames = render()?;
        self.renders.set(self.renders.get() + 1);
        let dir = self.dir_for(key);
        std::fs::create_dir_all(&dir)?;
        let mut raw = Vec::with_capacity(
            frames.len() * key.width as usize * key.height as usize * 4,
        );
        for frame in &frames {
            raw.extend_from_slice(&frame.pixels);
        }
        std::fs::write(dir.join("unit.frames"), raw)?;
        let meta = UnitMeta {
            width: key.width,
            height: key.height,
            frame_count: frames.len(),
            canonical: key.canonical(),
        };
        std::fs::write(
            dir.join("unit.json"),
            serde_json::to_string_pretty(&meta)
                .map_err(|e| RenderError::BadFrame { reason: e.to_string() })?,
        )?;
        Ok((frames, false))
    }

    /// Empty the cache home: every stored unit goes, the home
    /// directory itself stays.
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
