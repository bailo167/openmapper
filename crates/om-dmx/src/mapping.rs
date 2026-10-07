// SPDX-License-Identifier: Apache-2.0
//! LED pixel mapping: where each fixture pixel sits on the canvas, which
//! DMX channels it fills, and how its colour is sampled from a frame.

use std::collections::BTreeMap;

use om_project::dmx::{ChannelEncoding, ColourOrder, Fixture, PixelShape, Wiring};
use om_types::DmxNodeId;

/// One rendered canvas frame: sRGB RGBA8, straight alpha, rows top-down.
#[derive(Debug, Clone, Copy)]
pub struct FrameView<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba8: &'a [u8],
}

/// DMX data for every universe that fixtures touch, keyed by node and
/// universe. Untouched channels are 0.
pub type Universes = BTreeMap<(DmxNodeId, u16), [u8; 512]>;

/// Canvas positions of a shape's pixels in wiring order, and the size of
/// each pixel's footprint (canvas units, half-width and half-height).
#[must_use]
pub fn pixel_positions(shape: &PixelShape) -> (Vec<(f64, f64)>, (f64, f64)) {
    match shape {
        PixelShape::Point { at } => (vec![at.to_tuple()], (0.0, 0.0)),
        PixelShape::Line { from, to, count } => {
            let (a, b) = (from.to_tuple(), to.to_tuple());
            let n = (*count).max(1);
            let step = if n > 1 { 1.0 / f64::from(n - 1) } else { 0.0 };
            let pts = (0..n)
                .map(|i| {
                    let t = f64::from(i) * step;
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
                })
                .collect();
            // Each pixel covers the stretch up to its neighbours, as a square.
            let spacing = ((b.0 - a.0).hypot(b.1 - a.1) * step) / 2.0;
            (pts, (spacing, spacing))
        }
        PixelShape::Grid {
            corners,
            columns,
            rows,
            wiring,
        } => {
            let [tl, tr, br, bl] = corners.map(|c| c.to_tuple());
            let (cols, rows) = ((*columns).max(1), (*rows).max(1));
            let at = |c: u32, r: u32| {
                let u = (f64::from(c) + 0.5) / f64::from(cols);
                let v = (f64::from(r) + 0.5) / f64::from(rows);
                let top = (tl.0 + (tr.0 - tl.0) * u, tl.1 + (tr.1 - tl.1) * u);
                let bottom = (bl.0 + (br.0 - bl.0) * u, bl.1 + (br.1 - bl.1) * u);
                (
                    top.0 + (bottom.0 - top.0) * v,
                    top.1 + (bottom.1 - top.1) * v,
                )
            };
            let mut pts = Vec::with_capacity((cols * rows) as usize);
            match wiring {
                Wiring::Rows | Wiring::RowsSnake => {
                    for r in 0..rows {
                        let reverse = *wiring == Wiring::RowsSnake && r % 2 == 1;
                        for k in 0..cols {
                            let c = if reverse { cols - 1 - k } else { k };
                            pts.push(at(c, r));
                        }
                    }
                }
                Wiring::Columns | Wiring::ColumnsSnake => {
                    for c in 0..cols {
                        let reverse = *wiring == Wiring::ColumnsSnake && c % 2 == 1;
                        for k in 0..rows {
                            let r = if reverse { rows - 1 - k } else { k };
                            pts.push(at(c, r));
                        }
                    }
                }
            }
            // Footprint: the cell's bounding box (exact for rectangles).
            let w = (tr.0 - tl.0).abs().max((br.0 - bl.0).abs()) / f64::from(cols);
            let h = (bl.1 - tl.1).abs().max((br.1 - tr.1).abs()) / f64::from(rows);
            (pts, (w / 2.0, h / 2.0))
        }
    }
}

