// SPDX-License-Identifier: Apache-2.0
//! Desktop UI. Presentation only: every change the user makes is turned into
//! an `om_command::Command` and sent through the [`Session`]; this crate never
//! mutates the project directly.

mod canvas;
mod gpu;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Stroke};
use om_command::Command;
use std::sync::Arc;

use om_engine::{OpenReport, Session, Transport, path_for_storage};
use om_media_core::{AudioOpener, VideoOpener};
use om_output::Display;
use om_project::{Canvas, Media, MediaSource, Output, PatternKind, Playback, Shape, Surface};
use om_time::{Rate, RationalTime, Speed};
use om_types::{MediaId, OutputId, SurfaceId, UnitInterval};

use crate::canvas::{Drag, begin_drag, dragged_shape, fit_rect, to_screen};
use crate::gpu::Viewer;

/// How often to re-enumerate displays and retry missing media.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Top-level application state.
#[derive(Debug)]
pub struct OpenMapperApp {
    session: Session,
    viewer: Option<Viewer>,
    selected: Option<SurfaceId>,
    /// Text buffer for the project / media path field.
    path_input: String,
    media_path_input: String,
    status: Status,
    rename_buffer: Option<(SurfaceId, String)>,
    project_name_buffer: Option<String>,
    drag: Option<Drag>,
    displays: Vec<Display>,
    display_error: Option<String>,
    last_poll: Option<Instant>,
    /// Commands queued while drawing, applied after the frame's UI pass.
    pending: Vec<(Command, Option<String>)>,
    end_coalescing: bool,
    transport: Transport,
}

#[derive(Debug, Default)]
struct Status {
    message: String,
    is_error: bool,
}

