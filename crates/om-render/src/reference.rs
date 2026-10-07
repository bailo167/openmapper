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

    /// This image after an effect chain (same passes as the GPU).
    #[must_use]
    pub fn with_effects(&self, effects: &[om_project::EffectKind]) -> Self {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut px: Vec<[f32; 4]> = self
            .texels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for pass in crate::effects::passes(effects.iter()) {
            px = crate::effects::apply(&pass, &px, w, h);
        }
        Self {
            width: self.width,
            height: self.height,
            texels: px.into_iter().flatten().collect(),
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
    // Effect output per surface (computed once, shared by its items).
    let mut processed: HashMap<om_types::SurfaceId, RefImage> = HashMap::new();
    for item in &frame_plan.items {
        let Some(raw) = images.get(&item.media) else {
            continue;
        };
        let img = match &item.effects {
            Some(effects) => processed
                .entry(item.surface)
                .or_insert_with(|| raw.with_effects(effects)),
            None => raw,
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
                let m = item
                    .mask
                    .as_ref()
                    .map_or(1.0, |mask| mask_coverage(mask, px, py, w, h));
                // Coverage marks visible pixels, so mask edges join the edge band.
                if m > 0.0 {
                    coverage[i] = true;
                }
                let src = s.map(|v| v * item.opacity * m);
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

/// Mask coverage at pixel centre `(px, py)` of a `w × h` canvas; mirrors
/// mask.wgsl (signed distance in pixels, even-odd, linear feather).
#[must_use]
pub fn mask_coverage(mask: &crate::plan::MaskShape, px: u32, py: u32, w: u32, h: u32) -> f32 {
    let p = (f64::from(px) + 0.5, f64::from(py) + 0.5);
    let pts: Vec<(f64, f64)> = mask
        .polygon
        .iter()
        .map(|&(x, y)| {
            (
                f64::from((x * f64::from(w)) as f32),
                f64::from((y * f64::from(h)) as f32),
            )
        })
        .collect();
    let n = pts.len();
    let mut d2 = f64::MAX;
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let ab = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * ab.0 + (p.1 - a.1) * ab.1)
            / (ab.0 * ab.0 + ab.1 * ab.1).max(1e-12))
        .clamp(0.0, 1.0);
        let q = (a.0 + ab.0 * t - p.0, a.1 + ab.1 * t - p.1);
        d2 = d2.min(q.0 * q.0 + q.1 * q.1);
        if (a.1 > p.1) != (b.1 > p.1) {
            let x = (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0;
            if p.0 < x {
                inside = !inside;
            }
        }
    }
    let feather = mask.feather * f64::from(h);
    let mut c = if feather <= 0.0 {
        if inside { 1.0 } else { 0.0 }
    } else {
        let sd = if inside { d2.sqrt() } else { -d2.sqrt() };
        (0.5 + sd / feather).clamp(0.0, 1.0)
    };
    if mask.invert {
        c = 1.0 - c;
    }
    #[allow(clippy::cast_possible_truncation)]
    quantize_f16(c as f32)
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
