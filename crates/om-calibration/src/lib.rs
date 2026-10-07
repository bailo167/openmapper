// SPDX-License-Identifier: Apache-2.0
//! Calibration maths for projection mapping: homographies fitted to point
//! correspondences, projector calibration from 3-D ↔ 2-D correspondences,
//! and soft-edge blend ramps for overlapping projectors.

pub mod blend;
pub mod homography;
pub mod linalg;
pub mod projector;

pub use homography::Homography;
pub use projector::{Calibration, Intrinsics, Pose, Projector, calibrate, calibrate_projection};

/// Why a calibration could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalibrationError {
    #[error("need at least {need} point pairs, got {got}")]
    TooFewPoints { need: usize, got: usize },
    #[error("point lists have different lengths")]
    Mismatched,
    #[error("points contain NaN or infinity")]
    NonFinite,
    #[error("points are degenerate (collinear or coincident)")]
    Degenerate,
    #[error("world points all lie on one plane; add points off that plane")]
    Coplanar,
}