impl OpenMapperApp {
    /// Starts with an empty project, or opens `path` if given. A failed open
    /// falls back to an empty project and shows the error. Without a wgpu
    /// render state the UI runs but cannot show the canvas.
    #[must_use]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        opener: Option<Arc<dyn VideoOpener>>,
        audio_opener: Option<Arc<dyn AudioOpener>>,
    ) -> Self {
        let mut app = Self {
            session: Session::new("Untitled"),
            viewer: cc
                .wgpu_render_state
                .as_ref()
                .map(|rs| Viewer::new(&cc.egui_ctx, rs, opener, audio_opener)),
            selected: None,
            path_input: String::from("untitled.omproj"),
            media_path_input: String::new(),
            status: Status::default(),
            rename_buffer: None,
            project_name_buffer: None,
            drag: None,
            displays: Vec::new(),
            display_error: None,
            last_poll: None,
            pending: Vec::new(),
            end_coalescing: false,
            transport: Transport::default(),
        };
        if app.viewer.is_none() {
            app.error("No GPU renderer available; the canvas cannot be shown.");
        }
        if let Some(p) = path {
            app.path_input = p.display().to_string();
            app.open(p);
        }
        app
    }

    /// Starts the show transport.
    pub fn play(&mut self) {
        self.transport.play(Instant::now());
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = Status {
            message: msg.into(),
            is_error: false,
        };
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status = Status {
            message: msg.into(),
            is_error: true,
        };
    }

    fn queue(&mut self, command: Command) {
        self.pending.push((command, None));
    }

    fn queue_coalescing(&mut self, command: Command, key: String) {
        self.pending.push((command, Some(key)));
    }

    fn apply_pending(&mut self) {
        for (cmd, key) in std::mem::take(&mut self.pending) {
            let result = match key {
                Some(k) => self.session.execute_coalescing(cmd, k),
                None => self.session.execute(cmd),
            };
            if let Err(e) = result {
                self.error(e.to_string());
            }
        }
        if std::mem::take(&mut self.end_coalescing) {
            self.session.break_coalescing();
        }
    }

    fn open(&mut self, path: PathBuf) {
        match Session::open(&path) {
            Ok((session, report)) => {
                self.session = session;
                self.selected = None;
                self.drag = None;
                self.info(open_message(&path, &report));
            }
            Err(e) => self.error(format!("Could not open: {e}")),
        }
    }

    fn save_as(&mut self, path: PathBuf) {
        match self.session.save_as(&path) {
            Ok(()) => self.info(format!("Saved {}", path.display())),
            Err(e) => self.error(format!("Could not save: {e}")),
        }
    }

    fn save(&mut self) {
        match self.session.path().map(PathBuf::from) {
            Some(p) => self.save_as(p),
            None => self.save_as(PathBuf::from(self.path_input.trim())),
        }
    }

    fn undo(&mut self) {
        if let Err(e) = self.session.undo() {
            self.error(e.to_string());
        }
    }

    fn redo(&mut self) {
        if let Err(e) = self.session.redo() {
            self.error(e.to_string());
        }
    }

    fn poll(&mut self) {
        if self.last_poll.is_some_and(|t| t.elapsed() < POLL_INTERVAL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        match om_output::list_displays() {
            Ok(d) => {
                self.displays = d;
                self.display_error = None;
            }
            Err(e) => self.display_error = Some(e.to_string()),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        if ctx.input_mut(|i| i.consume_shortcut(&redo)) {
            self.redo();
        } else if ctx.input_mut(|i| i.consume_shortcut(&undo)) {
            self.undo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            self.save();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    self.session = Session::new("Untitled");
                    self.selected = None;
                    self.info("New project");
                    ui.close();
                }
                if ui.button("Open path").clicked() {
                    self.open(PathBuf::from(self.path_input.trim()));
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    self.save();
                    ui.close();
                }
                if ui.button("Save As path").clicked() {
                    self.save_as(PathBuf::from(self.path_input.trim()));
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                let doc = self.session.document();
                let undo_text = doc
                    .undo_label()
                    .map_or("Undo".to_owned(), |l| format!("Undo {l}"));
                let redo_text = doc
                    .redo_label()
                    .map_or("Redo".to_owned(), |l| format!("Redo {l}"));
                let (can_undo, can_redo) = (doc.can_undo(), doc.can_redo());
                if ui
                    .add_enabled(can_undo, egui::Button::new(undo_text))
                    .clicked()
                {
                    self.undo();
                    ui.close();
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new(redo_text))
                    .clicked()
                {
                    self.redo();
                    ui.close();
                }
            });
            ui.separator();
            ui.label("Project file:");
            ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(280.0));
            ui.separator();
            let now = Instant::now();
            let playing = self.transport.is_playing();
            if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
                if playing {
                    self.transport.pause(now);
                } else {
                    self.transport.play(now);
                }
            }
            if ui.button("Restart").clicked() {
                self.transport.seek(RationalTime::ZERO, now);
            }
            ui.monospace(clock(self.transport.time(now)));
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Project");
            let current = self.session.project().name.clone();
            let buffer = self.project_name_buffer.get_or_insert(current.clone());
            if ui.text_edit_singleline(buffer).lost_focus()
                && let Some(name) = self.project_name_buffer.take()
                && name != current
            {
                self.queue(Command::SetProjectName { name });
            }
            self.canvas_settings(ui);
            ui.separator();
            self.surface_list(ui);
            ui.separator();
            self.media_list(ui);
            ui.separator();
            self.output_list(ui);
        });
    }

    fn canvas_settings(&mut self, ui: &mut egui::Ui) {
        let Canvas {
            mut width,
            mut height,
        } = self.session.project().canvas;
        ui.horizontal(|ui| {
            ui.label("Canvas");
            let max = Canvas::MAX_DIMENSION;
            let w = ui.add(
                egui::DragValue::new(&mut width)
                    .range(1..=max)
                    .suffix(" px"),
            );
            ui.label("×");
            let h = ui.add(
                egui::DragValue::new(&mut height)
                    .range(1..=max)
                    .suffix(" px"),
            );
            if (w.changed() || h.changed()) && !(w.dragged() || h.dragged()) {
                self.queue(Command::SetCanvas {
                    canvas: Canvas { width, height },
                });
            }
        });
    }

    fn surface_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Surfaces");
            for (label, shape) in [
                ("+ Quad", Shape::centred_quad()),
                ("+ Triangle", Shape::centred_triangle()),
            ] {
                if ui.small_button(label).clicked() {
                    let n = self.session.project().surfaces.len() + 1;
                    let id = SurfaceId::new();
                    let mut s = Surface::new(id, format!("Surface {n}"));
                    s.shape = shape;
                    s.media = self.session.project().media.first().map(|m| m.id);
                    self.queue(Command::AddSurface {
                        surface: s,
                        index: None,
                    });
                    self.selected = Some(id);
                }
            }
        });
        let surfaces: Vec<(SurfaceId, String, bool)> = self
            .session
            .project()
            .surfaces
            .iter()
            .rev() // topmost first
            .map(|s| (s.id, s.name.clone(), s.enabled))
            .collect();
        if surfaces.is_empty() {
            ui.weak("No surfaces yet.");
        }
        for (id, name, enabled) in surfaces {
            let label = if enabled {
                name
            } else {
                format!("{name} (off)")
            };
            if ui
                .selectable_label(self.selected == Some(id), label)
                .clicked()
            {
                self.selected = Some(id);
                self.rename_buffer = None;
            }
        }
    }

    fn media_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Media");
        ui.horizontal(|ui| {
            for (label, pattern) in [
                ("+ UV grid", PatternKind::UvGrid),
                ("+ Checker", PatternKind::Checkerboard),
                ("+ White", PatternKind::White),
            ] {
                if ui.small_button(label).clicked() {
                    self.queue(Command::AddMedia {
                        media: Media {
                            id: MediaId::new(),
                            name: label.trim_start_matches("+ ").to_owned(),
                            source: MediaSource::Pattern { pattern },
                            playback: Default::default(),
                            extensions: Default::default(),
                        },
                        index: None,
                    });
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.media_path_input)
                    .hint_text("image, video or image-sequence folder path")
                    .desired_width(170.0),
            );
            let path = self.media_path_input.trim().to_owned();
            if ui
                .add_enabled(!path.is_empty(), egui::Button::new("Add"))
                .clicked()
            {
                let chosen = PathBuf::from(&path);
                let name = chosen
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                let stored = path_for_storage(self.session.project_dir(), &chosen);
                self.queue(Command::AddMedia {
                    media: Media {
                        id: MediaId::new(),
                        name,
                        source: source_for_path(&chosen, stored),
                        playback: Default::default(),
                        extensions: Default::default(),
                    },
                    index: None,
                });
                self.media_path_input.clear();
            }
        });
        let show = self.transport.time(Instant::now());
        let media = self.session.project().media.clone();
        for m in media {
            let status = self
                .viewer
                .as_ref()
                .and_then(|v| v.media.status(self.session.project(), m.id, show));
            ui.horizontal(|ui| {
                match status.as_ref().and_then(|s| s.error.clone()) {
                    Some(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, "missing")
                            .on_hover_text(e);
                    }
                    None => {
                        ui.label("•");
                    }
                }
                if let Some(t) = self.viewer.as_ref().and_then(|v| v.thumbnail(m.id)) {
                    let size = t.size_vec2();
                    let scale = 32.0 / size.y.max(1.0);
                    ui.image((t.id(), size * scale));
                }
                let label = ui.label(&m.name);
                if let Some(summary) = status.as_ref().and_then(|s| s.summary.clone()) {
                    label.on_hover_text(summary);
                }
                if ui.small_button("Remove").clicked() {
                    self.queue(Command::RemoveMedia { id: m.id });
                }
            });
            if m.source.is_time_based() {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let mut pb = m.playback;
                    if ui.checkbox(&mut pb.looping, "Loop").changed() {
                        self.queue(Command::SetMediaPlayback {
                            id: m.id,
                            playback: pb,
                        });
                    }
                    let mut vol = pb.volume.get();
                    let resp = ui.add(
                        egui::DragValue::new(&mut vol)
                            .range(0.0..=1.0)
                            .speed(0.01)
                            .prefix("vol "),
                    );
                    if resp.changed() {
                        self.queue_coalescing(
                            Command::SetMediaPlayback {
                                id: m.id,
                                playback: Playback {
                                    volume: om_types::UnitInterval::saturating(vol),
                                    ..pb
                                },
                            },
                            format!("volume:{}", m.id),
                        );
                    }
                    if resp.drag_stopped() || resp.lost_focus() {
                        self.end_coalescing = true;
                    }
                    let mut percent = pb.speed.num() * 100 / pb.speed.den();
                    let resp = ui.add(
                        egui::DragValue::new(&mut percent)
                            .range(-1600..=1600)
                            .suffix("%")
                            .speed(1.0),
                    );
                    if resp.changed()
                        && let Ok(speed) = Speed::from_percent(percent)
                    {
                        self.queue_coalescing(
                            Command::SetMediaPlayback {
                                id: m.id,
                                playback: Playback { speed, ..pb },
                            },
                            format!("speed:{}", m.id),
                        );
                    }
                    if resp.drag_stopped() || resp.lost_focus() {
                        self.end_coalescing = true;
                    }
                    if ui.small_button("Restart").clicked()
                        && let Some(v) = &mut self.viewer
                    {
                        v.media.restart(m.id, show);
                    }
                    if let Some(st) = &status
                        && let (Some(p), Some(d)) = (st.position, st.duration)
                    {
                        ui.weak(format!("{} / {}", clock(p), clock(d)));
                    }
                });
            }
        }
    }

    fn output_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Outputs");
            if ui.small_button("+ Output").clicked() {
                let n = self.session.project().outputs.len() + 1;
                let display = self
                    .displays
                    .iter()
                    .find(|d| !d.is_primary)
                    .map(Display::target);
                self.queue(Command::AddOutput {
                    output: Output {
                        id: OutputId::new(),
                        name: format!("Output {n}"),
                        enabled: false,
                        display,
                        extensions: Default::default(),
                    },
                    index: None,
                });
            }
        });
        if let Some(e) = &self.display_error {
            ui.colored_label(ui.visuals().warn_fg_color, e);
        }
        let outputs = self.session.project().outputs.clone();
        for o in outputs {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let mut enabled = o.enabled;
                    if ui.checkbox(&mut enabled, &o.name).changed() {
                        self.queue(Command::UpdateOutput {
                            id: o.id,
                            name: None,
                            enabled: Some(enabled),
                        });
                    }
                    if ui.small_button("Remove").clicked() {
                        self.queue(Command::RemoveOutput { id: o.id });
                    }
                });
                let resolved = o
                    .display
                    .as_ref()
                    .and_then(|t| om_output::resolve(t, &self.displays));
                let current = o
                    .display
                    .as_ref()
                    .map_or("(no display)".to_owned(), |t| t.name.clone());
                egui::ComboBox::from_id_salt(o.id)
                    .selected_text(current)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for d in &self.displays {
                            if ui
                                .selectable_label(
                                    resolved.is_some_and(|r| r.index == d.index),
                                    d.to_string(),
                                )
                                .clicked()
                            {
                                self.pending.push((
                                    Command::SetOutputDisplay {
                                        id: o.id,
                                        display: Some(d.target()),
                                    },
                                    None,
                                ));
                            }
                        }
                    });
                match resolved {
                    Some(d) => {
                        let (w, h) = d.native_size();
                        ui.horizontal(|ui| {
                            ui.weak(format!("connected, {w}×{h}"));
                            let canvas = self.session.project().canvas;
                            if (canvas.width, canvas.height) != (w, h)
                                && ui.small_button("Match canvas").clicked()
                            {
                                self.pending.push((
                                    Command::SetCanvas {
                                        canvas: Canvas {
                                            width: w,
                                            height: h,
                                        },
                                    },
                                    None,
                                ));
                            }
                        });
                    }
                    None if o.display.is_some() => {
                        ui.colored_label(ui.visuals().warn_fg_color, "display not connected");
                    }
                    None => {}
                }
            });
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.weak("Select a surface to edit it.");
            if let Some(v) = &self.viewer {
                ui.separator();
                ui.weak(v.gpu_summary());
                ui.weak(&v.audio_status);
            }
            return;
        };
        let Some(surface) = self.session.project().surface(id).cloned() else {
            self.selected = None;
            return;
        };
        ui.heading("Surface");

        let buffer = match &mut self.rename_buffer {
            Some((bid, text)) if *bid == id => text,
            slot => &mut slot.insert((id, surface.name.clone())).1,
        };
        if ui.text_edit_singleline(buffer).lost_focus()
            && let Some((_, name)) = self.rename_buffer.take()
            && name != surface.name
        {
            self.queue(Command::UpdateSurface {
                id,
                name: Some(name),
                enabled: None,
                opacity: None,
            });
        }

        let mut enabled = surface.enabled;
        if ui.checkbox(&mut enabled, "Enabled").changed() {
            self.queue(Command::UpdateSurface {
                id,
                name: None,
                enabled: Some(enabled),
                opacity: None,
            });
        }

        let mut opacity = surface.opacity.get();
        let resp = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity"));
        if resp.changed() {
            self.queue_coalescing(
                Command::UpdateSurface {
                    id,
                    name: None,
                    enabled: None,
                    opacity: Some(UnitInterval::saturating(opacity)),
                },
                format!("opacity:{id}"),
            );
        }
        if resp.drag_stopped() || resp.lost_focus() {
            self.end_coalescing = true;
        }

        let media_name = surface
            .media
            .and_then(|m| self.session.project().media_item(m))
            .map_or("(none)".to_owned(), |m| m.name.clone());
        egui::ComboBox::from_label("Media")
            .selected_text(media_name)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(surface.media.is_none(), "(none)")
                    .clicked()
                {
                    self.pending
                        .push((Command::SetSurfaceMedia { id, media: None }, None));
                }
                for m in &self.session.project().media {
                    if ui
                        .selectable_label(surface.media == Some(m.id), &m.name)
                        .clicked()
                    {
                        self.pending.push((
                            Command::SetSurfaceMedia {
                                id,
                                media: Some(m.id),
                            },
                            None,
                        ));
                    }
                }
            });

        if let Some(reason) = self
            .viewer
            .as_ref()
            .and_then(|v| v.last_frame.as_ref())
            .and_then(|f| f.plan.skipped.iter().find(|(s, _)| *s == id))
            .map(|(_, r)| r.to_string())
        {
            ui.colored_label(ui.visuals().warn_fg_color, format!("Not drawn: {reason}"));
        }

        ui.horizontal(|ui| {
            if ui.button("Reset shape").clicked() {
                let shape = match surface.shape {
                    Shape::Quad { .. } => Shape::centred_quad(),
                    Shape::Triangle { .. } => Shape::centred_triangle(),
                };
                self.queue(Command::SetSurfaceShape { id, shape });
            }
            if ui.button("Fill canvas").clicked() {
                self.queue(Command::SetSurfaceShape {
                    id,
                    shape: Shape::full_quad(),
                });
            }
        });
        ui.weak("Drag corners on the canvas; drag inside to move.");

        ui.add_space(12.0);
        if ui.button("Remove surface").clicked() {
            self.queue(Command::RemoveSurface { id });
            self.selected = None;
        }
    }

    fn canvas_view(&mut self, ui: &mut egui::Ui) {
        let project_dir = self.session.project_dir().map(PathBuf::from);
        let texture = match &mut self.viewer {
            Some(v) => v.update(
                self.session.project(),
                project_dir.as_deref(),
                self.transport.time(Instant::now()),
                &self.transport,
            ),
            None => None,
        };
        let canvas = self.session.project().canvas;
        let avail = ui.available_rect_before_wrap();
        let rect = fit_rect(avail.shrink(8.0), (canvas.width, canvas.height));
        let response = ui.allocate_rect(avail, egui::Sense::click_and_drag());
        let painter = ui.painter_at(avail);
        match texture {
            Some(id) => {
                painter.image(
                    id,
                    rect,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(rect, 0.0, Color32::BLACK);
                let msg = self
                    .viewer
                    .as_ref()
                    .and_then(|v| v.last_error.clone())
                    .unwrap_or_else(|| "Renderer unavailable".into());
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::default(),
                    Color32::LIGHT_RED,
                );
            }
        }
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, Color32::from_gray(80)),
            egui::StrokeKind::Outside,
        );

        // Outlines for every surface; handles for the selected one.
        let project = self.session.project();
        for s in &project.surfaces {
            let pts: Vec<Pos2> = s
                .shape
                .corners()
                .iter()
                .map(|c| to_screen(rect, *c))
                .collect();
            let selected = self.selected == Some(s.id);
            let colour = if selected {
                Color32::from_rgb(255, 200, 40)
            } else {
                Color32::from_white_alpha(90)
            };
            painter.add(egui::Shape::closed_line(
                pts.clone(),
                Stroke::new(if selected { 1.5 } else { 1.0 }, colour),
            ));
            if selected {
                for (i, p) in pts.iter().enumerate() {
                    painter.circle(
                        *p,
                        5.0,
                        Color32::from_black_alpha(160),
                        Stroke::new(1.5, colour),
                    );
                    painter.text(
                        *p + egui::vec2(8.0, -8.0),
                        egui::Align2::LEFT_BOTTOM,
                        i.to_string(),
                        egui::FontId::monospace(10.0),
                        colour,
                    );
                }
            }
        }

        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.drag = begin_drag(project, rect, self.selected, pos);
            if let Some(d) = &self.drag {
                self.selected = Some(d.surface);
            }
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.selected = begin_drag(project, rect, self.selected, pos).map(|d| d.surface);
        }
        if let (Some(drag), Some(pos)) = (self.drag, response.interact_pointer_pos())
            && response.dragged()
            && let Some(current) = project.surface(drag.surface).map(|s| s.shape)
            && let Some(shape) = dragged_shape(rect, &current, &drag, pos)
            && shape != current
        {
            self.queue_coalescing(
                Command::SetSurfaceShape {
                    id: drag.surface,
                    shape,
                },
                format!("shape:{}", drag.surface),
            );
        }
        if response.drag_stopped() {
            self.drag = None;
            self.end_coalescing = true;
        }
    }

    /// Opens a borderless fullscreen window for every enabled output whose
    /// display is connected. A window closed by the OS disables its output.
    fn output_windows(&mut self, ctx: &egui::Context) {
        let Some(texture) = self
            .viewer
            .as_ref()
            .and_then(|v| v.last_error.is_none().then_some(()))
            .and(self.preview_id())
        else {
            return;
        };
        let outputs = self.session.project().outputs.clone();
        for o in outputs.iter().filter(|o| o.enabled) {
            let Some(display) = o
                .display
                .as_ref()
                .and_then(|t| om_output::resolve(t, &self.displays))
            else {
                continue;
            };
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("OpenMapper — {}", o.name))
                .with_monitor(display.index as usize)
                .with_decorations(false);
            let id = egui::ViewportId::from_hash_of(("output", o.id));
            let close = ctx.show_viewport_immediate(id, builder, |ui, _class| {
                let rect = ui.max_rect();
                ui.painter().image(
                    texture,
                    rect,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
            });
            if close {
                self.queue(Command::UpdateOutput {
                    id: o.id,
                    name: None,
                    enabled: Some(false),
                });
            }
        }
    }

    fn preview_id(&self) -> Option<egui::TextureId> {
        self.viewer.as_ref().and_then(|v| v.preview_id())
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let p = self.session.project();
            let file = self
                .session
                .path()
                .map_or("(unsaved)".to_owned(), |p| p.display().to_string());
            let dirty = if self.session.is_dirty() { " •" } else { "" };
            ui.label(format!("{file}{dirty}"));
            ui.separator();
            ui.label(format!("rev {}", p.revision));
            ui.separator();
            ui.label(format!("{}×{}", p.canvas.width, p.canvas.height));
            if let Some(err) = self.session.journal_error() {
                ui.separator();
                ui.colored_label(ui.visuals().warn_fg_color, format!("journal: {err}"));
            }
            if !self.status.message.is_empty() {
                ui.separator();
                if self.status.is_error {
                    ui.colored_label(ui.visuals().error_fg_color, &self.status.message);
                } else {
                    ui.label(&self.status.message);
                }
            }
        });
    }
}

