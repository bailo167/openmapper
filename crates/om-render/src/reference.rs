// SPDX-License-Identifier: Apache-2.0
//! CPU reference renderer.
//!
//! An independent, deliberately simple implementation of the compositor's
//! maths, used as the golden source in tests: per-pixel-centre coverage,
//! canvas→UV homography, bilinear clamp-to-edge sampling of linear
//! premultiplied half-float texels, premultiplied "over" blending with
//! half-float storage after each surface, then composite over black and
//! sRGB encoding.

use std::collections::HashMap;

use om_geom::convex_contains;
use om_media_core::StillImage;
use om_project::Project;
use om_types::MediaId;

use crate::colour::{
    f16_bytes_to_f32, linear_to_srgb8, quantize_f16, rgba8_srgb_to_linear_premul_f16,
};
use crate::plan::{Clip, RenderPlan, plan};
use om_project::BlendMode;

/// A media image converted to the renderer's texel format.
#[derive(Debug, Clone)]
pub struct RefImage {
    width: u32,
    height: u32,
    texels: Vec<f32>,
}

impl RefImage {
    #[must_use]
    pub fn new(image: &StillImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            texels: f16_bytes_to_f32(&rgba8_srgb_to_linear_premul_f16(image.rgba8())),
        }
    }

    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, i64::from(self.width) - 1) as usize;
        let y = y.clamp(0, i64::from(self.height) - 1) as usize;
        let i = (y * self.width as usize + x) * 4;
        [
            self.texels[i],
            self.texels[i + 1],
            self.texels[i + 2],
            self.texels[i + 3],
        ]
    }

    /// Bilinear sample at normalised `uv` with clamp-to-edge addressing.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn sample(&self, uv: (f64, f64)) -> [f32; 4] {
        let tx = uv.0 * f64::from(self.width) - 0.5;
        let ty = uv.1 * f64::from(self.height) - 0.5;
        let (x0, y0) = (tx.floor(), ty.floor());
        let (fx, fy) = ((tx - x0) as f32, (ty - y0) as f32);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            out[k] = top + (bottom - top) * fy;
        }
        out
    }
}

/// Reference output: sRGB RGBA8 pixels plus per-pixel coverage.
#[derive(Debug, Clone)]
pub struct RefFrame {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    /// True where at least one surface covers the pixel centre.
    pub coverage: Vec<bool>,
    pub plan: RenderPlan,
}

/// Renders `project` on the CPU.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn render(project: &Project, images: &HashMap<MediaId, RefImage>) -> RefFrame {
    let (w, h) = (project.canvas.width, project.canvas.height);
    let frame_plan = plan(project, |m| images.contains_key(&m));
    let n = (w as usize) * (h as usize);
    let mut canvas = vec![[0.0f32; 4]; n];
    let mut coverage = vec![false; n];
    for item in &frame_plan.items {
        let Some(img) = images.get(&item.media) else {
            continue;
        };
        for py in 0..h {
            for px in 0..w {
                let p = (
                    (f64::from(px) + 0.5) / f64::from(w),
                    (f64::from(py) + 0.5) / f64::from(h),
                );
                if !convex_contains(&item.polygon_points(), p) {
                    continue;
                }
                if let Clip::Ellipse { canvas_to_local } = item.clip {
                    let Some((lx, ly)) = canvas_to_local.apply(p) else {
                        continue;
                    };
                    let (dx, dy) = (lx * 2.0 - 1.0, ly * 2.0 - 1.0);
                    if dx * dx + dy * dy > 1.0 {
                        continue;
                    }
                }
                let Some(uv) = item.mapping.apply(p) else {
                    continue;
                };
                let s = img.sample(uv);
                let i = (py * w + px) as usize;
                coverage[i] = true;
                let src = s.map(|v| v * item.opacity);
                canvas[i] = blend(item.blend, src, canvas[i]).map(quantize_f16);
            }
        }
    }
    let mut rgba8 = Vec::with_capacity(n * 4);
    for px in &canvas {
        for c in &px[..3] {
            rgba8.push(linear_to_srgb8(f64::from(*c)));
        }
        rgba8.push(255);
    }
    RefFrame {
        width: w,
        height: h,
        rgba8,
        coverage,
        plan: frame_plan,
    }
}

/// Premultiplied blend equations matching the GPU blend states.
#[must_use]
pub fn blend(mode: BlendMode, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let a = src[3];
    let mut out = [0.0; 4];
    for k in 0..3 {
        out[k] = match mode {
            BlendMode::Normal => src[k] + dst[k] * (1.0 - a),
            BlendMode::Add => src[k] + dst[k],
            BlendMode::Screen => src[k] + dst[k] * (1.0 - src[k]),
            BlendMode::Multiply => src[k] * dst[k] + dst[k] * (1.0 - a),
        };
    }
    out[3] = a + dst[3] * (1.0 - a);
    out
}

impl crate::plan::DrawItem {
    fn polygon_points(&self) -> Vec<om_geom::Point2> {
        self.polygon
            .iter()
            .filter_map(|&(x, y)| om_geom::Point2::new(x, y).ok())
            .collect()
    }
}
