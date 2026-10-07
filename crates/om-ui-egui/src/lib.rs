// SPDX-License-Identifier: Apache-2.0
//! Desktop UI. Presentation only: every change the user makes is turned into
//! an `om_command::Command` and sent through the [`Session`]; this crate never
//! mutates the project directly.

mod canvas;
mod dmx_ui;
mod gpu;
mod live_ui;
mod output_ui;
mod plugin_ui;
mod show_ui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Stroke};
use om_command::Command;

use om_engine::{Adapters, OpenReport, Session, Transport, path_for_storage};
use om_output::Display;
use om_project::{
    BlendMode, Canvas, MAX_MASK_POINTS, Mask, MaskPoint, Media, MediaSource, Output, PatternKind,
    Playback, Shape, Surface,
};
use om_time::{Rate, RationalTime, Speed};
use om_types::{MediaId, OutputId, SurfaceId, UnitInterval};

use crate::canvas::{Drag, begin_drag, dragged_shape, fit_rect, screen_outline, to_screen};
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
    /// Folder searched by "Relink", and the cached count of missing media.
    relink_folder: String,
    missing_media: Option<(Instant, usize)>,
    status: Status,
    rename_buffer: Option<(SurfaceId, String)>,
    project_name_buffer: Option<String>,
    drag: Option<Drag>,
    /// Canvas handles edit the selected surface's mask instead of its shape.
    mask_mode: bool,
    /// Show runtime, control servers, MIDI.
    live: om_engine::live::Live,
    /// The project as rendered this frame (document + show overrides).
    effective: Option<om_project::Project>,
    show_tab: show_ui::ShowTab,
    live_ui: live_ui::LiveUi,
    dmx_ui: dmx_ui::DmxUi,
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
        adapters: &Adapters,
    ) -> Self {
        let mut app = Self {
            session: Session::new("Untitled"),
            viewer: cc
                .wgpu_render_state
                .as_ref()
                .map(|rs| Viewer::new(&cc.egui_ctx, rs, adapters)),
            selected: None,
            path_input: String::from("untitled.omproj"),
            media_path_input: String::new(),
            relink_folder: String::new(),
            missing_media: None,
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
            mask_mode: false,
            live: om_engine::live::Live::new(),
            effective: None,
            show_tab: show_ui::ShowTab::Cues,
            dmx_ui: dmx_ui::DmxUi::default(),
            live_ui: live_ui::LiveUi::new(adapters.live.clone().map(om_engine::Discovery::new)),
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
            ui.separator();
            self.master_controls(ui);
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
            ui.separator();
            self.dmx_panel(ui);
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
                ("+ Ellipse", Shape::centred_ellipse()),
                ("+ Line", Shape::centred_line()),
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

    /// Shown when media files are missing: search a folder and relink.
    fn relink_controls(&mut self, ui: &mut egui::Ui) {
        let stale = self
            .missing_media
            .is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(2));
        if stale {
            let n = om_engine::relink::missing(self.session.project(), self.session.project_dir())
                .len();
            self.missing_media = Some((Instant::now(), n));
        }
        let missing = self.missing_media.map_or(0, |(_, n)| n);
        if missing == 0 {
            return;
        }
        ui.horizontal(|ui| {
            ui.colored_label(ui.visuals().warn_fg_color, format!("{missing} missing"));
            ui.add(
                egui::TextEdit::singleline(&mut self.relink_folder)
                    .hint_text("folder to search")
                    .desired_width(130.0),
            );
            let folder = PathBuf::from(self.relink_folder.trim());
            if ui
                .add_enabled(
                    !self.relink_folder.trim().is_empty(),
                    egui::Button::new("Relink"),
                )
                .on_hover_text("Find missing files by name in this folder (one undo step)")
                .clicked()
            {
                let found = om_engine::relink::find(
                    self.session.project(),
                    self.session.project_dir(),
                    &folder,
                );
                match om_engine::relink::command(&found) {
                    Some(cmd) => {
                        self.queue(cmd);
                        self.info(format!(
                            "Relinked {} of {missing} missing media",
                            found.len()
                        ));
                        self.missing_media = None;
                    }
                    None => self.error(format!("No missing media found in {}", folder.display())),
                }
            }
        });
    }

    fn media_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Media");
        self.relink_controls(ui);
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
                            plugins: Vec::new(),
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
                        plugins: Vec::new(),
                        extensions: Default::default(),
                    },
                    index: None,
                });
                self.media_path_input.clear();
            }
        });
        self.live_input_controls(ui);
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
            if let MediaSource::Shader { path, inputs } = &m.source {
                let compiled = self
                    .viewer
                    .as_ref()
                    .and_then(|v| v.media.shader(path))
                    .cloned();
                if let Some(compiled) = compiled {
                    let mut inputs = inputs.clone();
                    let id = m.id;
                    let path = path.clone();
                    egui::CollapsingHeader::new("Shader inputs")
                        .id_salt(("shader", id))
                        .show(ui, |ui| {
                            let edit = shader_input_controls(ui, &compiled, &mut inputs);
                            if edit.changed {
                                let cmd = Command::SetMediaSource {
                                    id,
                                    source: MediaSource::Shader {
                                        path: path.clone(),
                                        inputs: inputs.clone(),
                                    },
                                };
                                if edit.dragging {
                                    self.queue_coalescing(cmd, format!("shader:{id}"));
                                } else {
                                    self.queue(cmd);
                                }
                            }
                            if edit.finished {
                                self.end_coalescing = true;
                            }
                        });
                }
            }
            self.media_plugin_controls(ui, &m);
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
                        publish: Vec::new(),
                        mapping: Default::default(),
                        projection: None,
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
                self.publish_controls(ui, &o);
                self.output_mapping_controls(ui, &o);
                self.output_projection_controls(ui, &o);
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
                    Shape::Quad { .. } | Shape::Mesh { .. } => Shape::centred_quad(),
                    Shape::Triangle { .. } => Shape::centred_triangle(),
                    Shape::Ellipse { .. } => Shape::centred_ellipse(),
                    Shape::Line { .. } => Shape::centred_line(),
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
        if matches!(surface.shape, Shape::Quad { .. }) {
            ui.horizontal(|ui| {
                ui.label("Convert to mesh:");
                for n in [2u16, 4, 8] {
                    if ui.small_button(format!("{n}×{n}")).clicked()
                        && let Some(shape) = surface.shape.to_mesh(n, n)
                    {
                        self.queue(Command::SetSurfaceShape { id, shape });
                    }
                }
            });
        }
        if let Shape::Line { ends, width, uv } = &surface.shape {
            let mut w = width.get() * 100.0;
            let resp = ui.add(egui::Slider::new(&mut w, 0.1..=25.0).text("Width % of height"));
            if resp.changed()
                && let Ok(width) = om_types::Finite::new(w / 100.0)
            {
                self.queue_coalescing(
                    Command::SetSurfaceShape {
                        id,
                        shape: Shape::Line {
                            ends: *ends,
                            width,
                            uv: *uv,
                        },
                    },
                    format!("width:{id}"),
                );
            }
            if resp.drag_stopped() || resp.lost_focus() {
                self.end_coalescing = true;
            }
        }
        self.mask_controls(ui, &surface);
        self.effect_controls(ui, &surface);
        egui::ComboBox::from_label("Blend")
            .selected_text(format!("{:?}", surface.blend))
            .show_ui(ui, |ui| {
                for b in [
                    BlendMode::Normal,
                    BlendMode::Add,
                    BlendMode::Screen,
                    BlendMode::Multiply,
                ] {
                    if ui
                        .selectable_label(surface.blend == b, format!("{b:?}"))
                        .clicked()
                    {
                        self.pending
                            .push((Command::SetSurfaceBlend { id, blend: b }, None));
                    }
                }
            });
        ui.weak("Drag points on the canvas; drag inside to move.");

        ui.add_space(12.0);
        if ui.button("Remove surface").clicked() {
            self.queue(Command::RemoveSurface { id });
            self.selected = None;
        }
    }

    fn effect_controls(&mut self, ui: &mut egui::Ui, surface: &Surface) {
        use om_project::{Effect, EffectKind, MAX_BLUR_RADIUS, MAX_EFFECTS};
        let id = surface.id;
        ui.separator();
        let mut effects = surface.effects.clone();
        let mut changed: Option<bool> = None; // Some(coalesce?)
        ui.horizontal(|ui| {
            ui.label("Effects");
            ui.add_enabled_ui(effects.len() < MAX_EFFECTS, |ui| {
                ui.menu_button("+ Add", |ui| {
                    let options = [
                        EffectKind::neutral_color(),
                        EffectKind::Invert {},
                        EffectKind::Blur {
                            radius: om_types::Finite::new(4.0).unwrap_or(om_types::Finite::ZERO),
                        },
                        EffectKind::Pixelate { size: 8 },
                    ];
                    let shader_path = self.media_path_input.trim().to_owned();
                    if shader_path.ends_with(".fs") {
                        if ui.button(format!("Shader: {shader_path}")).clicked() {
                            let stored = path_for_storage(
                                self.session.project_dir(),
                                std::path::Path::new(&shader_path),
                            );
                            effects.push(Effect {
                                enabled: true,
                                kind: EffectKind::Shader {
                                    path: stored,
                                    inputs: Default::default(),
                                },
                            });
                            changed = Some(false);
                            ui.close();
                        }
                    } else {
                        ui.weak("Shader: type a .fs path in the media path field");
                    }
                    for kind in options {
                        if ui.button(kind.label()).clicked() {
                            effects.push(Effect {
                                enabled: true,
                                kind,
                            });
                            changed = Some(false);
                            ui.close();
                        }
                    }
                });
            });
        });
        let n = effects.len();
        let mut remove = None;
        let mut swap = None;
        for (i, e) in effects.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut e.enabled, e.kind.label()).changed() {
                        changed = Some(false);
                    }
                    if ui
                        .add_enabled(i > 0, egui::Button::new("↑").small())
                        .clicked()
                    {
                        swap = Some((i, i - 1));
                    }
                    if ui
                        .add_enabled(i + 1 < n, egui::Button::new("↓").small())
                        .clicked()
                    {
                        swap = Some((i, i + 1));
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
                let mut slider = |ui: &mut egui::Ui,
                                  v: &mut om_types::Finite,
                                  range: std::ops::RangeInclusive<f64>,
                                  text: &str| {
                    let mut x = v.get();
                    let r = ui.add(egui::Slider::new(&mut x, range).text(text));
                    if r.changed()
                        && let Ok(f) = om_types::Finite::new(x)
                    {
                        *v = f;
                        changed = Some(true);
                    }
                    if r.drag_stopped() || r.lost_focus() {
                        self.end_coalescing = true;
                    }
                };
                match &mut e.kind {
                    EffectKind::Color {
                        brightness,
                        contrast,
                        saturation,
                        hue,
                        gamma,
                    } => {
                        slider(ui, brightness, -1.0..=1.0, "Brightness");
                        slider(ui, contrast, 0.0..=4.0, "Contrast");
                        slider(ui, saturation, 0.0..=4.0, "Saturation");
                        slider(ui, hue, -180.0..=180.0, "Hue°");
                        slider(ui, gamma, 0.1..=10.0, "Gamma");
                    }
                    EffectKind::Blur { radius } => {
                        slider(ui, radius, 0.0..=MAX_BLUR_RADIUS, "Radius px")
                    }
                    EffectKind::Pixelate { size } => {
                        let r = ui.add(egui::Slider::new(size, 1..=256).text("Block px"));
                        if r.changed() {
                            changed = Some(true);
                        }
                        if r.drag_stopped() || r.lost_focus() {
                            self.end_coalescing = true;
                        }
                    }
                    EffectKind::Invert {} => {}
                    EffectKind::Shader { path, inputs } => {
                        ui.weak(path.as_str());
                        let viewer = self.viewer.as_ref();
                        if let Some(err) = viewer.and_then(|v| v.shader_error(path)) {
                            ui.colored_label(ui.visuals().error_fg_color, err);
                        }
                        if let Some(compiled) = viewer.and_then(|v| v.media.shader(path)).cloned() {
                            let edit = shader_input_controls(ui, &compiled, inputs);
                            if edit.changed {
                                changed = Some(edit.dragging);
                            }
                            if edit.finished {
                                self.end_coalescing = true;
                            }
                        }
                    }
                }
            });
        }
        if let Some((a, b)) = swap {
            effects.swap(a, b);
            changed = Some(false);
        }
        if let Some(i) = remove {
            effects.remove(i);
            changed = Some(false);
        }
        match changed {
            Some(true) => self.queue_coalescing(
                Command::SetSurfaceEffects { id, effects },
                format!("effects:{id}"),
            ),
            Some(false) => self.queue(Command::SetSurfaceEffects { id, effects }),
            None => {}
        }
    }

    fn mask_controls(&mut self, ui: &mut egui::Ui, surface: &Surface) {
        let id = surface.id;
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Mask");
            match &surface.mask {
                None => {
                    if ui.small_button("Add").clicked() {
                        let aspect = f64::from(self.session.project().canvas.width)
                            / f64::from(self.session.project().canvas.height.max(1));
                        let mask = Mask::around(&surface.shape.outline(aspect));
                        self.queue(Command::SetSurfaceMask {
                            id,
                            mask: Some(mask),
                        });
                        self.mask_mode = true;
                    }
                }
                Some(_) => {
                    ui.selectable_value(&mut self.mask_mode, false, "Edit shape");
                    ui.selectable_value(&mut self.mask_mode, true, "Edit mask");
                    if ui.small_button("Remove").clicked() {
                        self.queue(Command::SetSurfaceMask { id, mask: None });
                        self.mask_mode = false;
                    }
                }
            }
        });
        let Some(mask) = surface.mask.clone() else {
            return;
        };
        let mut edited = mask.clone();
        let mut feather = mask.feather.get() * 100.0;
        let resp = ui.add(egui::Slider::new(&mut feather, 0.0..=20.0).text("Feather % of height"));
        if resp.changed() {
            edited.feather = om_types::Finite::new(feather / 100.0).unwrap_or(mask.feather);
            self.queue_coalescing(
                Command::SetSurfaceMask {
                    id,
                    mask: Some(edited.clone()),
                },
                format!("feather:{id}"),
            );
        }
        if resp.drag_stopped() || resp.lost_focus() {
            self.end_coalescing = true;
        }
        ui.horizontal(|ui| {
            let mut invert = mask.invert;
            if ui.checkbox(&mut invert, "Invert").changed() {
                self.queue(Command::SetSurfaceMask {
                    id,
                    mask: Some(Mask {
                        invert,
                        ..mask.clone()
                    }),
                });
            }
            let all_smooth = mask.points.iter().all(|p| p.smooth);
            let mut smooth = all_smooth;
            if ui.checkbox(&mut smooth, "Smooth").changed() {
                let mut m = mask.clone();
                for p in &mut m.points {
                    p.smooth = smooth;
                }
                self.queue(Command::SetSurfaceMask { id, mask: Some(m) });
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("+ point").clicked() && mask.points.len() < MAX_MASK_POINTS {
                // Split the longest edge.
                let n = mask.points.len();
                let longest = (0..n)
                    .max_by(|&a, &b| {
                        let len = |i: usize| {
                            let (p, q) = (mask.points[i].p, mask.points[(i + 1) % n].p);
                            (q.x() - p.x()).hypot(q.y() - p.y())
                        };
                        len(a).total_cmp(&len(b))
                    })
                    .unwrap_or(0);
                let (p, q) = (mask.points[longest].p, mask.points[(longest + 1) % n].p);
                let mid =
                    om_geom::Point2::new((p.x() + q.x()) / 2.0, (p.y() + q.y()) / 2.0).unwrap_or(p);
                let mut m = mask.clone();
                m.points.insert(
                    longest + 1,
                    MaskPoint {
                        p: mid,
                        smooth: mask.points[longest].smooth,
                    },
                );
                self.queue(Command::SetSurfaceMask { id, mask: Some(m) });
            }
            if ui
                .add_enabled(mask.points.len() > 3, egui::Button::new("− point").small())
                .clicked()
            {
                let mut m = mask.clone();
                let i = match &self.drag {
                    Some(Drag {
                        kind: canvas::DragKind::MaskPoint(i),
                        ..
                    }) => *i,
                    _ => m.points.len() - 1,
                };
                m.points.remove(i.min(m.points.len() - 1));
                self.queue(Command::SetSurfaceMask { id, mask: Some(m) });
            }
        });
    }

    fn canvas_view(&mut self, ui: &mut egui::Ui) {
        let project_dir = self.session.project_dir().map(PathBuf::from);
        let texture = match &mut self.viewer {
            Some(v) => {
                let effective = self.effective.as_ref().unwrap_or(self.session.project());
                v.update(
                    effective,
                    project_dir.as_deref(),
                    self.transport.time(Instant::now()),
                    &self.transport,
                )
            }
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

        self.draw_fixtures(&painter, rect);

        // Outlines for every surface; handles for the selected one.
        let project = self.session.project();
        for s in &project.surfaces {
            let pts: Vec<Pos2> = s
                .shape
                .corners()
                .iter()
                .map(|c| to_screen(rect, *c))
                .collect();
            let outline = screen_outline(rect, &s.shape);
            let selected = self.selected == Some(s.id);
            let colour = if selected {
                Color32::from_rgb(255, 200, 40)
            } else {
                Color32::from_white_alpha(90)
            };
            let stroke = Stroke::new(if selected { 1.5 } else { 1.0 }, colour);
            painter.add(egui::Shape::closed_line(outline, stroke));
            draw_shape_guides(
                &painter,
                rect,
                &s.shape,
                Stroke::new(1.0, colour.gamma_multiply(0.6)),
            );
            if let Some(m) = &s.mask {
                let aspect = f64::from(rect.width() / rect.height().max(1.0));
                let tol = 1.0 / f64::from(rect.height().max(1.0));
                let path: Vec<Pos2> = m
                    .flatten(tol, aspect)
                    .into_iter()
                    .filter_map(|(x, y)| {
                        om_geom::Point2::new(x, y).ok().map(|p| to_screen(rect, p))
                    })
                    .collect();
                let mask_colour = Color32::from_rgb(80, 200, 255);
                painter.add(egui::Shape::dashed_line(
                    &[path.clone(), path.first().copied().into_iter().collect()].concat(),
                    Stroke::new(1.2, mask_colour),
                    6.0,
                    4.0,
                ));
                if selected && self.mask_mode {
                    for mp in &m.points {
                        let c = to_screen(rect, mp.p);
                        if mp.smooth {
                            painter.circle(
                                c,
                                5.0,
                                Color32::from_black_alpha(160),
                                Stroke::new(1.5, mask_colour),
                            );
                        } else {
                            painter.rect_stroke(
                                egui::Rect::from_center_size(c, egui::vec2(9.0, 9.0)),
                                0.0,
                                Stroke::new(1.5, mask_colour),
                                egui::StrokeKind::Middle,
                            );
                        }
                    }
                }
            }
            if selected && !self.mask_mode {
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
            self.drag = begin_drag(project, rect, self.selected, pos, self.mask_mode);
            if let Some(d) = &self.drag {
                self.selected = Some(d.surface);
            }
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && !self.mask_mode
        {
            self.selected = begin_drag(project, rect, self.selected, pos, false).map(|d| d.surface);
        }
        if let (Some(drag), Some(pos)) = (self.drag.clone(), response.interact_pointer_pos())
            && response.dragged()
            && let canvas::DragKind::MaskPoint(i) = drag.kind
            && let Some(mut mask) = self
                .session
                .project()
                .surface(drag.surface)
                .and_then(|s| s.mask.clone())
            && let Some(p) = canvas::to_canvas(rect, pos)
            && let Some(point) = mask.points.get_mut(i)
            && point.p != p
        {
            point.p = p;
            self.queue_coalescing(
                Command::SetSurfaceMask {
                    id: drag.surface,
                    mask: Some(mask),
                },
                format!("mask:{}", drag.surface),
            );
        }
        if let (Some(drag), Some(pos)) = (self.drag.clone(), response.interact_pointer_pos())
            && response.dragged()
            && let Some(current) = self
                .session
                .project()
                .surface(drag.surface)
                .map(|s| s.shape.clone())
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
            let size = display.native_size();
            let Some(texture) = self
                .viewer
                .as_mut()
                .and_then(|v| v.output_texture(o, size))
                .or(Some(texture))
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

/// Extra guides: mesh grid lines and the inscribed ellipse.
fn draw_shape_guides(painter: &egui::Painter, rect: egui::Rect, shape: &Shape, stroke: Stroke) {
    match shape {
        Shape::Mesh {
            columns,
            rows,
            points,
            ..
        } => {
            let (c, r) = (usize::from(*columns), usize::from(*rows));
            let at = |i: usize, j: usize| points.get(j * (c + 1) + i).map(|p| to_screen(rect, *p));
            for j in 1..r {
                let row: Vec<Pos2> = (0..=c).filter_map(|i| at(i, j)).collect();
                painter.add(egui::Shape::line(row, stroke));
            }
            for i in 1..c {
                let col: Vec<Pos2> = (0..=r).filter_map(|j| at(i, j)).collect();
                painter.add(egui::Shape::line(col, stroke));
            }
        }
        Shape::Ellipse { corners, .. } => {
            if let Ok(h) = om_geom::Homography::square_to_quad(corners) {
                let pts: Vec<Pos2> = (0..64)
                    .filter_map(|k| {
                        let a = f64::from(k) / 64.0 * std::f64::consts::TAU;
                        let (x, y) = h.apply((0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()))?;
                        om_geom::Point2::new(x, y).ok().map(|p| to_screen(rect, p))
                    })
                    .collect();
                painter.add(egui::Shape::closed_line(pts, stroke));
            }
        }
        _ => {}
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
    if ext == "fs" {
        MediaSource::Shader {
            path: stored,
            inputs: Default::default(),
        }
    } else if VIDEO.contains(&ext.as_str()) {
        MediaSource::Video { path: stored }
    } else {
        MediaSource::Image { path: stored }
    }
}

/// Result of editing shader inputs.
#[derive(Default)]
struct ShaderEdit {
    changed: bool,
    /// A slider/drag is in progress (coalesce into one undo step).
    dragging: bool,
    finished: bool,
}

/// Widgets for a shader's declared inputs (ISF header). Values not set
/// explicitly show the shader's DEFAULT.
fn shader_input_controls(
    ui: &mut egui::Ui,
    compiled: &om_isf::Compiled,
    values: &mut std::collections::BTreeMap<String, om_project::ShaderValue>,
) -> ShaderEdit {
    use om_isf::InputKind;
    use om_project::ShaderValue;
    let mut edit = ShaderEdit::default();
    let num =
        |v: Option<&serde_json::Value>, d: f64| v.and_then(serde_json::Value::as_f64).unwrap_or(d);
    let vec_of = |v: Option<&serde_json::Value>, n: usize, d: f64| -> Vec<f64> {
        let mut out: Vec<f64> = v
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
            .unwrap_or_default();
        out.resize(n, d);
        out
    };
    let fin = |x: f64| om_types::Finite::new(x).unwrap_or(om_types::Finite::ZERO);
    for input in &compiled.doc.inputs {
        let label = input.label.clone().unwrap_or_else(|| input.name.clone());
        let current = values.get(&input.name).cloned();
        match input.kind {
            InputKind::Image | InputKind::Audio | InputKind::AudioFft => {}
            InputKind::Float => {
                let mut x = match current {
                    Some(ShaderValue::Number(n)) => n.get(),
                    _ => num(input.default.as_ref(), 0.0),
                };
                let (lo, hi) = (num(input.min.as_ref(), 0.0), num(input.max.as_ref(), 1.0));
                let r = ui.add(egui::Slider::new(&mut x, lo.min(hi)..=hi.max(lo)).text(&label));
                if r.changed() {
                    values.insert(input.name.clone(), ShaderValue::Number(fin(x)));
                    edit.changed = true;
                    edit.dragging = true;
                }
                edit.finished |= r.drag_stopped() || r.lost_focus();
            }
            InputKind::Bool | InputKind::Event => {
                let mut b = match current {
                    Some(ShaderValue::Bool(b)) => b,
                    _ => input
                        .default
                        .as_ref()
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                };
                if ui.checkbox(&mut b, &label).changed() {
                    values.insert(input.name.clone(), ShaderValue::Bool(b));
                    edit.changed = true;
                }
            }
            InputKind::Long => {
                let mut x = match current {
                    Some(ShaderValue::Number(n)) => n.get().round() as i64,
                    _ => num(input.default.as_ref(), 0.0).round() as i64,
                };
                let before = x;
                if input.values.is_empty() {
                    let (lo, hi) = (
                        num(input.min.as_ref(), 0.0) as i64,
                        num(input.max.as_ref(), 10.0) as i64,
                    );
                    ui.add(egui::Slider::new(&mut x, lo.min(hi)..=hi.max(lo)).text(&label));
                } else {
                    let name_of = |v: i64| {
                        input
                            .values
                            .iter()
                            .position(|x| *x == v)
                            .and_then(|i| input.labels.get(i).cloned())
                            .unwrap_or_else(|| v.to_string())
                    };
                    egui::ComboBox::from_label(&label)
                        .selected_text(name_of(x))
                        .show_ui(ui, |ui| {
                            for v in &input.values {
                                ui.selectable_value(&mut x, *v, name_of(*v));
                            }
                        });
                }
                if x != before {
                    #[allow(clippy::cast_precision_loss)]
                    values.insert(input.name.clone(), ShaderValue::Number(fin(x as f64)));
                    edit.changed = true;
                }
            }
            InputKind::Point2D => {
                let mut v = match current {
                    Some(ShaderValue::Vector(v)) => v.iter().map(|f| f.get()).collect(),
                    _ => vec_of(input.default.as_ref(), 2, 0.0),
                };
                v.resize(2, 0.0);
                let mut moved = false;
                ui.horizontal(|ui| {
                    ui.label(&label);
                    for x in &mut v {
                        let r = ui.add(egui::DragValue::new(x).speed(0.01));
                        moved |= r.changed();
                        edit.finished |= r.drag_stopped() || r.lost_focus();
                    }
                });
                if moved {
                    edit.changed = true;
                    edit.dragging = true;
                    values.insert(
                        input.name.clone(),
                        ShaderValue::Vector(v.into_iter().map(fin).collect()),
                    );
                }
            }
            InputKind::Color => {
                let v = match current {
                    Some(ShaderValue::Vector(v)) => v.iter().map(|f| f.get()).collect(),
                    _ => vec_of(input.default.as_ref(), 4, 1.0),
                };
                #[allow(clippy::cast_possible_truncation)]
                let mut rgba = [
                    v[0] as f32,
                    v[1] as f32,
                    v[2] as f32,
                    v.get(3).copied().unwrap_or(1.0) as f32,
                ];
                ui.horizontal(|ui| {
                    ui.label(&label);
                    if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                        values.insert(
                            input.name.clone(),
                            ShaderValue::Vector(rgba.iter().map(|c| fin(f64::from(*c))).collect()),
                        );
                        edit.changed = true;
                        edit.dragging = true;
                    }
                });
            }
        }
    }
    edit
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
        // Live show: external control, cues, timelines, modulators.
        let now = Instant::now();
        if let Some(v) = &self.viewer {
            let l = v.audio_levels();
            self.live.audio = om_show::AudioLevels {
                level: l.level,
                low: l.low,
                mid: l.mid,
                high: l.high,
            };
        }
        self.effective = Some(self.live.frame(&mut self.session, &mut self.transport, now));
        if self.live.animating(self.session.project()) {
            ctx.request_repaint();
        }
        self.handle_shortcuts(&ctx);
        egui::Panel::top("menu").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("show")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.show_panel(ui));
        egui::Panel::left("project")
            .default_size(260.0)
            .show(ui, |ui| self.left_panel(ui));
        egui::Panel::right("inspector")
            .default_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
            });
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