/// `m:ss.mmm` for display.
fn clock(t: RationalTime) -> String {
    let ms = t.to_ticks_floor(1000).unwrap_or(0).max(0);
    format!("{}:{:02}.{:03}", ms / 60_000, (ms / 1000) % 60, ms % 1000)
}

/// Picks the media source kind from a chosen path.
fn source_for_path(chosen: &std::path::Path, stored: String) -> MediaSource {
    const VIDEO: &[&str] = &[
        "mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mpg", "mpeg", "ts",
    ];
    if chosen.is_dir() {
        return MediaSource::Sequence {
            path: stored,
            rate: Rate::FPS_30,
        };
    }
    let ext = chosen
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if VIDEO.contains(&ext.as_str()) {
        MediaSource::Video { path: stored }
    } else {
        MediaSource::Image { path: stored }
    }
}

fn open_message(path: &std::path::Path, report: &OpenReport) -> String {
    let mut msg = format!("Opened {}", path.display());
    if let Some(v) = report.migrated_from {
        msg.push_str(&format!(" (migrated from v{v})"));
    }
    if report.recovered_commands > 0 {
        msg.push_str(&format!(
            "; recovered {} unsaved change(s) — save to keep them",
            report.recovered_commands
        ));
    }
    for w in &report.warnings {
        msg.push_str("; ");
        msg.push_str(w);
    }
    msg
}

impl eframe::App for OpenMapperApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.handle_shortcuts(&ctx);
        egui::Panel::top("menu").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("project")
            .default_size(260.0)
            .show(ui, |ui| self.left_panel(ui));
        egui::Panel::right("inspector")
            .default_size(260.0)
            .show(ui, |ui| self.inspector(ui));
        egui::CentralPanel::default().show(ui, |ui| self.canvas_view(ui));
        self.output_windows(&ctx);
        self.apply_pending();
        if self.transport.is_playing() {
            ctx.request_repaint();
        } else {
            // Keep polling displays/media even when idle.
            ctx.request_repaint_after(POLL_INTERVAL);
        }
    }
}
