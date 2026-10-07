// SPDX-License-Identifier: Apache-2.0
//! Structured light: Gray-code patterns a projector shows while a camera
//! photographs them, and the decoder that turns the photographs into
//! camera-pixel ↔ projector-pixel correspondences.
//!
//! Each bit is shown as a pattern and its inverse, and a camera pixel's bit
//! is whichever of the pair is brighter. This is robust to surface colour,
//! ambient light and camera exposure, without global thresholds. Pixels
//! where any pair differs by less than `min_contrast` are left undecoded
//! (shadows, outside the projection).

/// A single-channel 8-bit image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gray {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Gray {
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.pixels
            .get((y as usize) * (self.width as usize) + x as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Bits needed to code `n` positions.
#[must_use]
pub fn bits_for(n: u32) -> u32 {
    32 - n.saturating_sub(1).leading_zeros()
}

fn gray(v: u32) -> u32 {
    v ^ (v >> 1)
}

fn gray_to_binary(mut g: u32) -> u32 {
    let mut shift = 1;
    while shift < 32 {
        g ^= g >> shift;
        shift <<= 1;
    }
    g
}

/// What one pattern encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    /// Bit `bit` (most significant first) of the column code, or its inverse.
    Column {
        bit: u32,
        inverted: bool,
    },
    Row {
        bit: u32,
        inverted: bool,
    },
}

/// The pattern sequence for a `width × height` projector: for each column
/// bit then each row bit, the pattern followed by its inverse.
#[must_use]
pub fn sequence(width: u32, height: u32) -> Vec<PatternKind> {
    let mut out = Vec::new();
    for bit in 0..bits_for(width) {
        out.push(PatternKind::Column {
            bit,
            inverted: false,
        });
        out.push(PatternKind::Column {
            bit,
            inverted: true,
        });
    }
    for bit in 0..bits_for(height) {
        out.push(PatternKind::Row {
            bit,
            inverted: false,
        });
        out.push(PatternKind::Row {
            bit,
            inverted: true,
        });
    }
    out
}

/// Renders one pattern at projector resolution (0 or 255 per pixel).
#[must_use]
pub fn render(kind: PatternKind, width: u32, height: u32) -> Gray {
    let (cb, rb) = (bits_for(width), bits_for(height));
    let on = |code: u32, bits: u32, bit: u32, inverted: bool| {
        let b = (gray(code) >> (bits - 1 - bit)) & 1 == 1;
        b != inverted
    };
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let lit = match kind {
                PatternKind::Column { bit, inverted } => on(x, cb, bit, inverted),
                PatternKind::Row { bit, inverted } => on(y, rb, bit, inverted),
            };
            pixels.push(if lit { 255 } else { 0 });
        }
    }
    Gray {
        width,
        height,
        pixels,
    }
}

/// Decoded projector position for every camera pixel (`None` where it
/// could not be decoded), row-major at camera resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub map: Vec<Option<(u32, u32)>>,
}

impl Decoded {
    /// Correspondences `(camera pixel centre, projector pixel centre)` for
    /// every decoded camera pixel.
    #[must_use]
    pub fn pairs(&self) -> Vec<((f64, f64), (f64, f64))> {
        let mut out = Vec::new();
        for (i, m) in self.map.iter().enumerate() {
            if let Some((px, py)) = m {
                let w = self.width as usize;
                #[allow(clippy::cast_precision_loss)]
                out.push((
                    ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5),
                    (f64::from(*px) + 0.5, f64::from(*py) + 0.5),
                ));
            }
        }
        out
    }
}

/// Why decoding failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("expected {expected} captures, got {got}")]
    Count { expected: usize, got: usize },
    #[error("captures have different sizes")]
    Sizes,
}

/// Decodes captures taken in [`sequence`] order for a `proj_w × proj_h`
/// projector.
pub fn decode(
    captures: &[Gray],
    proj_w: u32,
    proj_h: u32,
    min_contrast: u8,
) -> Result<Decoded, DecodeError> {
    let seq = sequence(proj_w, proj_h);
    if captures.len() != seq.len() {
        return Err(DecodeError::Count {
            expected: seq.len(),
            got: captures.len(),
        });
    }
    let (w, h) = (captures[0].width, captures[0].height);
    if captures
        .iter()
        .any(|c| c.width != w || c.height != h || c.pixels.len() != (w * h) as usize)
    {
        return Err(DecodeError::Sizes);
    }
    let (cb, rb) = (bits_for(proj_w), bits_for(proj_h));
    let mut map = Vec::with_capacity((w * h) as usize);
    for i in 0..(w * h) as usize {
        let mut col = 0u32;
        let mut row = 0u32;
        let mut ok = true;
        for (k, pair) in captures.as_chunks::<2>().0.iter().enumerate() {
            let (a, b) = (pair[0].pixels[i], pair[1].pixels[i]);
            if a.abs_diff(b) < min_contrast {
                ok = false;
                break;
            }
            let bit = u32::from(a > b);
            let k = u32::try_from(k).unwrap_or(u32::MAX);
            if k < cb {
                col = (col << 1) | bit;
            } else {
                row = (row << 1) | bit;
            }
        }
        let (x, y) = (gray_to_binary(col), gray_to_binary(row));
        map.push((ok && x < proj_w && y < proj_h && rb + cb > 0).then_some((x, y)));
    }
    Ok(Decoded {
        width: w,
        height: h,
        map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_codes_round_trip_and_neighbours_differ_by_one_bit() {
        for v in 0..5000u32 {
            assert_eq!(gray_to_binary(gray(v)), v);
            assert_eq!((gray(v) ^ gray(v + 1)).count_ones(), 1);
        }
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(1920), 11);
        assert_eq!(bits_for(1024), 10);
        assert_eq!(sequence(1920, 1080).len(), 2 * (11 + 11));
    }

    #[test]
    fn decoding_the_patterns_themselves_recovers_every_pixel() {
        let (w, h) = (37u32, 21u32);
        let caps: Vec<Gray> = sequence(w, h)
            .into_iter()
            .map(|k| render(k, w, h))
            .collect();
        let d = decode(&caps, w, h, 10).unwrap();
        for y in 0..h {
            for x in 0..w {
                assert_eq!(d.map[(y * w + x) as usize], Some((x, y)));
            }
        }
        assert_eq!(d.pairs().len(), (w * h) as usize);
        assert!(matches!(
            decode(&caps[1..], w, h, 10),
            Err(DecodeError::Count { .. })
        ));
    }
}