/// Where each pixel's channels start: `(universe, 0-based channel)`.
/// Pixels never straddle universes: one that does not fit starts at
/// channel 0 of the next universe.
#[must_use]
pub fn pixel_slots(fixture: &Fixture) -> Vec<(u16, usize)> {
    let width = fixture.order.channels();
    let n = usize::try_from(fixture.shape.pixels()).unwrap_or(0);
    let mut universe = fixture.universe;
    let mut channel = usize::from(fixture.address.max(1)) - 1;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if channel + width > 512 {
            let Some(next) = universe.checked_add(1) else {
                break;
            };
            universe = next;
            channel = 0;
        }
        out.push((universe, channel));
        channel += width;
    }
    out
}

/// sRGB 8-bit → linear, exact table.
fn srgb_to_linear_table() -> [f32; 256] {
    let mut t = [0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let c = i as f64 / 255.0;
        let l = if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
        #[allow(clippy::cast_possible_truncation)]
        {
            *v = l as f32;
        }
    }
    t
}

fn linear_to_srgb8(l: f32) -> u8 {
    let l = f64::from(l.clamp(0.0, 1.0));
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

fn linear_to_u8(l: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (l.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

/// Samples frames in linear light.
#[derive(Debug, Clone)]
pub struct Sampler {
    lut: [f32; 256],
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            lut: srgb_to_linear_table(),
        }
    }
}

/// Most image pixels averaged per side of a footprint (larger footprints
/// are sampled on a regular sub-grid).
const MAX_TAPS: usize = 32;

impl Sampler {
    /// Linear, alpha-weighted (over black) colour of one image pixel.
    fn texel(&self, f: &FrameView<'_>, x: usize, y: usize) -> [f32; 3] {
        let i = (y * f.width as usize + x) * 4;
        let Some(p) = f.rgba8.get(i..i + 4) else {
            return [0.0; 3];
        };
        let a = f32::from(p[3]) / 255.0;
        [
            self.lut[usize::from(p[0])] * a,
            self.lut[usize::from(p[1])] * a,
            self.lut[usize::from(p[2])] * a,
        ]
    }

    /// Bilinear sample at canvas position `(x, y)` (pixel centres at
    /// `(i + 0.5) / width`), clamped at the edges.
    fn bilinear(&self, f: &FrameView<'_>, x: f64, y: f64) -> [f32; 3] {
        let (w, h) = (f64::from(f.width), f64::from(f.height));
        let fx = (x * w - 0.5).clamp(0.0, w - 1.0);
        let fy = (y * h - 0.5).clamp(0.0, h - 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let x1 = (x0 + 1).min(f.width as usize - 1);
        let y1 = (y0 + 1).min(f.height as usize - 1);
        #[allow(clippy::cast_possible_truncation)]
        let (tx, ty) = ((fx - fx.floor()) as f32, (fy - fy.floor()) as f32);
        let (a, b, c, d) = (
            self.texel(f, x0, y0),
            self.texel(f, x1, y0),
            self.texel(f, x0, y1),
            self.texel(f, x1, y1),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }

    /// Average linear colour over the canvas box centred at `(x, y)` with
    /// half-size `(hw, hh)`: the mean of the image pixels whose centres
    /// lie inside it, or a bilinear sample if the box holds none.
    #[must_use]
    pub fn sample(&self, f: &FrameView<'_>, (x, y): (f64, f64), (hw, hh): (f64, f64)) -> [f32; 3] {
        if f.width == 0 || f.height == 0 || f.rgba8.len() < (f.width * f.height * 4) as usize {
            return [0.0; 3];
        }
        let (w, h) = (f64::from(f.width), f64::from(f.height));
        // Image pixel index range whose centres fall inside the box.
        let range = |lo: f64, hi: f64, n: f64| {
            let a = (lo * n - 0.5).ceil().max(0.0);
            let b = (hi * n - 0.5).floor().min(n - 1.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            (a <= b).then_some((a as usize, b as usize))
        };
        let (Some((x0, x1)), Some((y0, y1))) = (range(x - hw, x + hw, w), range(y - hh, y + hh, h))
        else {
            return self.bilinear(f, x, y);
        };
        let stride = |a: usize, b: usize| ((b - a + 1).div_ceil(MAX_TAPS)).max(1);
        let (sx, sy) = (stride(x0, x1), stride(y0, y1));
        let mut sum = [0f32; 3];
        let mut n = 0u32;
        for py in (y0..=y1).step_by(sy) {
            for px in (x0..=x1).step_by(sx) {
                let t = self.texel(f, px, py);
                for k in 0..3 {
                    sum[k] += t[k];
                }
                n += 1;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let n = n.max(1) as f32;
        sum.map(|s| s / n)
    }
}

/// Channel values for one pixel of linear colour `rgb`.
#[must_use]
pub fn encode_pixel(
    rgb: [f32; 3],
    brightness: f32,
    order: ColourOrder,
    encoding: ChannelEncoding,
) -> ([u8; 4], usize) {
    let [r, g, b] = rgb.map(|c| c * brightness);
    let enc = |v: f32| match encoding {
        ChannelEncoding::Srgb => linear_to_srgb8(v),
        ChannelEncoding::Linear => linear_to_u8(v),
    };
    let w = r.min(g).min(b).max(0.0);
    let out = match order {
        ColourOrder::Rgb => [enc(r), enc(g), enc(b), 0],
        ColourOrder::Rbg => [enc(r), enc(b), enc(g), 0],
        ColourOrder::Grb => [enc(g), enc(r), enc(b), 0],
        ColourOrder::Gbr => [enc(g), enc(b), enc(r), 0],
        ColourOrder::Brg => [enc(b), enc(r), enc(g), 0],
        ColourOrder::Bgr => [enc(b), enc(g), enc(r), 0],
        ColourOrder::Rgbw => [enc(r - w), enc(g - w), enc(b - w), enc(w)],
        ColourOrder::Grbw => [enc(g - w), enc(r - w), enc(b - w), enc(w)],
        ColourOrder::Mono => [enc(0.2126 * r + 0.7152 * g + 0.0722 * b), 0, 0, 0],
    };
    (out, order.channels())
}

/// A fixture's pixel positions and channel slots, computed once.
#[derive(Debug, Clone)]
pub struct FixturePlan {
    pub node: DmxNodeId,
    pub positions: Vec<(f64, f64)>,
    pub footprint: (f64, f64),
    pub slots: Vec<(u16, usize)>,
    pub order: ColourOrder,
    pub encoding: ChannelEncoding,
    pub brightness: f32,
}

impl FixturePlan {
    #[must_use]
    pub fn new(f: &Fixture) -> Self {
        let (positions, footprint) = pixel_positions(&f.shape);
        #[allow(clippy::cast_possible_truncation)]
        Self {
            node: f.node,
            positions,
            footprint,
            slots: pixel_slots(f),
            order: f.order,
            encoding: f.encoding,
            brightness: f.brightness.get() as f32,
        }
    }

    /// Samples `frame` and writes this fixture's channels into `out`.
    pub fn render(&self, sampler: &Sampler, frame: &FrameView<'_>, out: &mut Universes) {
        for (pos, (universe, channel)) in self.positions.iter().zip(&self.slots) {
            let rgb = sampler.sample(frame, *pos, self.footprint);
            let (values, n) = encode_pixel(rgb, self.brightness, self.order, self.encoding);
            let data = out.entry((self.node, *universe)).or_insert([0; 512]);
            data[*channel..*channel + n].copy_from_slice(&values[..n]);
        }
    }
}

/// Renders every enabled fixture of `fixtures` from `frame`.
#[must_use]
pub fn render_fixtures(
    plans: &[FixturePlan],
    sampler: &Sampler,
    frame: &FrameView<'_>,
) -> Universes {
    let mut out = Universes::new();
    for p in plans {
        p.render(sampler, frame, &mut out);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use om_geom::Point2;
    use om_project::dmx::Fixture;
    use om_types::{FixtureId, UnitInterval};

    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y).unwrap()
    }

    fn fixture(shape: PixelShape, order: ColourOrder, universe: u16, address: u16) -> Fixture {
        Fixture {
            id: FixtureId::new(),
            name: "F".into(),
            enabled: true,
            node: DmxNodeId::from_u128(1),
            universe,
            address,
            order,
            encoding: ChannelEncoding::Srgb,
            brightness: UnitInterval::ONE,
            shape,
        }
    }

    #[test]
    fn grid_wiring_orders() {
        let corners = [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let grid = |wiring| PixelShape::Grid {
            corners,
            columns: 3,
            rows: 2,
            wiring,
        };
        let idx = |pts: Vec<(f64, f64)>| -> Vec<(u32, u32)> {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            pts.iter()
                .map(|(x, y)| ((x * 3.0) as u32, (y * 2.0) as u32))
                .collect()
        };
        assert_eq!(
            idx(pixel_positions(&grid(Wiring::Rows)).0),
            [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]
        );
        assert_eq!(
            idx(pixel_positions(&grid(Wiring::RowsSnake)).0),
            [(0, 0), (1, 0), (2, 0), (2, 1), (1, 1), (0, 1)]
        );
        assert_eq!(
            idx(pixel_positions(&grid(Wiring::Columns)).0),
            [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]
        );
        assert_eq!(
            idx(pixel_positions(&grid(Wiring::ColumnsSnake)).0),
            [(0, 0), (0, 1), (1, 1), (1, 0), (2, 0), (2, 1)]
        );
        let (pts, half) = pixel_positions(&grid(Wiring::Rows));
        assert_eq!(pts[0], (1.0 / 6.0, 0.25), "cell centres");
        assert_eq!(half, (1.0 / 6.0, 0.25));
    }

    #[test]
    fn line_positions_include_both_ends() {
        let (pts, half) = pixel_positions(&PixelShape::Line {
            from: p(0.0, 0.5),
            to: p(1.0, 0.5),
            count: 5,
        });
        assert_eq!(pts.first(), Some(&(0.0, 0.5)));
        assert_eq!(pts.last(), Some(&(1.0, 0.5)));
        assert_eq!(pts[2], (0.5, 0.5));
        assert_eq!(half, (0.125, 0.125));
    }

    #[test]
    fn slots_wrap_whole_pixels_into_the_next_universe() {
        let line = |count| PixelShape::Line {
            from: p(0.0, 0.0),
            to: p(1.0, 0.0),
            count,
        };
        let s = pixel_slots(&fixture(line(172), ColourOrder::Rgb, 3, 1));
        assert_eq!(s[0], (3, 0));
        assert_eq!(s[169], (3, 507));
        assert_eq!(s[170], (4, 0), "171st pixel starts the next universe");
        assert_eq!(s[171], (4, 3));
        let s = pixel_slots(&fixture(line(2), ColourOrder::Rgbw, 1, 509));
        assert_eq!(s, [(1, 508), (2, 0)]);
        // The project validator refuses fixtures that run past the last
        // universe; the planner still never overflows.
        let s = pixel_slots(&fixture(line(400), ColourOrder::Rgb, u16::MAX, 1));
        assert_eq!(s.len(), 170);
    }

    #[test]
    fn encodings_and_orders() {
        let (v, n) = encode_pixel(
            [1.0, 0.5, 0.0],
            1.0,
            ColourOrder::Grb,
            ChannelEncoding::Linear,
        );
        assert_eq!((&v[..n], n), (&[128u8, 255, 0][..], 3));
        let (v, _) = encode_pixel(
            [0.215_861, 0.0, 1.0],
            1.0,
            ColourOrder::Rgb,
            ChannelEncoding::Srgb,
        );
        assert_eq!(
            v[..3],
            [128, 0, 255],
            "sRGB encoding of linear 0.2159 is 128"
        );
        let (v, n) = encode_pixel(
            [1.0, 1.0, 0.5],
            1.0,
            ColourOrder::Rgbw,
            ChannelEncoding::Linear,
        );
        assert_eq!(&v[..n], &[128, 128, 0, 128], "white takes the common part");
        let (v, n) = encode_pixel(
            [1.0, 1.0, 1.0],
            0.5,
            ColourOrder::Mono,
            ChannelEncoding::Linear,
        );
        assert_eq!(&v[..n], &[128]);
        let (v, _) = encode_pixel(
            [2.0, -1.0, f32::NAN],
            1.0,
            ColourOrder::Rgb,
            ChannelEncoding::Srgb,
        );
        assert_eq!(v[..2], [255, 0], "out-of-range values clamp");
    }

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let f = &f;
        (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| f(x, y)))
            .collect()
    }

    #[test]
    fn sampling_averages_footprints_in_linear_light() {
        // Left half black, right half white.
        let data = frame(8, 2, |x, _| {
            if x < 4 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let f = FrameView {
            width: 8,
            height: 2,
            rgba8: &data,
        };
        let s = Sampler::default();
        let whole = s.sample(&f, (0.5, 0.5), (0.5, 0.5));
        assert!((whole[0] - 0.5).abs() < 1e-6, "{whole:?}");
        let (v, _) = encode_pixel(whole, 1.0, ColourOrder::Rgb, ChannelEncoding::Srgb);
        assert_eq!(v[0], 188, "half white in linear light is sRGB 188, not 128");
        assert_eq!(s.sample(&f, (0.75, 0.5), (0.25, 0.5))[0], 1.0);
        assert_eq!(
            s.sample(&f, (0.25, 0.5), (0.0, 0.0))[0],
            0.0,
            "point sample"
        );
        // Transparent pixels count as black.
        let clear = frame(2, 2, |_, _| [255, 255, 255, 0]);
        let f = FrameView {
            width: 2,
            height: 2,
            rgba8: &clear,
        };
        assert_eq!(s.sample(&f, (0.5, 0.5), (0.5, 0.5)), [0.0; 3]);
        // A malformed frame samples as black instead of panicking.
        let f = FrameView {
            width: 4,
            height: 4,
            rgba8: &[1, 2, 3],
        };
        assert_eq!(s.sample(&f, (0.5, 0.5), (0.1, 0.1)), [0.0; 3]);
    }

    /// Pixel-mapping golden: an 4×2 RGBW matrix over a known 8×4 image
    /// produces exactly these channel values.
    #[test]
    fn pixel_mapping_golden() {
        // Each 2×2 block of the image has one flat colour.
        let colours = [
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
            ],
            [
                [128, 128, 128, 255],
                [255, 255, 0, 255],
                [0, 0, 0, 255],
                [255, 255, 255, 128],
            ],
        ];
        let data = frame(8, 4, |x, y| colours[(y / 2) as usize][(x / 2) as usize]);
        let f = FrameView {
            width: 8,
            height: 4,
            rgba8: &data,
        };
        let shape = PixelShape::Grid {
            corners: [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
            columns: 4,
            rows: 2,
            wiring: Wiring::RowsSnake,
        };
        let plan = FixturePlan::new(&fixture(shape, ColourOrder::Rgbw, 7, 10));
        let out = render_fixtures(&[plan], &Sampler::default(), &f);
        let u = out.get(&(DmxNodeId::from_u128(1), 7)).unwrap();
        #[rustfmt::skip]
        let expected: [u8; 32] = [
            255, 0, 0, 0,       // red
            0, 255, 0, 0,       // green
            0, 0, 255, 0,       // blue
            0, 0, 0, 255,       // white → W only
            // second row, wired right to left
            0, 0, 0, 188,       // white at half alpha: half linear light
            0, 0, 0, 0,         // black
            255, 255, 0, 0,     // yellow (no common part with blue 0)
            0, 0, 0, 128,       // grey 128 → W only, same sRGB value
        ];
        assert_eq!(&u[9..41], &expected);
        assert!(u[..9].iter().all(|&b| b == 0));
        assert!(u[41..].iter().all(|&b| b == 0));
    }
}
