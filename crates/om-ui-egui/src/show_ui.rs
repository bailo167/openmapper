// SPDX-License-Identifier: Apache-2.0
//! Show-control UI: master, cues, timelines, modulators, external control.

use std::time::Instant;

use eframe::egui;
use om_command::{Command, params};
use om_project::{
    AudioBand, Cue, CueValue, Ease, Keyframe, LfoShape, MidiTarget, ModSource, Modulator, ParamId,
    ParamKind, ParamValue, Timeline, Track,
};
use om_show::control::{Action, ControlMessage};
use om_time::RationalTime;
use om_types::{CueId, Finite, ModulatorId, TimelineId, UnitInterval};

use crate::OpenMapperApp;

/// Tabs of the show panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowTab {
    Cues,
    Timelines,
    Modulators,
    Control,
}

fn fin(v: f64) -> Finite {
    Finite::new(v).unwrap_or(Finite::ZERO)
}

fn secs(v: f64) -> RationalTime {
    #[allow(clippy::cast_possible_truncation)]
    RationalTime::new((v.max(0.0) * 1000.0).round() as i128, 1000).unwrap_or(RationalTime::ZERO)
}

/// A combo box choosing one of the project's parameters.
fn param_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    project: &om_project::Project,
    current: &mut ParamId,
) -> bool {
    let infos = params::list(project);
    let label = infos
        .iter()
        .find(|p| &p.id == current)
        .map_or_else(|| current.to_string(), |p| p.label.clone());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .width(200.0)
        .show_ui(ui, |ui| {
            for p in infos {
                if ui.selectable_label(&p.id == current, &p.label).clicked() {
                    *current = p.id;
                    changed = true;
                }
            }
        });
    changed
}

/// Editor for a parameter value of the given kind. Returns true if edited.
fn value_editor(ui: &mut egui::Ui, kind: Option<ParamKind>, value: &mut ParamValue) -> bool {
    match kind {
        Some(ParamKind::Bool) => {
            let mut b = value.as_bool();
            let changed = ui.checkbox(&mut b, "on").changed();
            *value = ParamValue::Bool(b);
            changed
        }
        Some(ParamKind::Float { min, max }) => {
            let mut v = value.as_f64();
            let (lo, hi) = (min.max(-1000.0), max.min(1000.0));
            let changed = ui.add(egui::Slider::new(&mut v, lo..=hi)).changed();
            *value = ParamValue::Float(v);
            changed
        }
        None => {
            ui.weak("(target removed)");
            false
        }
    }
}

impl OpenMapperApp {
    pub(crate) fn master_controls(&mut self, ui: &mut egui::Ui) {
        let master = self.session.project().master;
        let mut opacity = master.opacity.get();
        let r = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Master"));
        if r.changed() {
            self.queue_coalescing(
                Command::SetMaster {
                    master: om_project::Master {
                        opacity: UnitInterval::saturating(opacity),
                        ..master
                    },
                },
                "master".into(),
            );
        }
        if r.drag_stopped() || r.lost_focus() {
            self.end_coalescing = true;
        }
        let label = if master.blackout {
            "BLACKOUT"
        } else {
            "Blackout"
        };
        let button = egui::Button::new(label).selected(master.blackout);
        if ui.add(button).clicked() {
            self.queue(Command::SetMaster {
                master: om_project::Master {
                    blackout: !master.blackout,
                    ..master
                },
            });
        }
    }

    fn run(&mut self, action: Action) {
        let msg = ControlMessage::Action(action);
        if let Err(e) =
            self.live
                .control(&msg, &mut self.session, &mut self.transport, Instant::now())
        {
            self.error(e);
        }
    }

