// SPDX-License-Identifier: Apache-2.0
//! DMX output and LED pixel mapping (see docs/dmx.md).
//!
//! Nodes are network destinations (Art-Net or sACN). Fixtures are LED
//! strips, matrices or single lights: each samples the rendered canvas at
//! its pixel positions and fills consecutive DMX channels on a node.

use om_geom::Point2;
use om_types::{DmxNodeId, FixtureId, UnitInterval};
use serde::{Deserialize, Serialize};

/// Most fixtures in a project.
pub const MAX_FIXTURES: usize = 4096;
/// Most pixels in one fixture.
pub const MAX_FIXTURE_PIXELS: u32 = 65_536;
/// Most nodes in a project.
pub const MAX_NODES: usize = 64;
/// Most distinct (node, universe) streams a project may send: bounds
/// network traffic and per-frame sampling work.
pub const MAX_UNIVERSES: usize = 1024;
/// Allowed DMX refresh rates (packets per second per universe).
pub const DMX_RATES: std::ops::RangeInclusive<u32> = 1..=44;

fn default_rate() -> u32 {
    40
}

/// The project's DMX setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dmx {
    /// Packets per second per universe while sending.
    #[serde(default = "default_rate")]
    pub rate: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<DmxNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixtures: Vec<Fixture>,
}

impl Default for Dmx {
    fn default() -> Self {
        Self {
            rate: default_rate(),
            nodes: Vec::new(),
            fixtures: Vec::new(),
        }
    }
}

/// A network destination for DMX universes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmxNode {
    pub id: DmxNodeId,
    pub name: String,
    /// Whether packets are sent.
    #[serde(default)]
    pub enabled: bool,
    pub protocol: DmxProtocol,
}

fn default_priority() -> u8 {
    100
}

/// How a node is addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DmxProtocol {
    /// Art-Net 4 to `address` (an IPv4 address; a broadcast address such as
    /// `2.255.255.255` reaches every node on that network).
    ArtNet { address: String },
    /// sACN (E1.31). An empty `address` uses the standard multicast group
    /// of each universe; otherwise packets are sent unicast.
    Sacn {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        address: String,
        /// 0–200; receivers follow the highest-priority source.
        #[serde(default = "default_priority")]
        priority: u8,
    },
}

impl DmxProtocol {
    /// Valid universe numbers for this protocol (Art-Net: 15-bit
    /// Port-Address; sACN: 1–63999).
    #[must_use]
    pub const fn universes(&self) -> std::ops::RangeInclusive<u16> {
        match self {
            Self::ArtNet { .. } => 0..=0x7fff,
            Self::Sacn { .. } => 1..=63999,
        }
    }
}

/// Order and number of channels per pixel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourOrder {
    #[default]
    Rgb,
    Rbg,
    Grb,
    Gbr,
    Brg,
    Bgr,
    /// RGB plus a white channel (white = the common part of R, G and B).
    Rgbw,
    Grbw,
    /// One intensity channel (luminance), e.g. a dimmer or single-colour LED.
    Mono,
}

impl ColourOrder {
    /// Channels per pixel.
    #[must_use]
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgbw | Self::Grbw => 4,
            Self::Mono => 1,
            _ => 3,
        }
    }
}

/// How sampled colour becomes channel values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelEncoding {
    /// The values a monitor would show (sRGB). Suits most LED products,
    /// whose drivers apply their own correction.
    #[default]
    Srgb,
    /// Linear light (for drivers that expect linear values).
    Linear,
}

/// Where a pixel matrix's wiring starts and how it runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wiring {
    /// Row by row from the top-left corner, each row left to right.
    #[default]
    Rows,
    /// Row by row from the top-left, alternating direction ("snake").
    RowsSnake,
    /// Column by column from the top-left, each column top to bottom.
    Columns,
    /// Column by column from the top-left, alternating direction.
    ColumnsSnake,
}

/// Where a fixture's pixels sit on the canvas (canvas space, see om-geom).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PixelShape {
    /// One pixel sampled at `at` (a single light or a whole-colour wash).
    Point { at: Point2 },
    /// `count` pixels evenly spaced from `from` to `to` inclusive.
    Line {
        from: Point2,
        to: Point2,
        count: u32,
    },
    /// A `columns × rows` matrix filling the quad `corners` (top-left,
    /// top-right, bottom-right, bottom-left), wired as `wiring` says.
    Grid {
        corners: [Point2; 4],
        columns: u32,
        rows: u32,
        #[serde(default)]
        wiring: Wiring,
    },
}

impl PixelShape {
    /// Number of pixels.
    #[must_use]
    pub fn pixels(&self) -> u64 {
        match self {
            Self::Point { .. } => 1,
            Self::Line { count, .. } => u64::from(*count),
            Self::Grid { columns, rows, .. } => u64::from(*columns) * u64::from(*rows),
        }
    }
}

