//! Exact integer time for all edit maths.
//!
//! One tick is 1 / 254_016_000_000 of a second. The rate divides evenly by
//! the 44,100 and 48,000 audio sample rates and by every common frame rate,
//! including the rational NTSC rates such as 29.97 (30000/1001), so edit
//! maths never accumulates floating point error. Floating point seconds
//! exist only at the edges, for display and for talking to outside code,
//! and are never stored in the model.

use serde::{Deserialize, Serialize};

/// Ticks per second for all edit maths in the project.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// An exact point in time, or an exact duration, in ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tick(pub i64);

impl Tick {
    pub const ZERO: Tick = Tick(0);

    pub fn from_ticks(ticks: i64) -> Tick {
        Tick(ticks)
    }

    pub fn as_ticks(self) -> i64 {
        self.0
    }

    /// Convert from floating point seconds. Edge use only: the result is
    /// rounded to the nearest tick and the float is never stored.
    pub fn from_seconds(seconds: f64) -> Tick {
        Tick((seconds * TICKS_PER_SECOND as f64).round() as i64)
    }

    /// Convert to floating point seconds. Edge use only.
    pub fn to_seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    /// Exact tick for a whole number of audio samples at `sample_rate`.
    /// Returns None if the sample count does not land on whole ticks, which
    /// cannot happen for the standard 44,100 and 48,000 rates.
    pub fn from_samples(samples: i64, sample_rate: u32) -> Option<Tick> {
        let numerator = (samples as i128) * (TICKS_PER_SECOND as i128);
        let denominator = sample_rate as i128;
        if numerator % denominator != 0 {
            return None;
        }
        i64::try_from(numerator / denominator).ok().map(Tick)
    }

    /// Whole audio samples at `sample_rate` for this tick, rounded down.
    pub fn to_samples_floor(self, sample_rate: u32) -> i64 {
        ((self.0 as i128) * (sample_rate as i128) / (TICKS_PER_SECOND as i128)) as i64
    }

    /// Exact tick at the start of frame `frame` for a rational frame rate
    /// given as numerator over denominator, for example 30000/1001 for
    /// 29.97 fps. Returns None if the frame does not land on a whole tick.
    pub fn from_frame(frame: i64, rate_num: u32, rate_den: u32) -> Option<Tick> {
        if rate_num == 0 || rate_den == 0 {
            return None;
        }
        let numerator = (frame as i128) * (TICKS_PER_SECOND as i128) * (rate_den as i128);
        let denominator = rate_num as i128;
        if numerator % denominator != 0 {
            return None;
        }
        i64::try_from(numerator / denominator).ok().map(Tick)
    }

    /// Duration of one frame at a rational frame rate, in ticks, when the
    /// rate divides the tick rate evenly.
    pub fn frame_duration(rate_num: u32, rate_den: u32) -> Option<Tick> {
        Tick::from_frame(1, rate_num, rate_den)
    }

    pub fn checked_add(self, other: Tick) -> Option<Tick> {
        self.0.checked_add(other.0).map(Tick)
    }

    pub fn checked_sub(self, other: Tick) -> Option<Tick> {
        self.0.checked_sub(other.0).map(Tick)
    }

    pub fn checked_mul(self, factor: i64) -> Option<Tick> {
        self.0.checked_mul(factor).map(Tick)
    }

    /// Saturating addition for UI facing code that prefers clamping over
    /// error handling.
    pub fn saturating_add(self, other: Tick) -> Tick {
        Tick(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Tick) -> Tick {
        Tick(self.0.saturating_sub(other.0))
    }
}

impl std::ops::Add for Tick {
    type Output = Tick;
    fn add(self, rhs: Tick) -> Tick {
        Tick(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Tick {
    type Output = Tick;
    fn sub(self, rhs: Tick) -> Tick {
        Tick(self.0 - rhs.0)
    }
}
