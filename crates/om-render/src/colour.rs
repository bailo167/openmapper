// SPDX-License-Identifier: Apache-2.0
//! Transfer functions and texel conversion shared by the GPU upload path and
//! the CPU reference renderer, so both start from identical texel values.

use half::f16;

/// sRGB-encoded value (0..=1) to linear light.
#[must_use]
pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light (0..=1) to sRGB-encoded.
#[must_use]
pub fn linear_to_srgb(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear (0..=1) to an 8-bit sRGB code value, rounded to nearest.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn linear_to_srgb8(v: f64) -> u8 {
    (linear_to_srgb(v) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Rounds to the nearest representable half-float, as the GPU stores it.
#[must_use]
pub fn quantize_f16(v: f32) -> f32 {
    f16::from_f32(v).to_f32()
}

/// Converts sRGB straight-alpha RGBA8 to linear premultiplied RGBA f16
/// (little-endian bytes, the `Rgba16Float` texel layout).
#[must_use]
pub fn rgba8_srgb_to_linear_premul_f16(rgba8: &[u8]) -> Vec<u8> {
    let lut: Vec<f32> = (0..256)
        .map(|i| srgb_to_linear(f64::from(i) / 255.0) as f32)
        .collect();
    let mut out = Vec::with_capacity(rgba8.len() * 2);
    for px in rgba8.as_chunks::<4>().0 {
        let a = f32::from(px[3]) / 255.0;
        for v in [
            lut[usize::from(px[0])] * a,
            lut[usize::from(px[1])] * a,
            lut[usize::from(px[2])] * a,
            a,
        ] {
            out.extend_from_slice(&f16::from_f32(v).to_le_bytes());
        }
    }
    out
}

/// Decodes `Rgba16Float` texel bytes back to f32.
#[must_use]
pub fn f16_bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f16::from_le_bytes([b[0], b[1]]).to_f32())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips_every_code() {
        for i in 0..=255u8 {
            let lin = srgb_to_linear(f64::from(i) / 255.0);
            assert_eq!(linear_to_srgb8(lin), i);
        }
    }

    #[test]
    fn f16_texel_round_trip_is_within_one_code() {
        // Upload quantises to f16; re-encoding must stay within 1 LSB.
        for i in 0..=255u8 {
            let bytes = rgba8_srgb_to_linear_premul_f16(&[i, i, i, 255]);
            let v = f16_bytes_to_f32(&bytes)[0];
            let back = linear_to_srgb8(f64::from(v));
            assert!(back.abs_diff(i) <= 1, "{i} -> {back}");
        }
    }
}