/// An LED strip, matrix or light driven from the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: FixtureId,
    pub name: String,
    #[serde(default = "crate::schema::default_true")]
    pub enabled: bool,
    pub node: DmxNodeId,
    /// Universe of the first pixel (Art-Net Port-Address or sACN universe).
    /// Pixels that do not fit continue at channel 1 of the next universe;
    /// a pixel is never split across universes.
    pub universe: u16,
    /// First channel of the first pixel, 1–512.
    pub address: u16,
    #[serde(default)]
    pub order: ColourOrder,
    #[serde(default)]
    pub encoding: ChannelEncoding,
    /// Scales the light output (linear light).
    #[serde(default)]
    pub brightness: UnitInterval,
    pub shape: PixelShape,
}

impl Fixture {
    /// Checks ranges that do not depend on the rest of the project.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("fixture name is empty".into());
        }
        if !(1..=512).contains(&self.address) {
            return Err(format!(
                "fixture \"{}\": address {} is not 1–512",
                self.name, self.address
            ));
        }
        if usize::from(self.address) - 1 + self.order.channels() > 512 {
            return Err(format!(
                "fixture \"{}\": a pixel starting at channel {} does not fit in the universe",
                self.name, self.address
            ));
        }
        let n = self.shape.pixels();
        if n == 0 || n > u64::from(MAX_FIXTURE_PIXELS) {
            return Err(format!(
                "fixture \"{}\" has {n} pixels (1–{MAX_FIXTURE_PIXELS})",
                self.name
            ));
        }
        Ok(())
    }

    /// Universes this fixture occupies (first, last).
    #[must_use]
    pub fn universe_span(&self) -> (u32, u32) {
        let per = 512 / self.order.channels();
        let first_room = (512 - (usize::from(self.address).max(1) - 1)) / self.order.channels();
        let n = usize::try_from(self.shape.pixels()).unwrap_or(usize::MAX);
        let extra = if n <= first_room {
            0
        } else {
            (n - first_room).div_ceil(per)
        };
        let first = u32::from(self.universe);
        (first, first + u32::try_from(extra).unwrap_or(u32::MAX))
    }
}

