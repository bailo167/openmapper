// SPDX-License-Identifier: Apache-2.0
//! Effect parameters shared by the GPU passes (effects.wgsl) and the CPU
//! reference, so both compute the same thing.

use om_project::EffectKind;

use crate::colour::{linear_to_srgb, quantize_f16, srgb_to_linear};

/// One GPU/CPU pass of an effect (blur is two passes).
#[derive(Debug, Clone, PartialEq)]
pub enum Pass {
    /// An ISF filter, identified by its stored path; `slot` is the effect's
    /// index in the chain (each slot keeps its own shader state).
    Isf {
        path: String,
        slot: usize,
        inputs: std::collections::BTreeMap<String, om_project::ShaderValue>,
    },
    Color {
        brightness: f32,
        contrast: f32,
        saturation: f32,
        gamma: f32,
        hue_cos: f32,
        hue_sin: f32,
    },
    Invert,
    BlurH {
        radius: i32,
        weights: [f32; 64],
    },
    BlurV {
        radius: i32,
        weights: [f32; 64],
    },
    Pixelate {
        size: i32,
    },
}

/// Normalised Gaussian weights for offsets 0..=r (radius = 3 sigma).
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn blur_weights(radius: f64) -> (i32, [f32; 64]) {
    let r = (radius.ceil() as i32).clamp(0, 63);
    let sigma = (radius / 3.0).max(1e-3);
    let mut w = [0f32; 64];
    let mut total = 0.0f64;
    for i in -r..=r {
        total += (-(f64::from(i * i)) / (2.0 * sigma * sigma)).exp();
    }
    for i in 0..=r {
        w[i as usize] = ((-(f64::from(i * i)) / (2.0 * sigma * sigma)).exp() / total) as f32;
    }
    (r, w)
}

/// Expands enabled effects into passes. Identity effects are dropped.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn passes<'a>(effects: impl IntoIterator<Item = &'a EffectKind>) -> Vec<Pass> {
    let mut out = Vec::new();
    for (slot, e) in effects.into_iter().enumerate() {
        match e {
            EffectKind::Color {
                brightness,
                contrast,
                saturation,
                hue,
                gamma,
            } => {
                let h = hue.get().to_radians();
                out.push(Pass::Color {
                    brightness: brightness.get() as f32,
                    contrast: contrast.get() as f32,
                    saturation: saturation.get() as f32,
                    gamma: gamma.get().max(0.1) as f32,
                    hue_cos: h.cos() as f32,
                    hue_sin: h.sin() as f32,
                });
            }
            EffectKind::Invert {} => out.push(Pass::Invert),
            EffectKind::Blur { radius } if radius.get() >= 0.5 => {
                let (radius, weights) = blur_weights(radius.get());
                out.push(Pass::BlurH { radius, weights });
                out.push(Pass::BlurV { radius, weights });
            }
            EffectKind::Blur { .. } => {}
            EffectKind::Pixelate { size } if *size > 1 => out.push(Pass::Pixelate {
                size: i32::from(*size),
            }),
            EffectKind::Pixelate { .. } => {}
            EffectKind::Shader { path, inputs } => out.push(Pass::Isf {
                path: path.clone(),
                slot,
                inputs: inputs.clone(),
            }),
        }
    }
    out
}

/// GPU uniform block for a pass (matches `Fx` in effects.wgsl).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn uniform(pass: &Pass) -> [f32; 72] {
    let mut u = [0f32; 72];
    match pass.clone() {
        Pass::Color {
            brightness,
            contrast,
            saturation,
            gamma,
            hue_cos,
            hue_sin,
        } => {
            u[..6].copy_from_slice(&[brightness, contrast, saturation, gamma, hue_cos, hue_sin]);
        }
        Pass::Invert => {}
        Pass::BlurH { radius, weights } | Pass::BlurV { radius, weights } => {
            u[4] = radius as f32;
            u[8..].copy_from_slice(&weights);
        }
        Pass::Pixelate { size } => u[4] = size as f32,
        Pass::Isf { .. } => {}
    }
    u
}

fn unpack(c: [f32; 4]) -> [f32; 3] {
    if c[3] <= 0.0 {
        return [0.0; 3];
    }
    [0, 1, 2].map(|k| linear_to_srgb(f64::from((c[k] / c[3]).clamp(0.0, 1.0))) as f32)
}

fn pack(s: [f32; 3], a: f32) -> [f32; 4] {
    let l = s.map(|v| srgb_to_linear(f64::from(v.clamp(0.0, 1.0))) as f32 * a);
    [l[0], l[1], l[2], a]
}