    pub(crate) fn show_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (ShowTab::Cues, "Cues"),
                (ShowTab::Timelines, "Timelines"),
                (ShowTab::Modulators, "Modulators"),
                (ShowTab::Control, "OSC / MIDI / DMX"),
            ] {
                ui.selectable_value(&mut self.show_tab, tab, label);
            }
            ui.separator();
            if let Some(target) = self.live.mapper.learning() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("MIDI learn: move a control for {target:?}"),
                );
                if ui.small_button("Cancel").clicked() {
                    self.live.mapper.cancel_learn();
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| match self.show_tab {
                ShowTab::Cues => self.cues_tab(ui),
                ShowTab::Timelines => self.timelines_tab(ui),
                ShowTab::Modulators => self.modulators_tab(ui),
                ShowTab::Control => self.control_tab(ui),
            });
    }

    fn cues_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("GO ▶").clicked() {
                self.run(Action::CueGoNext);
            }
            if ui.button("Release").clicked() {
                self.run(Action::CueRelease { fade: 1.0 });
            }
            if ui.small_button("MIDI learn GO").clicked() {
                self.live.mapper.learn(MidiTarget::CueGo);
            }
            if ui.button("+ Cue").clicked() {
                let n = self.session.project().show.cues.len() + 1;
                self.queue(Command::PutCue {
                    cue: Cue {
                        id: CueId::new(),
                        name: format!("Cue {n}"),
                        fade: fin(1.0),
                        values: Vec::new(),
                    },
                    index: None,
                });
            }
        });
        let current = self.live.show.current_cue();
        let project = self.session.project().clone();
        for cue in &project.show.cues {
            let mut edited = cue.clone();
            let mut changed = false;
            ui.push_id(cue.id, |ui| {
                ui.horizontal(|ui| {
                    let marker = if current == Some(cue.id) { "▶" } else { " " };
                    ui.monospace(marker);
                    if ui.button("GO").clicked() {
                        self.run(Action::CueGo(cue.id));
                    }
                    changed |= ui.text_edit_singleline(&mut edited.name).lost_focus();
                    let mut fade = edited.fade.get();
                    if ui
                        .add(
                            egui::DragValue::new(&mut fade)
                                .range(0.0..=600.0)
                                .suffix(" s fade")
                                .speed(0.05),
                        )
                        .changed()
                    {
                        edited.fade = fin(fade);
                        changed = true;
                    }
                    if ui.small_button("+ value").clicked() {
                        edited.values.push(CueValue {
                            param: ParamId::MasterOpacity,
                            value: ParamValue::Float(1.0),
                        });
                        changed = true;
                    }
                    if ui.small_button("MIDI learn").clicked() {
                        self.live.mapper.learn(MidiTarget::Cue { cue: cue.id });
                    }
                    if ui.small_button("Remove").clicked() {
                        self.queue(Command::RemoveCue { id: cue.id });
                    }
                });
                let mut remove = None;
                for (i, v) in edited.values.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        changed |= param_picker(ui, (cue.id, i), &project, &mut v.param);
                        changed |= value_editor(ui, params::kind(&project, &v.param), &mut v.value);
                        if ui.small_button("x").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    edited.values.remove(i);
                    changed = true;
                }
            });
            if changed && edited != *cue {
                self.queue(Command::PutCue {
                    cue: edited,
                    index: None,
                });
            }
        }
    }

    fn timelines_tab(&mut self, ui: &mut egui::Ui) {
        if ui.button("+ Timeline").clicked() {
            let n = self.session.project().show.timelines.len() + 1;
            self.queue(Command::PutTimeline {
                timeline: Timeline {
                    id: TimelineId::new(),
                    name: format!("Timeline {n}"),
                    duration: RationalTime::from_seconds(10),
                    looping: true,
                    tracks: Vec::new(),
                    markers: Vec::new(),
                },
            });
        }
        let now = Instant::now();
        let clock = self.live.clock(now);
        let project = self.session.project().clone();
        for tl in &project.show.timelines {
            let mut edited = tl.clone();
            let mut changed = false;
            let position = self.live.show.timeline_position(tl, clock);
            ui.push_id(tl.id, |ui| {
                ui.horizontal(|ui| {
                    let playing = self.live.show.timeline_playing(tl.id);
                    if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
                        self.run(if playing {
                            Action::TimelinePause(tl.id)
                        } else {
                            Action::TimelinePlay(tl.id)
                        });
                    }
                    if ui.button("Stop").clicked() {
                        self.run(Action::TimelineStop(tl.id));
                    }
                    changed |= ui.text_edit_singleline(&mut edited.name).lost_focus();
                    let mut d = edited.duration.as_seconds_f64();
                    if ui
                        .add(
                            egui::DragValue::new(&mut d)
                                .range(0.1..=36_000.0)
                                .suffix(" s")
                                .speed(0.1),
                        )
                        .changed()
                    {
                        edited.duration = secs(d);
                        changed = true;
                    }
                    changed |= ui.checkbox(&mut edited.looping, "Loop").changed();
                    if ui.small_button("+ track").clicked() {
                        edited.tracks.push(Track {
                            param: ParamId::MasterOpacity,
                            keys: Vec::new(),
                        });
                        changed = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        self.queue(Command::RemoveTimeline { id: tl.id });
                    }
                });
                // Scrub bar.
                let mut pos = position.map_or(0.0, RationalTime::as_seconds_f64);
                let r = ui.add(
                    egui::Slider::new(&mut pos, 0.0..=tl.duration.as_seconds_f64())
                        .text("position s")
                        .show_value(true),
                );
                if r.changed() {
                    self.run(Action::TimelineSeek(tl.id, pos));
                }
                for (ti, track) in edited.tracks.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        changed |= param_picker(ui, (tl.id, ti), &project, &mut track.param);
                        // Record a key at the playhead with the current value.
                        if ui.small_button("● key").clicked() {
                            let at = position.unwrap_or(RationalTime::ZERO);
                            let value = self
                                .live
                                .overrides
                                .get(&track.param)
                                .copied()
                                .or_else(|| params::get(&project, &track.param))
                                .unwrap_or(ParamValue::Float(0.0));
                            track.keys.retain(|k| k.at != at);
                            track.keys.push(Keyframe {
                                at,
                                value,
                                ease: Ease::Linear,
                            });
                            track.keys.sort_by(|a, b| {
                                a.at.as_seconds_f64().total_cmp(&b.at.as_seconds_f64())
                            });
                            changed = true;
                        }
                        ui.weak(format!("{} keys", track.keys.len()));
                    });
                    let kind = params::kind(&project, &track.param);
                    let mut remove = None;
                    for (ki, key) in track.keys.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(48.0);
                            let mut at = key.at.as_seconds_f64();
                            if ui
                                .add(
                                    egui::DragValue::new(&mut at)
                                        .range(0.0..=36_000.0)
                                        .suffix(" s")
                                        .speed(0.01),
                                )
                                .changed()
                            {
                                key.at = secs(at);
                                changed = true;
                            }
                            changed |= value_editor(ui, kind, &mut key.value);
                            egui::ComboBox::from_id_salt((tl.id, ti, ki))
                                .selected_text(format!("{:?}", key.ease))
                                .show_ui(ui, |ui| {
                                    for e in [Ease::Linear, Ease::Step, Ease::Smooth] {
                                        changed |= ui
                                            .selectable_value(&mut key.ease, e, format!("{e:?}"))
                                            .changed();
                                    }
                                });
                            if ui.small_button("x").clicked() {
                                remove = Some(ki);
                            }
                        });
                    }
                    if let Some(k) = remove {
                        track.keys.remove(k);
                        changed = true;
                    }
                }
            });
            if changed && edited != *tl {
                self.queue(Command::PutTimeline { timeline: edited });
            }
        }
    }

    fn modulators_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("+ LFO").clicked() {
                self.queue(Command::PutModulator {
                    modulator: Modulator {
                        id: ModulatorId::new(),
                        name: "LFO".into(),
                        enabled: true,
                        param: ParamId::MasterOpacity,
                        source: ModSource::Lfo {
                            shape: LfoShape::Sine,
                            rate: fin(0.5),
                            phase: Finite::ZERO,
                        },
                        depth: Finite::ONE,
                        offset: Finite::ZERO,
                    },
                });
            }
            if ui.button("+ Audio").clicked() {
                self.queue(Command::PutModulator {
                    modulator: Modulator {
                        id: ModulatorId::new(),
                        name: "Audio".into(),
                        enabled: true,
                        param: ParamId::MasterOpacity,
                        source: ModSource::Audio {
                            band: AudioBand::Low,
                            gain: Finite::ONE,
                        },
                        depth: Finite::ONE,
                        offset: Finite::ZERO,
                    },
                });
            }
            let a = self.live.audio;
            ui.weak(format!(
                "audio  level {:.2}  low {:.2}  mid {:.2}  high {:.2}",
                a.level, a.low, a.mid, a.high
            ));
        });
        let project = self.session.project().clone();
        for m in &project.show.modulators {
            let mut e = m.clone();
            let mut changed = false;
            let mut drag = false;
            ui.push_id(m.id, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut e.enabled, "").changed();
                    changed |= ui.text_edit_singleline(&mut e.name).lost_focus();
                    changed |= param_picker(ui, m.id, &project, &mut e.param);
                    match &mut e.source {
                        ModSource::Lfo { shape, rate, phase } => {
                            egui::ComboBox::from_id_salt((m.id, "shape"))
                                .selected_text(format!("{shape:?}"))
                                .show_ui(ui, |ui| {
                                    for s in [
                                        LfoShape::Sine,
                                        LfoShape::Triangle,
                                        LfoShape::Square,
                                        LfoShape::Saw,
                                    ] {
                                        changed |= ui
                                            .selectable_value(shape, s, format!("{s:?}"))
                                            .changed();
                                    }
                                });
                            let mut r = rate.get();
                            if ui
                                .add(
                                    egui::DragValue::new(&mut r)
                                        .range(0.0..=50.0)
                                        .suffix(" Hz")
                                        .speed(0.01),
                                )
                                .changed()
                            {
                                *rate = fin(r);
                                drag = true;
                            }
                            let mut p = phase.get();
                            if ui
                                .add(
                                    egui::DragValue::new(&mut p)
                                        .range(0.0..=1.0)
                                        .prefix("phase ")
                                        .speed(0.01),
                                )
                                .changed()
                            {
                                *phase = fin(p);
                                drag = true;
                            }
                        }
                        ModSource::Audio { band, gain } => {
                            egui::ComboBox::from_id_salt((m.id, "band"))
                                .selected_text(format!("{band:?}"))
                                .show_ui(ui, |ui| {
                                    for b in [
                                        AudioBand::Level,
                                        AudioBand::Low,
                                        AudioBand::Mid,
                                        AudioBand::High,
                                    ] {
                                        changed |= ui
                                            .selectable_value(band, b, format!("{b:?}"))
                                            .changed();
                                    }
                                });
                            let mut g = gain.get();
                            if ui
                                .add(
                                    egui::DragValue::new(&mut g)
                                        .range(0.0..=20.0)
                                        .prefix("gain ")
                                        .speed(0.05),
                                )
                                .changed()
                            {
                                *gain = fin(g);
                                drag = true;
                            }
                        }
                    }
                    let mut d = e.depth.get();
                    if ui
                        .add(egui::DragValue::new(&mut d).prefix("depth ").speed(0.01))
                        .changed()
                    {
                        e.depth = fin(d);
                        drag = true;
                    }
                    let mut o = e.offset.get();
                    if ui
                        .add(egui::DragValue::new(&mut o).prefix("offset ").speed(0.01))
                        .changed()
                    {
                        e.offset = fin(o);
                        drag = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        self.queue(Command::RemoveModulator { id: m.id });
                    }
                });
            });
            if (changed || drag) && e != *m {
                let cmd = Command::PutModulator { modulator: e };
                if drag {
                    self.queue_coalescing(cmd, format!("modulator:{}", m.id));
                } else {
                    self.queue(cmd);
                }
            }
        }
    }

    fn control_tab(&mut self, ui: &mut egui::Ui) {
        let controls = self.session.project().controls.clone();
        ui.horizontal(|ui| {
            let mut osc = controls.osc_port;
            let mut query = controls.oscquery_port;
            ui.label("OSC port");
            let a = ui.add(egui::DragValue::new(&mut osc).range(0..=65535));
            ui.label("OSCQuery port");
            let b = ui.add(egui::DragValue::new(&mut query).range(0..=65535));
            if (a.lost_focus() || b.lost_focus() || a.drag_stopped() || b.drag_stopped())
                && (osc, query) != (controls.osc_port, controls.oscquery_port)
            {
                self.queue(Command::SetControls {
                    controls: om_project::Controls {
                        osc_port: osc,
                        oscquery_port: query,
                        ..controls.clone()
                    },
                });
            }
            ui.weak("(0 = off)");
        });
        let mut network = controls.network;
        if ui
            .checkbox(&mut network, "Accept control from other computers")
            .on_hover_text(
                "Listen on every network interface and advertise OSCQuery over mDNS. \
                 Anyone on the network can then control the show: use trusted networks only.",
            )
            .changed()
        {
            self.queue(Command::SetControls {
                controls: om_project::Controls {
                    network,
                    ..controls.clone()
                },
            });
        }
        ui.label(&self.live.servers.status);
        if let Some(e) = self.live.servers.errors.last() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("last control error: {e}"),
            );
        }
        ui.separator();
        let ports = self.live.midi.ports();
        ui.label(if ports.is_empty() {
            "MIDI: no input ports".to_owned()
        } else {
            format!("MIDI inputs: {}", ports.join(", "))
        });
        ui.horizontal(|ui| {
            ui.label("MIDI learn parameter:");
            let mut param = ParamId::MasterOpacity;
            if param_picker(ui, "learn", self.session.project(), &mut param) {
                self.live.mapper.learn(MidiTarget::Param { param });
            }
        });
        let mut remove = None;
        for (i, b) in controls.midi.iter().enumerate() {
            ui.horizontal(|ui| {
                let port = if b.port.is_empty() {
                    "any port"
                } else {
                    &b.port
                };
                ui.monospace(format!(
                    "{port} ch{} {:?} {}",
                    b.channel, b.message, b.number
                ));
                ui.label("→");
                let target = match &b.target {
                    MidiTarget::Param { param } => params::list(self.session.project())
                        .into_iter()
                        .find(|p| &p.id == param)
                        .map_or_else(|| param.to_string(), |p| p.label),
                    MidiTarget::CueGo => "GO (next cue)".into(),
                    MidiTarget::Cue { cue } => format!("cue {cue}"),
                };
                ui.label(target);
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            let mut c = controls.clone();
            c.midi.remove(i);
            self.queue(Command::SetControls { controls: c });
        }
        ui.separator();
        self.dmx_input_section(ui, &controls);
    }

    fn dmx_input_section(&mut self, ui: &mut egui::Ui, controls: &om_project::Controls) {
        let input = &controls.dmx_input;
        ui.horizontal(|ui| {
            let mut enabled = input.enabled;
            if ui
                .checkbox(&mut enabled, "DMX input (Art-Net / sACN)")
                .changed()
            {
                let mut c = controls.clone();
                c.dmx_input.enabled = enabled;
                self.queue(Command::SetControls { controls: c });
            }
            ui.weak(self.live.dmx.status());
        });
        if !input.enabled {
            return;
        }
        ui.horizontal(|ui| {
            ui.label("DMX learn parameter:");
            let mut param = ParamId::MasterOpacity;
            if param_picker(ui, "dmx learn", self.session.project(), &mut param) {
                self.live.dmx.learn(MidiTarget::Param { param });
            }
            if ui.small_button("learn GO").clicked() {
                self.live.dmx.learn(MidiTarget::CueGo);
            }
            if let Some(target) = self.live.dmx.learning() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("move a DMX channel for {target:?}"),
                );
                if ui.small_button("Cancel").clicked() {
                    self.live.dmx.cancel_learn();
                }
            }
        });
        let mut changed = None;
        for (i, b) in input.bindings.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("universe {} ch {}", b.universe, b.channel));
                let mut fine = b.fine;
                if ui
                    .add_enabled(b.channel < 512, egui::Checkbox::new(&mut fine, "16-bit"))
                    .changed()
                {
                    let mut c = controls.clone();
                    c.dmx_input.bindings[i].fine = fine;
                    changed = Some(c);
                }
                ui.label("→");
                ui.label(match &b.target {
                    MidiTarget::Param { param } => params::list(self.session.project())
                        .into_iter()
                        .find(|p| &p.id == param)
                        .map_or_else(|| param.to_string(), |p| p.label),
                    MidiTarget::CueGo => "GO (next cue)".into(),
                    MidiTarget::Cue { cue } => format!("cue {cue}"),
                });
                if ui.small_button("x").clicked() {
                    let mut c = controls.clone();
                    c.dmx_input.bindings.remove(i);
                    changed = Some(c);
                }
            });
        }
        if let Some(c) = changed {
            self.queue(Command::SetControls { controls: c });
        }
    }
}
