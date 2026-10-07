// SPDX-License-Identifier: Apache-2.0
//! DMX / LED UI: network nodes, fixtures (strips, matrices, single
//! lights) and an overlay of fixture pixels on the canvas.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Pos2, Stroke};
use om_command::Command;
use om_geom::Point2;
use om_project::dmx::{
    ChannelEncoding, ColourOrder, DmxNode, DmxProtocol, Fixture, PixelShape, Wiring,
};
use om_types::{DmxNodeId, FixtureId, UnitInterval};

use crate::OpenMapperApp;
use crate::canvas::to_screen;

/// Text being edited (node addresses commit when focus leaves).
#[derive(Debug, Default)]
pub struct DmxUi {
    addresses: HashMap<DmxNodeId, String>,
}

const ORDERS: [(ColourOrder, &str); 9] = [
    (ColourOrder::Rgb, "RGB"),
    (ColourOrder::Rbg, "RBG"),
    (ColourOrder::Grb, "GRB"),
    (ColourOrder::Gbr, "GBR"),
    (ColourOrder::Brg, "BRG"),
    (ColourOrder::Bgr, "BGR"),
    (ColourOrder::Rgbw, "RGBW"),
    (ColourOrder::Grbw, "GRBW"),
    (ColourOrder::Mono, "Mono"),
];

const WIRINGS: [(Wiring, &str); 4] = [
    (Wiring::Rows, "Rows"),
    (Wiring::RowsSnake, "Rows (snake)"),
    (Wiring::Columns, "Columns"),
    (Wiring::ColumnsSnake, "Columns (snake)"),
];

fn pt(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap_or_default()
}

fn protocol_label(p: &DmxProtocol) -> &'static str {
    match p {
        DmxProtocol::ArtNet { .. } => "Art-Net",
        DmxProtocol::Sacn { .. } => "sACN",
    }
}

/// Edits a canvas coordinate pair in place; true if changed.
fn point_edit(ui: &mut egui::Ui, label: &str, p: &mut Point2) -> bool {
    let (mut x, mut y) = p.to_tuple();
    let mut changed = false;
    ui.label(label);
    changed |= ui
        .add(
            egui::DragValue::new(&mut x)
                .speed(0.002)
                .range(0.0..=1.0)
                .max_decimals(3),
        )
        .changed();
    changed |= ui
        .add(
            egui::DragValue::new(&mut y)
                .speed(0.002)
                .range(0.0..=1.0)
                .max_decimals(3),
        )
        .changed();
    if changed {
        *p = pt(x, y);
    }
    changed
}

impl OpenMapperApp {
    /// The DMX / LED section of the left panel.
    pub(crate) fn dmx_panel(&mut self, ui: &mut egui::Ui) {
        let dmx = self.session.project().dmx.clone();
        ui.horizontal(|ui| {
            ui.heading("DMX / LED");
            ui.menu_button("+ Node", |ui| {
                let n = dmx.nodes.len() + 1;
                let mut add = |protocol| {
                    self.queue(Command::PutDmxNode {
                        node: DmxNode {
                            id: DmxNodeId::new(),
                            name: format!("Node {n}"),
                            enabled: false,
                            protocol,
                        },
                        index: None,
                    });
                };
                if ui.button("Art-Net").clicked() {
                    add(DmxProtocol::ArtNet {
                        address: "2.255.255.255".into(),
                    });
                    ui.close();
                }
                if ui.button("sACN (multicast)").clicked() {
                    add(DmxProtocol::Sacn {
                        address: String::new(),
                        priority: 100,
                    });
                    ui.close();
                }
            });
            let first_node = dmx.nodes.first().map(|n| (n.id, n.protocol.universes()));
            ui.add_enabled_ui(first_node.is_some(), |ui| {
                ui.menu_button("+ Fixture", |ui| {
                    let Some((node, universes)) = first_node else {
                        return;
                    };
                    let universe = *universes.start();
                    let base = |name: &str, order, shape| Fixture {
                        id: FixtureId::new(),
                        name: name.into(),
                        enabled: true,
                        node,
                        universe,
                        address: 1,
                        order,
                        encoding: ChannelEncoding::Srgb,
                        brightness: UnitInterval::ONE,
                        shape,
                    };
                    let chosen = if ui.button("LED strip (60 px)").clicked() {
                        Some(base(
                            "Strip",
                            ColourOrder::Grb,
                            PixelShape::Line {
                                from: pt(0.1, 0.5),
                                to: pt(0.9, 0.5),
                                count: 60,
                            },
                        ))
                    } else if ui.button("LED matrix (16×16)").clicked() {
                        Some(base(
                            "Matrix",
                            ColourOrder::Grb,
                            PixelShape::Grid {
                                corners: [pt(0.3, 0.2), pt(0.7, 0.2), pt(0.7, 0.8), pt(0.3, 0.8)],
                                columns: 16,
                                rows: 16,
                                wiring: Wiring::RowsSnake,
                            },
                        ))
                    } else if ui.button("Single light (RGB)").clicked() {
                        Some(base(
                            "Light",
                            ColourOrder::Rgb,
                            PixelShape::Point { at: pt(0.5, 0.5) },
                        ))
                    } else {
                        None
                    };
                    if let Some(fixture) = chosen {
                        self.queue(Command::PutFixture {
                            fixture,
                            index: None,
                        });
                        ui.close();
                    }
                });
            });
        });
        let mut rate = dmx.rate;
        ui.horizontal(|ui| {
            ui.label("Refresh");
            if ui
                .add(egui::DragValue::new(&mut rate).range(1..=44).suffix(" Hz"))
                .changed()
            {
                self.queue_coalescing(Command::SetDmxRate { rate }, "dmx-rate".into());
            }
            if let Some(s) = self.viewer.as_ref().and_then(|v| v.dmx.stats()) {
                let text = format!("{} universes · {} packets", s.universes, s.packets);
                let r = ui.weak(text);
                if let Some(e) = &s.last_error {
                    r.on_hover_text(format!("{} errors; last: {e}", s.errors));
                }
            }
        });
        for node in &dmx.nodes {
            self.node_row(ui, node);
        }
        for f in &dmx.fixtures {
            self.fixture_row(ui, f, &dmx.nodes);
        }
    }

