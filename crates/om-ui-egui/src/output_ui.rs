// SPDX-License-Identifier: Apache-2.0
//! Output mapping controls: canvas region, corner pin and soft edges.

use eframe::egui;
use om_command::Command;
use om_geom::Point2;
use om_project::{Output, OutputMapping, Projection, SoftEdge};
use om_types::Finite;

use crate::OpenMapperApp;

fn pt(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap_or_default()
}

fn drag(ui: &mut egui::Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>) -> bool {
    ui.add(
        egui::DragValue::new(v)
            .speed(0.002)
            .range(range)
            .max_decimals(3),
    )
    .changed()
}

fn corner(ui: &mut egui::Ui, label: &str, p: &mut Point2) -> bool {
    let (mut x, mut y) = p.to_tuple();
    ui.label(label);
    let changed = drag(ui, &mut x, -1.0..=2.0) | drag(ui, &mut y, -1.0..=2.0);
    if changed {
        *p = pt(x, y);
    }
    changed
}

fn finite(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut Finite,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let mut x = v.get();
    ui.label(label);
    let changed = drag(ui, &mut x, range);
    if changed && let Ok(f) = Finite::new(x) {
        *v = f;
    }
    changed
}

impl OpenMapperApp {
    pub(crate) fn output_mapping_controls(&mut self, ui: &mut egui::Ui, o: &Output) {
        let mut m = o.mapping.clone();
        let mut changed = false;
        egui::CollapsingHeader::new("Mapping")
            .id_salt(("mapping", o.id))
            .default_open(!m.is_identity())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Canvas region");
                    let (mut x0, mut y0) = m.region[0].to_tuple();
                    let (mut x1, mut y1) = m.region[2].to_tuple();
                    let moved = drag(ui, &mut x0, 0.0..=1.0)
                        | drag(ui, &mut y0, 0.0..=1.0)
                        | drag(ui, &mut x1, 0.0..=1.0)
                        | drag(ui, &mut y1, 0.0..=1.0);
                    if moved && x1 > x0 && y1 > y0 {
                        m.region = [pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)];
                        changed = true;
                    }
                })
                .response
                .on_hover_text("Left, top, right, bottom of the canvas area this output shows");
                ui.label("Corner pin (output space)");
                let names = ["TL", "TR", "BR", "BL"];
                for row in [[0usize, 1], [3, 2]] {
                    ui.horizontal(|ui| {
                        for k in row {
                            changed |= corner(ui, names[k], &mut m.warp[k]);
                        }
                    });
                }
                ui.label("Soft edges (fraction of the region)");
                ui.horizontal(|ui| {
                    let e: &mut SoftEdge = &mut m.soft_edge;
                    changed |= finite(ui, "L", &mut e.left, 0.0..=0.5);
                    changed |= finite(ui, "R", &mut e.right, 0.0..=0.5);
                    changed |= finite(ui, "T", &mut e.top, 0.0..=0.5);
                    changed |= finite(ui, "B", &mut e.bottom, 0.0..=0.5);
                });
                ui.horizontal(|ui| {
                    let e: &mut SoftEdge = &mut m.soft_edge;
                    changed |= finite(ui, "curve", &mut e.curve, 1.0..=8.0);
                    changed |= finite(ui, "gamma", &mut e.gamma, 0.5..=4.0);
                    if ui.small_button("Reset").clicked() {
                        m = OutputMapping::default();
                        changed = true;
                    }
                });
            });
        if changed && m.validate().is_ok() {
            self.queue_coalescing(
                Command::SetOutputMapping {
                    id: o.id,
                    mapping: m,
                },
                format!("output-mapping-{}", o.id),
            );
        }
    }
}

fn finite_drag(ui: &mut egui::Ui, v: &mut Finite, speed: f64) -> bool {
    let mut x = v.get();
    let changed = ui
        .add(egui::DragValue::new(&mut x).speed(speed).max_decimals(3))
        .changed();
    if changed && let Ok(f) = Finite::new(x) {
        *v = f;
    }
    changed
}

