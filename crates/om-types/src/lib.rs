// SPDX-License-Identifier: Apache-2.0
//! Foundation types shared by every OpenMapper crate.
//!
//! This crate is layer 0: it must not depend on any other OpenMapper crate.

mod finite;
mod id;

pub use finite::{Finite, NonFiniteError, UnitInterval};
pub use id::{
    CueId, DmxNodeId, FixtureId, IdParseError, MediaId, ModulatorId, OutputId, ProjectId,
    SurfaceId, TimelineId,
};
