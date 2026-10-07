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
use crate::plan::{RenderPlan, plan};

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
                let Some(uv) = item.canvas_to_uv.apply(p) else {
                    continue;
                };
                let s = img.sample(uv);
                let i = (py * w + px) as usize;
                coverage[i] = true;
                let dst = &mut canvas[i];
                let src_a = s[3] * item.opacity;
                for k in 0..4 {
                    let src = s[k] * item.opacity;
                    dst[k] = quantize_f16(src + dst[k] * (1.0 - src_a));
                }
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

impl crate::plan::DrawItem {
    fn polygon_points(&self) -> Vec<om_geom::Point2> {
        self.polygon
            .iter()
            .filter_map(|&(x, y)| om_geom::Point2::new(x, y).ok())
            .collect()
    }
}
