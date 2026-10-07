// SPDX-License-Identifier: Apache-2.0
//! Image comparison for golden tests and (later) oracle frame diffs.
//!
//! Tolerances follow docs/PLAN.md: interior and edge regions are measured
//! separately, and statistics are absolute channel errors in 8-bit codes.

/// Error statistics over the RGB channels of the selected pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DiffStats {
    pub pixels: usize,
    pub max: u8,
    /// Mean absolute error per channel, in 8-bit codes.
    pub mean: f64,
    /// 99.9th percentile absolute channel error.
    pub p999: u8,
}

/// Compares two RGBA8 images over pixels where `include(i)` is true.
#[must_use]
pub fn diff(a: &[u8], b: &[u8], include: impl Fn(usize) -> bool) -> DiffStats {
    let mut hist = [0usize; 256];
    let mut sum = 0u64;
    let mut pixels = 0usize;
    for (i, (pa, pb)) in a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .enumerate()
    {
        if !include(i) {
            continue;
        }
        pixels += 1;
        for k in 0..3 {
            let d = pa[k].abs_diff(pb[k]);
            hist[usize::from(d)] += 1;
            sum += u64::from(d);
        }
    }
    let samples = pixels * 3;
    if samples == 0 {
        return DiffStats::default();
    }
    let max = hist.iter().rposition(|&c| c > 0).unwrap_or(0);
    let target = (samples as f64 * 0.999).ceil() as usize;
    let mut acc = 0;
    let mut p999 = 0;
    for (v, c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            p999 = v;
            break;
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    DiffStats {
        pixels,
        max: max as u8,
        mean: sum as f64 / samples as f64,
        p999: p999 as u8,
    }
}

/// Marks pixels within `radius` pixels (Chebyshev) of a coverage boundary.
#[must_use]
pub fn edge_mask(coverage: &[bool], width: u32, height: u32, radius: u32) -> Vec<bool> {
    let (w, h) = (width as i64, height as i64);
    // Pixels beyond the image are ignored: the canvas border is not an edge.
    let at = |x: i64, y: i64| -> Option<bool> {
        if x < 0 || y < 0 || x >= w || y >= h {
            None
        } else {
            Some(coverage[(y * w + x) as usize])
        }
    };
    let r = i64::from(radius);
    let mut out = vec![false; coverage.len()];
    for y in 0..h {
        for x in 0..w {
            let c = at(x, y);
            'search: for dy in -r..=r {
                for dx in -r..=r {
                    if at(x + dx, y + dy).is_some_and(|n| Some(n) != c) {
                        out[(y * w + x) as usize] = true;
                        break 'search;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_have_zero_error() {
        let a = vec![10u8, 20, 30, 255, 40, 50, 60, 255];
        let s = diff(&a, &a, |_| true);
        assert_eq!((s.pixels, s.max, s.p999), (2, 0, 0));
        assert_eq!(s.mean, 0.0);
    }

    #[test]
    fn stats_report_max_and_mean() {
        let a = vec![0u8, 0, 0, 255, 0, 0, 0, 255];
        let b = vec![3u8, 0, 0, 255, 0, 0, 0, 255];
        let s = diff(&a, &b, |_| true);
        assert_eq!(s.max, 3);
        assert!((s.mean - 0.5).abs() < 1e-12);
        assert_eq!(diff(&a, &b, |i| i == 1).max, 0);
    }

    #[test]
    fn edge_mask_marks_boundary_band() {
        // 4x1: covered, covered, not, not
        let m = edge_mask(&[true, true, false, false], 4, 1, 1);
        assert_eq!(m, vec![false, true, true, false]);
    }
}