    fn node_row(&mut self, ui: &mut egui::Ui, node: &DmxNode) {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                let mut n = node.clone();
                let mut changed = ui.checkbox(&mut n.enabled, "").changed();
                let mut name = n.name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                    .lost_focus()
                    && !name.trim().is_empty()
                    && name != n.name
                {
                    n.name = name;
                    changed = true;
                }
                ui.weak(protocol_label(&n.protocol));
                if ui.small_button("×").on_hover_text("Remove node").clicked() {
                    self.queue(Command::RemoveDmxNode { id: n.id });
                }
                if changed {
                    self.queue(Command::PutDmxNode {
                        node: n,
                        index: None,
                    });
                }
            });
            ui.horizontal(|ui| {
                let current = match &node.protocol {
                    DmxProtocol::ArtNet { address } | DmxProtocol::Sacn { address, .. } => {
                        address.clone()
                    }
                };
                let text = self
                    .dmx_ui
                    .addresses
                    .entry(node.id)
                    .or_insert_with(|| current.clone());
                let hint = match node.protocol {
                    DmxProtocol::ArtNet { .. } => "IPv4 or broadcast address",
                    DmxProtocol::Sacn { .. } => "empty = multicast",
                };
                let r = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .desired_width(120.0),
                );
                if r.lost_focus() {
                    let value = text.trim().to_owned();
                    self.dmx_ui.addresses.remove(&node.id);
                    if value != current {
                        let mut n = node.clone();
                        match &mut n.protocol {
                            DmxProtocol::ArtNet { address } | DmxProtocol::Sacn { address, .. } => {
                                *address = value;
                            }
                        }
                        self.queue(Command::PutDmxNode {
                            node: n,
                            index: None,
                        });
                    }
                } else if !r.has_focus() && *text != current {
                    *text = current;
                }
                if let DmxProtocol::Sacn { priority, .. } = &node.protocol {
                    let mut p = *priority;
                    ui.label("priority");
                    if ui
                        .add(egui::DragValue::new(&mut p).range(0..=200))
                        .changed()
                    {
                        let mut n = node.clone();
                        if let DmxProtocol::Sacn { priority, .. } = &mut n.protocol {
                            *priority = p;
                        }
                        self.queue_coalescing(
                            Command::PutDmxNode {
                                node: n,
                                index: None,
                            },
                            format!("dmx-node-{}", node.id),
                        );
                    }
                }
            });
        });
    }

    fn fixture_row(&mut self, ui: &mut egui::Ui, f: &Fixture, nodes: &[DmxNode]) {
        let key = format!("fixture-{}", f.id);
        let mut g = f.clone();
        let mut changed = false;
        let mut coalesce = false;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut g.enabled, "").changed();
                let mut name = g.name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                    .lost_focus()
                    && !name.trim().is_empty()
                    && name != g.name
                {
                    g.name = name;
                    changed = true;
                }
                ui.weak(format!("{} px", g.shape.pixels()));
                if ui
                    .small_button("×")
                    .on_hover_text("Remove fixture")
                    .clicked()
                {
                    self.queue(Command::RemoveFixture { id: g.id });
                }
            });
            ui.horizontal(|ui| {
                let node_name = nodes
                    .iter()
                    .find(|n| n.id == g.node)
                    .map_or("?", |n| n.name.as_str());
                egui::ComboBox::from_id_salt(("node", g.id))
                    .selected_text(node_name)
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        for n in nodes {
                            changed |= ui.selectable_value(&mut g.node, n.id, &n.name).changed();
                        }
                    });
                ui.label("univ");
                coalesce |= ui
                    .add(egui::DragValue::new(&mut g.universe).range(0..=63999))
                    .changed();
                ui.label("ch");
                coalesce |= ui
                    .add(egui::DragValue::new(&mut g.address).range(1..=512))
                    .changed();
                let order = ORDERS
                    .iter()
                    .find(|(o, _)| *o == g.order)
                    .map_or("?", |o| o.1);
                egui::ComboBox::from_id_salt(("order", g.id))
                    .selected_text(order)
                    .width(56.0)
                    .show_ui(ui, |ui| {
                        for (o, label) in ORDERS {
                            changed |= ui.selectable_value(&mut g.order, o, label).changed();
                        }
                    });
            });
            ui.horizontal(|ui| match &mut g.shape {
                PixelShape::Point { at } => {
                    coalesce |= point_edit(ui, "at", at);
                }
                PixelShape::Line { from, to, count } => {
                    coalesce |= point_edit(ui, "from", from);
                    coalesce |= point_edit(ui, "to", to);
                    ui.label("px");
                    coalesce |= ui
                        .add(egui::DragValue::new(count).range(1..=4096))
                        .changed();
                }
                PixelShape::Grid {
                    corners,
                    columns,
                    rows,
                    wiring,
                } => {
                    coalesce |= ui
                        .add(egui::DragValue::new(columns).range(1..=512))
                        .changed();
                    ui.label("×");
                    coalesce |= ui.add(egui::DragValue::new(rows).range(1..=512)).changed();
                    let w = WIRINGS
                        .iter()
                        .find(|(x, _)| x == wiring)
                        .map_or("?", |x| x.1);
                    egui::ComboBox::from_id_salt(("wiring", g.id))
                        .selected_text(w)
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for (x, label) in WIRINGS {
                                changed |= ui.selectable_value(wiring, x, label).changed();
                            }
                        });
                    // Edit as a rectangle (top-left / bottom-right).
                    let (mut tl, mut br) = (corners[0], corners[2]);
                    let moved = point_edit(ui, "tl", &mut tl) | point_edit(ui, "br", &mut br);
                    if moved {
                        *corners = [tl, pt(br.x(), tl.y()), br, pt(tl.x(), br.y())];
                        coalesce = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                let mut b = g.brightness.get();
                ui.label("brightness");
                if ui.add(egui::Slider::new(&mut b, 0.0..=1.0)).changed() {
                    g.brightness = UnitInterval::new(b).unwrap_or(UnitInterval::ONE);
                    coalesce = true;
                }
                let mut linear = g.encoding == ChannelEncoding::Linear;
                if ui
                    .checkbox(&mut linear, "linear")
                    .on_hover_text("Send linear-light values instead of sRGB")
                    .changed()
                {
                    g.encoding = if linear {
                        ChannelEncoding::Linear
                    } else {
                        ChannelEncoding::Srgb
                    };
                    changed = true;
                }
                let (first, last) = g.universe_span();
                ui.weak(if first == last {
                    format!("universe {first}")
                } else {
                    format!("universes {first}–{last}")
                });
            });
        });
        if changed {
            self.queue(Command::PutFixture {
                fixture: g,
                index: None,
            });
        } else if coalesce {
            self.queue_coalescing(
                Command::PutFixture {
                    fixture: g,
                    index: None,
                },
                key,
            );
        }
    }

    /// Draws every fixture's pixels over the canvas preview.
    pub(crate) fn draw_fixtures(&self, painter: &egui::Painter, rect: egui::Rect) {
        let colour = Color32::from_rgb(255, 120, 220);
        for f in &self.session.project().dmx.fixtures {
            let (positions, _) = om_dmx::mapping::pixel_positions(&f.shape);
            let alpha = if f.enabled { 1.0 } else { 0.35 };
            let pts: Vec<Pos2> = positions
                .iter()
                .map(|&(x, y)| to_screen(rect, pt(x, y)))
                .collect();
            // Wiring path, then the pixels.
            if pts.len() > 1 && pts.len() <= 4096 {
                painter.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(1.0, colour.gamma_multiply(0.35 * alpha)),
                ));
            }
            let radius = if pts.len() > 1024 { 1.0 } else { 2.0 };
            for p in pts.iter().take(16_384) {
                painter.circle_filled(*p, radius, colour.gamma_multiply(alpha));
            }
            if let Some(first) = pts.first() {
                painter.text(
                    *first + egui::vec2(4.0, -4.0),
                    egui::Align2::LEFT_BOTTOM,
                    &f.name,
                    egui::FontId::proportional(11.0),
                    colour.gamma_multiply(alpha),
                );
            }
        }
    }
}
