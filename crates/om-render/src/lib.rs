// SPDX-License-Identifier: Apache-2.0
//! The renderer.
//!
//! [`Compositor`] draws a project's surfaces into a linear-light,
//! premultiplied `Rgba16Float` canvas, then presents that canvas on any
//! target (display window, UI preview, readback) through a blit that
//! composites over black and applies the sRGB transfer function.
//!
//! The renderer only reads the project (`&Project`); it can never change
//! document state, so device loss or resize cannot corrupt a show.
//!
//! [`reference`] is an independent CPU implementation of the same maths used
//! to check GPU output in golden tests.

pub mod audio;
mod colour;
pub mod compare;
mod compositor;
pub mod effects;
pub mod isf;
pub mod output;
mod plan;
pub mod projection;
mod readback;
pub mod reference;

pub use colour::{linear_to_srgb, linear_to_srgb8, srgb_to_linear};
pub use compositor::{Compositor, FrameInputs, FrameReport, RenderError, ResourceCounts};
pub use plan::{
    Clip, DrawItem, MAX_MASK_VERTICES, Mapping, MaskShape, RenderPlan, SkipReason, plan,
};
pub use readback::FrameReader;