/// Converts a stored shader value for the ISF runtime.
#[must_use]
pub fn isf_value(v: &om_project::ShaderValue) -> crate::isf::IsfValue {
    match v {
        om_project::ShaderValue::Bool(b) => crate::isf::IsfValue::Bool(*b),
        om_project::ShaderValue::Number(n) => crate::isf::IsfValue::Number(n.get()),
        om_project::ShaderValue::Vector(v) => {
            crate::isf::IsfValue::Vector(v.iter().map(|f| f.get()).collect())
        }
    }
}

/// Applies `pass` to a `w × h` linear premultiplied image (CPU reference),
/// storing results as half floats like the GPU's intermediate textures.
#[must_use]
pub fn apply(pass: &Pass, src: &[[f32; 4]], w: usize, h: usize) -> Vec<[f32; 4]> {
    let at = |x: i64, y: i64| {
        let x = x.clamp(0, w as i64 - 1) as usize;
        let y = y.clamp(0, h as i64 - 1) as usize;
        src[y * w + x]
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let c = at(x, y);
            let v = match pass.clone() {
                Pass::Color {
                    brightness,
                    contrast,
                    saturation,
                    gamma,
                    hue_cos: co,
                    hue_sin: si,
                } => {
                    let mut s = unpack(c).map(|v| v + brightness);
                    s = s.map(|v| (v - 0.5) * contrast + 0.5);
                    let luma = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
                    s = s.map(|v| luma + (v - luma) * saturation);
                    let r0 = [
                        0.213 + co * 0.787 - si * 0.213,
                        0.715 - co * 0.715 - si * 0.715,
                        0.072 - co * 0.072 + si * 0.928,
                    ];
                    let r1 = [
                        0.213 - co * 0.213 + si * 0.143,
                        0.715 + co * 0.285 + si * 0.140,
                        0.072 - co * 0.072 - si * 0.283,
                    ];
                    let r2 = [
                        0.213 - co * 0.213 - si * 0.787,
                        0.715 - co * 0.715 + si * 0.715,
                        0.072 + co * 0.928 + si * 0.072,
                    ];
                    let dot = |r: [f32; 3]| r[0] * s[0] + r[1] * s[1] + r[2] * s[2];
                    s = [dot(r0), dot(r1), dot(r2)];
                    s = s.map(|v| v.max(0.0).powf(1.0 / gamma).clamp(0.0, 1.0));
                    pack(s, c[3])
                }
                Pass::Invert => pack(unpack(c).map(|v| 1.0 - v), c[3]),
                Pass::BlurH { radius, weights } | Pass::BlurV { radius, weights } => {
                    let horizontal = matches!(pass, Pass::BlurH { .. });
                    let mut acc = [0f32; 4];
                    for i in -radius..=radius {
                        let t = if horizontal {
                            at(x + i64::from(i), y)
                        } else {
                            at(x, y + i64::from(i))
                        };
                        let wgt = weights[i.unsigned_abs() as usize];
                        for k in 0..4 {
                            acc[k] += t[k] * wgt;
                        }
                    }
                    acc
                }
                Pass::Pixelate { size } => {
                    let n = i64::from(size);
                    at((x / n) * n + n / 2, (y / n) * n + n / 2)
                }
                // ISF shaders are not emulated on the CPU (tested against
                // analytic results instead); the reference treats them as
                // identity.
                Pass::Isf { .. } => c,
            };
            out.push(v.map(quantize_f16));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_weights_are_normalised_and_symmetric() {
        let (r, w) = blur_weights(9.0);
        assert_eq!(r, 9);
        let total: f32 = w[0] + 2.0 * w[1..=9].iter().sum::<f32>();
        assert!((total - 1.0).abs() < 1e-5);
        assert!(w[0] > w[1] && w[8] > w[9]);
    }

    #[test]
    fn neutral_colour_is_identity() {
        let ps = passes([&EffectKind::neutral_color()]);
        let img = vec![
            [0.25f32, 0.5, 0.125, 1.0],
            [0.0, 0.0, 0.0, 0.0],
            [0.1, 0.2, 0.3, 0.5],
        ];
        let out = apply(&ps[0], &img, 3, 1);
        for (a, b) in img.iter().zip(&out) {
            for k in 0..4 {
                assert!((a[k] - b[k]).abs() < 2e-3, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn identity_effects_produce_no_passes() {
        let none = passes([
            &EffectKind::Blur {
                radius: om_types::Finite::new(0.2).unwrap(),
            },
            &EffectKind::Pixelate { size: 1 },
        ]);
        assert!(none.is_empty());
    }
}
