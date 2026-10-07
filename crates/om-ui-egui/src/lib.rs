// SPDX-License-Identifier: Apache-2.0
//! Desktop UI. Presentation only: every change the user makes is turned into
//! an `om_command::Command` and sent through the [`Session`]; this crate never
//! mutates the project directly.

use std::path::PathBuf;

use eframe::egui;
use om_command::Command;
use om_engine::{OpenReport, Session};
use om_project::Surface;
use om_types::{SurfaceId, UnitInterval};

/// Top-level application state.
#[derive(Debug)]
pub struct OpenMapperApp {
    session: Session,
    selected: Option<SurfaceId>,
    /// Text buffer for the "Save As" / "Open" path field.
    path_input: String,
    status: Status,
    /// Name being edited, committed on focus loss/enter.
    rename_buffer: Option<(SurfaceId, String)>,
    project_name_buffer: Option<String>,
}

#[derive(Debug, Default)]
struct Status {
    message: String,
    is_error: bool,
}

impl OpenMapperApp {
    /// Starts with an empty project, or opens `path` if given. A failed open
    /// falls back to an empty project and shows the error.
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            session: Session::new("Untitled"),
            selected: None,
            path_input: String::from("untitled.omproj"),
            status: Status::default(),
            rename_buffer: None,
            project_name_buffer: None,
        };
        if let Some(p) = path {
            app.path_input = p.display().to_string();
            app.open(p);
        }
        app
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

    fn run(&mut self, command: Command) {
        if let Err(e) = self.session.execute(command) {
            self.error(e.to_string());
        }
    }

    fn open(&mut self, path: PathBuf) {
        match Session::open(&path) {
            Ok((session, report)) => {
                self.session = session;
                self.selected = None;
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

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        // Check the longer shortcut first so Cmd+Shift+Z is not eaten by Cmd+Z.
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
            ui.label("Path:");
            ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(280.0));
        });
    }

    fn surface_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Project");
        let current = self.session.project().name.clone();
        let buffer = self.project_name_buffer.get_or_insert(current.clone());
        let resp = ui.text_edit_singleline(buffer);
        if resp.lost_focus()
            && let Some(name) = self.project_name_buffer.take()
            && name != current
        {
            self.run(Command::SetProjectName { name });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Surfaces");
            if ui.button("+ Add").clicked() {
                let n = self.session.project().surfaces.len() + 1;
                let id = SurfaceId::new();
                self.run(Command::AddSurface {
                    surface: Surface::new(id, format!("Surface {n}")),
                    index: None,
                });
                self.selected = Some(id);
            }
        });
        let surfaces: Vec<(SurfaceId, String, bool)> = self
            .session
            .project()
            .surfaces
            .iter()
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

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.weak("Select a surface to edit it.");
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
        let resp = ui.text_edit_singleline(buffer);
        if resp.lost_focus()
            && let Some((_, name)) = self.rename_buffer.take()
            && name != surface.name
        {
            self.run(Command::UpdateSurface {
                id,
                name: Some(name),
                enabled: None,
                opacity: None,
            });
        }

        let mut enabled = surface.enabled;
        if ui.checkbox(&mut enabled, "Enabled").changed() {
            self.run(Command::UpdateSurface {
                id,
                name: None,
                enabled: Some(enabled),
                opacity: None,
            });
        }

        let mut opacity = surface.opacity.get();
        let resp = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity"));
        if resp.changed() {
            let cmd = Command::UpdateSurface {
                id,
                name: None,
                enabled: None,
                opacity: Some(UnitInterval::saturating(opacity)),
            };
            if let Err(e) = self
                .session
                .execute_coalescing(cmd, format!("opacity:{id}"))
            {
                self.error(e.to_string());
            }
        }
        if resp.drag_stopped() || resp.lost_focus() {
            self.session.break_coalescing();
        }

        ui.add_space(12.0);
        if ui.button("Remove surface").clicked() {
            self.run(Command::RemoveSurface { id });
            self.selected = None;
        }
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
        self.handle_shortcuts(&ui.ctx().clone());
        egui::Panel::top("menu").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("surfaces")
            .default_size(220.0)
            .show(ui, |ui| self.surface_list(ui));
        egui::Panel::right("inspector")
            .default_size(260.0)
            .show(ui, |ui| self.inspector(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.weak("Mapping canvas arrives with the renderer milestone.");
            });
        });
    }
}
