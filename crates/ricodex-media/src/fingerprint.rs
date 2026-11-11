//! Content fingerprints: identify a media file by what it is, not by
//! where it lives. The fingerprint is the music video-core [`Fingerprint`]
//! struct: file size plus a hash of the bytes sampled at the head,
//! middle and tail of the file. Equal files fingerprint equal by
//! construction, and the hash is a stable, documented, dependency free
//! function so a fingerprint computed today matches one computed by
//! any later build.
//!
//! The sample hash is FNV-1a 64 (offset basis 0xcbf29ce484222325,
//! prime 0x100000001b3): for each byte, xor then multiply. Each sample
//! covers up to 8192 bytes; the middle sample is centred on the middle
//! of the file, the tail sample ends at the last byte.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use ricodex_core::Fingerprint;

/// Bytes read for each of the head, middle and tail samples.
const SAMPLE_BYTES: u64 = 8192;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hash_sample(file: &mut File, start: u64, size: u64) -> std::io::Result<u64> {
    let length = SAMPLE_BYTES.min(size.saturating_sub(start)) as usize;
    let mut buffer = vec![0u8; length];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut buffer)?;
    Ok(fnv1a(&buffer))
}

/// Compute the fingerprint of a real file.
pub fn compute_fingerprint(path: &Path) -> std::io::Result<Fingerprint> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size == 0 {
        let empty = fnv1a(&[]);
        return Ok(Fingerprint::new(0, empty, empty, empty));
    }
    let head = hash_sample(&mut file, 0, size)?;
    let middle_start = (size / 2).saturating_sub(SAMPLE_BYTES / 2);
    let middle = hash_sample(&mut file, middle_start, size)?;
    let tail_start = size.saturating_sub(SAMPLE_BYTES);
    let tail = hash_sample(&mut file, tail_start, size)?;
    Ok(Fingerprint::new(size, head, middle, tail))
}

/// The cache key for a fingerprint: every field in fixed width hex, so
/// keys are stable file names.
pub fn fingerprint_key(fingerprint: &Fingerprint) -> String {
    format!(
        "{:016x}-{:016x}-{:016x}-{:016x}",
        fingerprint.size_bytes,
        fingerprint.head_hash,
        fingerprint.middle_hash,
        fingerprint.tail_hash
    )
}