impl OpenMapperApp {
    /// 3-D projection: model, calibration points and calibration.
    pub(crate) fn output_projection_controls(&mut self, ui: &mut egui::Ui, o: &Output) {
        let Some(projection) = o.projection.clone() else {
            ui.horizontal(|ui| {
                if ui
                    .small_button("Use 3D model")
                    .on_hover_text("Show an OBJ model, textured with the canvas, through a calibrated projector")
                    .clicked()
                {
                    self.queue(Command::SetOutputProjection {
                        id: o.id,
                        projection: Some(Projection {
                            model: "model.obj".into(),
                            projector: None,
                            points: Vec::new(),
                        }),
                    });
                }
            });
            return;
        };
        let mut p = projection.clone();
        let mut changed = false;
        egui::CollapsingHeader::new("3D projection")
            .id_salt(("projection", o.id))
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Model");
                    let mut path = p.model.clone();
                    if ui
                        .add(egui::TextEdit::singleline(&mut path).desired_width(140.0))
                        .lost_focus()
                        && !path.trim().is_empty()
                        && path != p.model
                    {
                        p.model = path.trim().to_owned();
                        changed = true;
                    }
                    if ui.small_button("Reload").clicked()
                        && let Some(v) = &mut self.viewer
                    {
                        v.reload_model(&p.model);
                    }
                });
                match self.viewer.as_ref().and_then(|v| v.models.get(&p.model)) {
                    Some(Ok(n)) => {
                        ui.weak(format!("{n} triangles"));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    None => {}
                }
                match &p.projector {
                    Some(k) => {
                        ui.weak(format!(
                            "Calibrated {}×{}: f {:.1}/{:.1}, centre {:.1}, {:.1}",
                            k.width,
                            k.height,
                            k.fx.get(),
                            k.fy.get(),
                            k.cx.get(),
                            k.cy.get()
                        ));
                    }
                    None => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Not calibrated: add at least 6 points (not all on one plane)",
                        );
                    }
                }
                ui.label("Points: model x y z → projector pixel u v");
                let mut remove = None;
                for (k, pt) in p.points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        for v in &mut pt.world {
                            changed |= finite_drag(ui, v, 0.005);
                        }
                        ui.label("→");
                        for v in &mut pt.pixel {
                            changed |= finite_drag(ui, v, 0.25);
                        }
                        if ui.small_button("×").clicked() {
                            remove = Some(k);
                        }
                    });
                }
                if let Some(k) = remove {
                    p.points.remove(k);
                    changed = true;
                }
                ui.horizontal(|ui| {
                    if ui.small_button("+ Point").clicked()
                        && p.points.len() < om_project::MAX_CALIBRATION_POINTS
                    {
                        p.points.push(om_project::CalibrationPoint {
                            world: [Finite::ZERO; 3],
                            pixel: [Finite::ZERO; 2],
                        });
                        changed = true;
                    }
                    let (w, h) = o
                        .display
                        .as_ref()
                        .and_then(|t| om_output::resolve(t, &self.displays))
                        .map_or((1920, 1080), om_output::Display::native_size);
                    if ui
                        .add_enabled(p.points.len() >= 6, egui::Button::new("Calibrate"))
                        .on_hover_text(format!("Fit the projector at {w}×{h}"))
                        .clicked()
                    {
                        match om_calibration::calibrate_projection(&p, w, h) {
                            Ok(c) => match c.projector.to_params() {
                                Some(params) => {
                                    p.projector = Some(params);
                                    changed = true;
                                    self.info(format!(
                                        "Calibrated: RMS reprojection error {:.2} px",
                                        c.rms_error
                                    ));
                                }
                                None => self.error("Calibration produced invalid numbers"),
                            },
                            Err(e) => self.error(format!("Calibration failed: {e}")),
                        }
                    }
                    if ui.small_button("Remove 3D").clicked() {
                        self.queue(Command::SetOutputProjection {
                            id: o.id,
                            projection: None,
                        });
                    }
                });
            });
        if changed && p != projection && p.validate().is_ok() {
            self.queue_coalescing(
                Command::SetOutputProjection {
                    id: o.id,
                    projection: Some(p),
                },
                format!("output-projection-{}", o.id),
            );
        }
    }
}
