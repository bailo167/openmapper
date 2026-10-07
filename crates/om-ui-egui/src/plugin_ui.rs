// SPDX-License-Identifier: Apache-2.0
//! Plugin filters on a media item: list, enable, parameters and status.

use eframe::egui;
use om_command::Command;
use om_project::{Media, PluginUse};
use om_types::Finite;

use crate::OpenMapperApp;

impl OpenMapperApp {
    pub(crate) fn media_plugin_controls(&mut self, ui: &mut egui::Ui, m: &Media) {
        let mut plugins = m.plugins.clone();
        let mut changed = false;
        let mut dragging = false;
        let dir = self.session.project_dir().map(std::path::PathBuf::from);
        egui::CollapsingHeader::new(format!("Plugins ({})", plugins.len()))
            .id_salt(("plugins", m.id))
            .show(ui, |ui| {
                let status = self
                    .viewer
                    .as_ref()
                    .map(|v| v.plugins.status(m.id))
                    .unwrap_or_default();
                for s in &status {
                    let colour = if s.ok {
                        ui.visuals().weak_text_color()
                    } else {
                        ui.visuals().error_fg_color
                    };
                    ui.colored_label(colour, format!("{}: {}", s.name, s.state));
                }
                let mut remove = None;
                for (k, p) in plugins.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut p.enabled, "").changed();
                        let mut path = p.path.clone();
                        if ui
                            .add(egui::TextEdit::singleline(&mut path).desired_width(140.0))
                            .lost_focus()
                            && !path.trim().is_empty()
                            && path != p.path
                        {
                            p.path = path.trim().to_owned();
                            changed = true;
                        }
                        if ui.small_button("×").clicked() {
                            remove = Some(k);
                        }
                    });
                    let resolved = om_engine::resolve_media_path(dir.as_deref(), &p.path);
                    let manifest = self
                        .viewer
                        .as_ref()
                        .and_then(|v| v.plugins.manifest(&resolved))
                        .cloned();
                    if let Some(manifest) = manifest {
                        for decl in &manifest.params {
                            ui.horizontal(|ui| {
                                ui.add_space(14.0);
                                let mut v = p
                                    .params
                                    .get(&decl.name)
                                    .map_or(f64::from(decl.default), |f| f.get());
                                let r = ui.add(
                                    egui::Slider::new(
                                        &mut v,
                                        f64::from(decl.min)..=f64::from(decl.max),
                                    )
                                    .text(&decl.name),
                                );
                                if r.changed()
                                    && let Ok(f) = Finite::new(v)
                                {
                                    p.params.insert(decl.name.clone(), f);
                                    changed = true;
                                    dragging |= r.dragged();
                                }
                            });
                        }
                    }
                }
                if let Some(k) = remove {
                    plugins.remove(k);
                    changed = true;
                }
                if ui
                    .small_button("+ Plugin")
                    .on_hover_text(
                        "A .wasm plugin file (sandboxed: no files, network or environment)",
                    )
                    .clicked()
                    && plugins.len() < om_project::MAX_PLUGINS
                {
                    plugins.push(PluginUse {
                        path: "plugin.wasm".into(),
                        enabled: true,
                        params: Default::default(),
                    });
                    changed = true;
                }
                if ui.small_button("Reload plugins").clicked()
                    && let Some(v) = &mut self.viewer
                {
                    v.plugins.reload();
                }
            });
        if changed {
            let cmd = Command::SetMediaPlugins { id: m.id, plugins };
            if dragging {
                self.queue_coalescing(cmd, format!("plugins:{}", m.id));
            } else {
                self.queue(cmd);
            }
        }
    }
}