impl Dmx {
    /// True when there is nothing to save (default rate, no nodes or
    /// fixtures).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Checks the DMX setup (called by `Project::validate`).
    pub fn validate(&self) -> Result<(), String> {
        if !DMX_RATES.contains(&self.rate) {
            return Err(format!("DMX rate {} is not 1–44", self.rate));
        }
        if self.nodes.len() > MAX_NODES || self.fixtures.len() > MAX_FIXTURES {
            return Err("too many DMX nodes or fixtures".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for n in &self.nodes {
            if !ids.insert(n.id.to_string()) {
                return Err(format!("duplicate DMX node id {}", n.id));
            }
            if n.name.trim().is_empty() {
                return Err("DMX node name is empty".into());
            }
            match &n.protocol {
                DmxProtocol::ArtNet { address } => {
                    address.parse::<std::net::Ipv4Addr>().map_err(|_| {
                        format!(
                            "DMX node \"{}\": {address:?} is not an IPv4 address",
                            n.name
                        )
                    })?;
                }
                DmxProtocol::Sacn { address, priority } => {
                    if !address.is_empty() && address.parse::<std::net::Ipv4Addr>().is_err() {
                        return Err(format!(
                            "DMX node \"{}\": {address:?} is not an IPv4 address",
                            n.name
                        ));
                    }
                    if *priority > 200 {
                        return Err(format!("DMX node \"{}\": priority above 200", n.name));
                    }
                }
            }
        }
        let mut fixture_ids = std::collections::BTreeSet::new();
        let mut streams = std::collections::HashSet::new();
        for f in &self.fixtures {
            if !fixture_ids.insert(f.id.to_string()) {
                return Err(format!("duplicate fixture id {}", f.id));
            }
            f.validate()?;
            let node = self
                .nodes
                .iter()
                .find(|n| n.id == f.node)
                .ok_or_else(|| format!("fixture \"{}\" uses a missing DMX node", f.name))?;
            let range = node.protocol.universes();
            let (first, last) = f.universe_span();
            if first < u32::from(*range.start()) || last > u32::from(*range.end()) {
                return Err(format!(
                    "fixture \"{}\" needs universes {first}–{last}, outside {}–{}",
                    f.name,
                    range.start(),
                    range.end()
                ));
            }
            for u in first..=last {
                streams.insert((f.node, u));
                if streams.len() > MAX_UNIVERSES {
                    return Err(format!(
                        "fixtures use more than {MAX_UNIVERSES} DMX universes"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y).unwrap()
    }

    fn node(protocol: DmxProtocol) -> DmxNode {
        DmxNode {
            id: DmxNodeId::new(),
            name: "Node".into(),
            enabled: true,
            protocol,
        }
    }

    fn strip(node: DmxNodeId, universe: u16, address: u16, count: u32) -> Fixture {
        Fixture {
            id: FixtureId::new(),
            name: "Strip".into(),
            enabled: true,
            node,
            universe,
            address,
            order: ColourOrder::Rgb,
            encoding: ChannelEncoding::Srgb,
            brightness: UnitInterval::ONE,
            shape: PixelShape::Line {
                from: p(0.0, 0.5),
                to: p(1.0, 0.5),
                count,
            },
        }
    }

    #[test]
    fn total_universes_are_capped() {
        let n = node(DmxProtocol::Sacn {
            address: String::new(),
            priority: 100,
        });
        let id = n.id;
        // 170 RGB pixels per universe: 65 536 pixels span 386 universes.
        let mut dmx = Dmx {
            nodes: vec![n],
            ..Dmx::default()
        };
        let fixture = |u: u16| {
            let mut f = strip(id, u, 1, MAX_FIXTURE_PIXELS);
            f.id = FixtureId::new();
            f
        };
        dmx.fixtures = vec![fixture(1), fixture(1000)];
        dmx.validate().unwrap();
        dmx.fixtures.push(fixture(2000));
        assert!(dmx.validate().unwrap_err().contains("universes"));
        // Overlapping fixtures share universes.
        dmx.fixtures.pop();
        dmx.fixtures.push(fixture(1));
        dmx.validate().unwrap();
    }

    #[test]
    fn universe_spans_never_split_pixels() {
        let n = DmxNodeId::new();
        assert_eq!(strip(n, 1, 1, 170).universe_span(), (1, 1));
        assert_eq!(strip(n, 1, 1, 171).universe_span(), (1, 2));
        assert_eq!(strip(n, 1, 4, 169).universe_span(), (1, 1));
        assert_eq!(strip(n, 1, 4, 170).universe_span(), (1, 2));
        assert_eq!(strip(n, 5, 1, 1000).universe_span(), (5, 10)); // 170 per universe
        let mut rgbw = strip(n, 1, 1, 129);
        rgbw.order = ColourOrder::Rgbw;
        assert_eq!(rgbw.universe_span(), (1, 2)); // 128 per universe
    }

    #[test]
    fn validation_checks_ranges_and_references() {
        let art = node(DmxProtocol::ArtNet {
            address: "2.255.255.255".into(),
        });
        let sacn = node(DmxProtocol::Sacn {
            address: String::new(),
            priority: 100,
        });
        let mut dmx = Dmx {
            nodes: vec![art.clone(), sacn.clone()],
            fixtures: vec![strip(art.id, 0, 1, 170), strip(sacn.id, 1, 1, 170)],
            ..Dmx::default()
        };
        assert!(dmx.validate().is_ok());

        dmx.fixtures[1].universe = 0;
        assert!(dmx.validate().unwrap_err().contains("outside 1–63999"));
        dmx.fixtures[1].universe = 1;

        dmx.fixtures[0].universe = 0x7fff;
        dmx.fixtures[0].shape = PixelShape::Point { at: p(0.5, 0.5) };
        assert!(dmx.validate().is_ok());
        dmx.fixtures[0] = strip(art.id, 0x7fff, 1, 171);
        assert!(dmx.validate().is_err(), "spills past the last universe");

        dmx.fixtures[0] = strip(art.id, 0, 511, 1);
        assert!(dmx.validate().unwrap_err().contains("does not fit"));
        dmx.fixtures[0] = strip(art.id, 0, 0, 1);
        assert!(dmx.validate().is_err());
        dmx.fixtures[0] = strip(DmxNodeId::new(), 0, 1, 1);
        assert!(dmx.validate().unwrap_err().contains("missing DMX node"));
        dmx.fixtures[0] = strip(art.id, 0, 1, 0);
        assert!(dmx.validate().is_err(), "no pixels");

        dmx.fixtures.truncate(1);
        dmx.fixtures[0] = strip(art.id, 0, 1, 1);
        dmx.nodes[0].protocol = DmxProtocol::ArtNet {
            address: "nonsense".into(),
        };
        assert!(dmx.validate().is_err());
        dmx.nodes[0].protocol = DmxProtocol::ArtNet {
            address: "10.0.0.1".into(),
        };
        dmx.rate = 45;
        assert!(dmx.validate().is_err());
    }

    #[test]
    fn serialises_compactly_with_defaults() {
        let dmx = Dmx::default();
        assert!(dmx.is_empty());
        assert_eq!(serde_json::to_string(&dmx).unwrap(), r#"{"rate":40}"#);
        let back: Dmx = serde_json::from_str("{}").unwrap();
        assert_eq!(back, dmx);
        let n = node(DmxProtocol::Sacn {
            address: String::new(),
            priority: 100,
        });
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(
            json["protocol"],
            serde_json::json!({"type": "sacn", "priority": 100})
        );
    }
}
