// SPDX-License-Identifier: Apache-2.0
//! Live video UI: adding cameras, streams and senders as media, and
//! publishing outputs (Syphon, Spout, NDI, network streams).

use eframe::egui;
use om_command::Command;
use om_engine::Discovery;
use om_media_core::SinkState;
use om_project::{LiveInput, Media, MediaSource, Output, Publish, StreamCodec};
use om_types::{MediaId, OutputId};

use crate::OpenMapperApp;

/// Text fields and discovery for the live UI.
#[derive(Debug, Default)]
pub struct LiveUi {
    pub discovery: Option<Discovery>,
    stream_url: String,
    publish_url: String,
    publish_lossless: bool,
    /// Output whose publish name is being edited, and the text.
    renaming: Option<(OutputId, usize, String)>,
}

impl LiveUi {
    #[must_use]
    pub fn new(discovery: Option<Discovery>) -> Self {
        Self {
            discovery,
            ..Self::default()
        }
    }
}

fn add_live(app: &mut OpenMapperApp, input: LiveInput) {
    let name = match &input {
        LiveInput::Camera { device } => device.clone(),
        LiveInput::Stream { url } => url.clone(),
        LiveInput::Ndi { source } => source.clone(),
        LiveInput::Syphon { server, app } if server.is_empty() => app.clone(),
        LiveInput::Syphon { server, .. } => server.clone(),
        LiveInput::Spout { sender } => sender.clone(),
    };
    app.queue(Command::AddMedia {
        media: Media {
            id: MediaId::new(),
            name,
            source: MediaSource::Live { input },
            playback: Default::default(),
            extensions: Default::default(),
        },
        index: None,
    });
}

fn state_colour(ui: &egui::Ui, state: &SinkState) -> egui::Color32 {
    match state {
        SinkState::Sending => egui::Color32::from_rgb(80, 190, 90),
        SinkState::Connecting => ui.visuals().warn_fg_color,
        SinkState::Retrying { .. } => ui.visuals().error_fg_color,
    }
}

impl OpenMapperApp {
    /// "+ Live input" menu and stream URL field (in the media panel).
    pub(crate) fn live_input_controls(&mut self, ui: &mut egui::Ui) {
        if self.live_ui.discovery.is_none() {
            return;
        }
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.menu_button("+ Live input", |ui| {
                let Some(d) = &self.live_ui.discovery else {
                    return;
                };
                let inputs = d.inputs();
                if inputs.is_empty() {
                    ui.weak(if d.scanned() {
                        "No cameras or senders found."
                    } else {
                        "Searching…"
                    });
                }
                for input in inputs {
                    if ui.button(input.label()).clicked() {
                        chosen = Some(input);
                        ui.close();
                    }
                }
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.live_ui.stream_url)
                    .hint_text("srt://host:9000, udp://@:5000, rtsp://…")
                    .desired_width(150.0),
            );
            let url = self.live_ui.stream_url.trim().to_owned();
            if ui
                .add_enabled(!url.is_empty(), egui::Button::new("Add stream"))
                .clicked()
            {
                match om_project::validate_stream_url(&url) {
                    Ok(()) => {
                        chosen = Some(LiveInput::Stream { url });
                        self.live_ui.stream_url.clear();
                    }
                    Err(e) => self.error(e),
                }
            }
        });
        if let Some(input) = chosen {
            add_live(self, input);
        }
    }

    /// Publish targets of one output (in the outputs panel).
    pub(crate) fn publish_controls(&mut self, ui: &mut egui::Ui, o: &Output) {
        let Some(viewer) = &self.viewer else { return };
        let mut publish = o.publish.clone();
        let mut changed = false;
        let mut remove = None;
        for (k, target) in o.publish.iter().enumerate() {
            let status = viewer.publish.status(o.id, target);
            ui.horizontal(|ui| {
                match &status {
                    Some(s) => {
                        let text = match &s.state {
                            SinkState::Sending => "●".to_owned(),
                            SinkState::Connecting => "◌".to_owned(),
                            SinkState::Retrying { .. } => "▲".to_owned(),
                        };
                        let hover = match &s.state {
                            SinkState::Retrying { error } => error.clone(),
                            SinkState::Connecting => "waiting for a receiver…".to_owned(),
                            SinkState::Sending => {
                                format!("{} — {} frames sent", s.stats.description, s.stats.sent)
                            }
                        };
                        ui.colored_label(state_colour(ui, &s.state), text)
                            .on_hover_text(hover);
                    }
                    None => {
                        ui.colored_label(ui.visuals().error_fg_color, "▲")
                            .on_hover_text("not available on this platform or build");
                    }
                }
                let editing =
                    matches!(&self.live_ui.renaming, Some((id, i, _)) if *id == o.id && *i == k);
                if editing {
                    if let Some((_, _, text)) = &mut self.live_ui.renaming {
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(140.0));
                        if r.lost_focus() {
                            let text = text.trim().to_owned();
                            if let Some(t) = publish.get_mut(k) {
                                match t {
                                    Publish::Syphon { name }
                                    | Publish::Spout { name }
                                    | Publish::Ndi { name } => *name = text,
                                    Publish::Stream { url, .. } => *url = text,
                                }
                                changed = true;
                            }
                            self.live_ui.renaming = None;
                        }
                    }
                } else if ui.label(target.label()).double_clicked() {
                    let text = match target {
                        Publish::Syphon { name }
                        | Publish::Spout { name }
                        | Publish::Ndi { name } => name.clone(),
                        Publish::Stream { url, .. } => url.clone(),
                    };
                    self.live_ui.renaming = Some((o.id, k, text));
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Stop publishing")
                    .clicked()
                {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            publish.remove(k);
            changed = true;
        }
        let candidates = [
            (
                "+ Syphon",
                Publish::Syphon {
                    name: o.name.clone(),
                },
            ),
            (
                "+ Spout",
                Publish::Spout {
                    name: o.name.clone(),
                },
            ),
            (
                "+ NDI",
                Publish::Ndi {
                    name: o.name.clone(),
                },
            ),
        ];
        ui.horizontal(|ui| {
            for (label, target) in candidates {
                if viewer.publish.supports(&target)
                    && !publish.contains(&target)
                    && ui.small_button(label).clicked()
                {
                    publish.push(target);
                    changed = true;
                }
            }
        });
        let stream_probe = Publish::Stream {
            url: String::new(),
            codec: StreamCodec::Compatible,
            fps: 30,
        };
        if viewer.publish.supports(&stream_probe) {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.live_ui.publish_url)
                        .hint_text("srt://host:9000 or udp://host:5000")
                        .desired_width(150.0),
                );
                ui.checkbox(&mut self.live_ui.publish_lossless, "lossless")
                    .on_hover_text("FFV1 in Matroska: bit-exact, needs tcp:// or srt://");
                let url = self.live_ui.publish_url.trim().to_owned();
                if ui
                    .add_enabled(!url.is_empty(), egui::Button::new("+ Stream"))
                    .clicked()
                {
                    let target = Publish::Stream {
                        url,
                        codec: if self.live_ui.publish_lossless {
                            StreamCodec::Lossless
                        } else {
                            StreamCodec::Compatible
                        },
                        fps: 30,
                    };
                    match target.validate() {
                        Ok(()) => {
                            publish.push(target);
                            changed = true;
                            self.live_ui.publish_url.clear();
                        }
                        Err(e) => self.error(e),
                    }
                }
            });
        }
        if changed {
            self.queue(Command::SetOutputPublish { id: o.id, publish });
        }
    }
}
