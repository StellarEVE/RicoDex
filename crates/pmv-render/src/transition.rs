//! Transitions as data: the named forms from the product
//! specification, plus the small parameter set they share. The CPU
//! compositor in the composite module implements every kind; the GPU
//! compositor implements a subset and reports which ones.

use serde::{Deserialize, Serialize};

/// One named transition form. The names are the product
/// specification's twelve (Cross Dissolve counted once, Dip to Color
/// carrying black and white as a parameter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionKind {
    CrossDissolve,
    DipToColor,
    FilmDissolve,
    DipToDissolve,
    CrashZoom,
    DirectionalZoom,
    TwistWheel,
    RadialTwist,
    Push,
    Slide,
    Split,
}

impl TransitionKind {
    /// A stable name used in cache keys and descriptions.
    pub fn name(&self) -> &'static str {
        match self {
            TransitionKind::CrossDissolve => "cross-dissolve",
            TransitionKind::DipToColor => "dip-to-color",
            TransitionKind::FilmDissolve => "film-dissolve",
            TransitionKind::DipToDissolve => "dip-to-dissolve",
            TransitionKind::CrashZoom => "crash-zoom",
            TransitionKind::DirectionalZoom => "directional-zoom",
            TransitionKind::TwistWheel => "twist-wheel",
            TransitionKind::RadialTwist => "radial-twist",
            TransitionKind::Push => "push",
            TransitionKind::Slide => "slide",
            TransitionKind::Split => "split",
        }
    }
}

/// Parameters shared by the transition forms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransitionParams {
    /// The colour Dip to Color passes through, as RGB. Black is the
    /// classic dip; white gives the Dip to White variant.
    pub dip_color: [u8; 3],
    /// Direction of travel for the directional forms, as a unit
    /// vector in frame space: Push and Slide enter from the side
    /// opposite this vector, Directional Zoom drifts its focal point
    /// along it. The default (1, 0) is left to right travel.
    pub direction: (f32, f32),
}

impl Default for TransitionParams {
    fn default() -> Self {
        TransitionParams {
            dip_color: [0, 0, 0],
            direction: (1.0, 0.0),
        }
    }
}

impl TransitionParams {
    pub fn dip_black() -> TransitionParams {
        TransitionParams::default()
    }

    pub fn dip_white() -> TransitionParams {
        TransitionParams {
            dip_color: [255, 255, 255],
            direction: (1.0, 0.0),
        }
    }

    /// A canonical description for cache keys: every field, fixed
    /// formatting, so equal parameters describe equal.
    pub fn describe(&self) -> String {
        format!(
            "dip={:02x}{:02x}{:02x};dir={:.4},{:.4}",
            self.dip_color[0], self.dip_color[1], self.dip_color[2], self.direction.0,
            self.direction.1
        )
    }
}
