// SPDX-License-Identifier: Apache-2.0
//! Live control: show runtime, external control servers, and applying
//! control messages to the session.

use std::time::{Duration, Instant};

use om_command::params;
use om_project::{Controls, Project};
use om_show::control::{Action, ControlMessage};
use om_show::{AudioLevels, Overrides, ShowState};
use om_time::RationalTime;

use crate::{Session, Transport};

/// The project as it should be rendered now: the document with this
/// frame's show overrides applied (a derived copy; the document is
/// untouched).
#[must_use]
pub fn effective_project(project: &Project, overrides: &Overrides) -> Project {
    let mut p = project.clone();
    params::apply_overrides(&mut p, overrides);
    p
}

/// Applies one control message. Parameter sets become undoable commands
/// (coalesced per parameter, so a fader sweep is one undo step). `show_time`
/// is the live show clock (cues, timelines, modulators); the transport only
/// governs media playback.
pub fn apply_control(
    msg: &ControlMessage,
    session: &mut Session,
    transport: &mut Transport,
    show: &mut ShowState,
    now: Instant,
    show_time: RationalTime,
) -> Result<(), String> {
    match msg {
        ControlMessage::Set { param, value } => {
            let cmd = params::set_command(session.project(), param, *value)
                .ok_or_else(|| format!("no such parameter: {param}"))?;
            session
                .execute_coalescing(cmd, format!("control:{param}"))
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        ControlMessage::Action(a) => {
            match a {
                Action::Play => transport.play(now),
                Action::Pause => transport.pause(now),
                Action::Restart => transport.seek(RationalTime::ZERO, now),
                Action::CueGoNext => {
                    show.go_next(session.project(), show_time);
                }
                Action::CueGo(id) => show.go(session.project(), *id, show_time),
                Action::CueRelease { fade } => show.release(session.project(), show_time, *fade),
                Action::TimelinePlay(id) => show.play_timeline(*id, show_time),
                Action::TimelinePause(id) => show.pause_timeline(*id, show_time),
                Action::TimelineStop(id) => show.stop_timeline(*id),
                Action::TimelineSeek(id, seconds) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let pos = RationalTime::new((seconds * 1000.0).round() as i128, 1000)
                        .map_err(|e| e.to_string())?;
                    show.seek_timeline(*id, pos, show_time);
                }
            }
            Ok(())
        }
    }
}

/// Most control messages applied per frame.
pub const MAX_CONTROL_MESSAGES_PER_FRAME: usize = 1024;

/// One frame's control messages as applied: only the last value per
/// parameter (a flood of sets becomes one edit and one journal line per
/// parameter per frame), actions in order, at most
/// [`MAX_CONTROL_MESSAGES_PER_FRAME`].
#[must_use]
pub fn coalesce(messages: Vec<ControlMessage>) -> Vec<ControlMessage> {
    let last: std::collections::HashMap<&om_project::ParamId, usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| match m {
            ControlMessage::Set { param, .. } => Some((param, i)),
            ControlMessage::Action(_) => None,
        })
        .collect();
    let keep: Vec<bool> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| match m {
            ControlMessage::Set { param, .. } => last.get(param) == Some(&i),
            ControlMessage::Action(_) => true,
        })
        .collect();
    messages
        .into_iter()
        .zip(keep)
        .filter_map(|(m, k)| k.then_some(m))
        .take(MAX_CONTROL_MESSAGES_PER_FRAME)
        .collect()
}

/// OSC and OSCQuery servers, (re)started to match the project's settings.
#[derive(Debug, Default)]
pub struct ControlServers {
    osc: Option<om_osc::OscServer>,
    oscquery: Option<om_oscquery::OscQueryServer>,
    ports: Option<(u16, u16, bool)>,
    last_publish: Option<Instant>,
    /// Human-readable status ("OSC :8010, OSCQuery :8011" or an error).
    pub status: String,
    /// Recent problems (bad messages), newest last, bounded.
    pub errors: Vec<String>,
}

const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

impl ControlServers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts/stops/restarts servers when the configured ports change.
    pub fn configure(&mut self, controls: &Controls, advertise: bool) {
        let wanted = (controls.osc_port, controls.oscquery_port, controls.network);
        if self.ports == Some(wanted) {
            return;
        }
        self.ports = Some(wanted);
        self.osc = None;
        self.oscquery = None;
        let mut status = Vec::new();
        if controls.osc_port != 0 {
            match om_osc::OscServer::start_on(controls.osc_port, controls.network) {
                Ok(s) => {
                    status.push(format!("OSC :{}", s.port()));
                    self.osc = Some(s);
                }
                Err(e) => status.push(e.to_string()),
            }
        }
        if controls.oscquery_port != 0 {
            let osc_port = self.osc.as_ref().map_or(0, om_osc::OscServer::port);
            match om_oscquery::OscQueryServer::start_on(
                controls.oscquery_port,
                osc_port,
                "OpenMapper",
                advertise,
                controls.network,
            ) {
                Ok(s) => {
                    status.push(format!("OSCQuery :{}", s.port()));
                    self.oscquery = Some(s);
                }
                Err(e) => status.push(e.to_string()),
            }
        }
        self.status = if status.is_empty() {
            "control servers off".into()
        } else if controls.network {
            format!("{} (network)", status.join(", "))
        } else {
            format!("{} (this computer only)", status.join(", "))
        };
    }

    /// Ports actually bound (OSC, OSCQuery).
    #[must_use]
    pub fn ports(&self) -> (Option<u16>, Option<u16>) {
        (
            self.osc.as_ref().map(om_osc::OscServer::port),
            self.oscquery
                .as_ref()
                .map(om_oscquery::OscQueryServer::port),
        )
    }

    /// Control messages received since the last call; bad ones are logged.
    pub fn poll(&mut self) -> Vec<ControlMessage> {
        let Some(osc) = &self.osc else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in osc.drain() {
            match m {
                Ok(msg) => out.push(msg),
                Err(e) => {
                    self.errors.push(e.to_string());
                    if self.errors.len() > 50 {
                        self.errors.remove(0);
                    }
                }
            }
        }
        out
    }

    /// Publishes the namespace and current (effective) values to OSCQuery,
    /// at most every 100 ms unless `force`.
    pub fn publish(&mut self, effective: &Project, force: bool) {
        let Some(q) = &self.oscquery else { return };
        if !force
            && self
                .last_publish
                .is_some_and(|t| t.elapsed() < PUBLISH_INTERVAL)
        {
            return;
        }
        self.last_publish = Some(Instant::now());
        q.update(snapshot(effective));
    }
}

/// OSCQuery description of a project.
#[must_use]
pub fn snapshot(project: &Project) -> om_oscquery::Snapshot {
    let params = params::list(project)
        .into_iter()
        .filter_map(|info| {
            Some(om_oscquery::ParamNode {
                address: om_osc::address(&info.id),
                value: params::get(project, &info.id)?,
                kind: info.kind,
                description: info.label,
            })
        })
        .collect();
    let mut actions = vec![
        action("/openmapper/transport/play", "Play", None),
        action("/openmapper/transport/pause", "Pause", None),
        action("/openmapper/transport/restart", "Restart show clock", None),
        action("/openmapper/cue/go", "Run next cue", None),
        action(
            "/openmapper/cue/release",
            "Release cues (fade seconds)",
            Some("f"),
        ),
    ];
    for c in &project.show.cues {
        actions.push(om_oscquery::ActionNode {
            address: format!("/openmapper/cue/{}/go", c.id),
            description: format!("Run cue {}", c.name),
            argument: None,
        });
    }
    for t in &project.show.timelines {
        for (op, arg) in [
            ("play", None),
            ("pause", None),
            ("stop", None),
            ("seek", Some("f")),
        ] {
            actions.push(om_oscquery::ActionNode {
                address: format!("/openmapper/timeline/{}/{op}", t.id),
                description: format!("{op} timeline {}", t.name),
                argument: arg,
            });
        }
    }
    om_oscquery::Snapshot { params, actions }
}

fn action(
    address: &str,
    description: &str,
    argument: Option<&'static str>,
) -> om_oscquery::ActionNode {
    om_oscquery::ActionNode {
        address: address.into(),
        description: description.into(),
        argument,
    }
}

/// Bundles the show runtime with its inputs for front ends.
#[derive(Debug, Default)]
pub struct Live {
    pub show: ShowState,
    /// Audio analysis levels for audio modulators (set by the front end).
    pub audio: AudioLevels,
    pub servers: ControlServers,
    pub midi: om_midi::MidiInputs,
    pub mapper: om_midi::Mapper,
    /// Art-Net / sACN input.
    pub dmx: crate::dmx_input::DmxControl,
    /// Overrides from the last frame (for UI display).
    pub overrides: Overrides,
    /// Disable to keep tests from touching MIDI devices / mDNS.
    pub devices: bool,
    /// Holds back the project's external connections (camera, network
    /// output, DMX, remote control; see [`crate::trust`]). Front ends set
    /// it for an opened project the user has not allowed.
    pub external_blocked: bool,
    /// Origin of the live show clock (always running, unlike the media
    /// transport, so fades and modulators continue while media is paused).
    epoch: Option<Instant>,
}

impl Live {
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: true,
            ..Self::default()
        }
    }

    /// The live show clock at `now`.
    pub fn clock(&mut self, now: Instant) -> RationalTime {
        let epoch = *self.epoch.get_or_insert(now);
        RationalTime::from_nanos(now.saturating_duration_since(epoch).as_nanos())
    }

    /// Applies a control message from the UI.
    pub fn control(
        &mut self,
        msg: &ControlMessage,
        session: &mut Session,
        transport: &mut Transport,
        now: Instant,
    ) -> Result<(), String> {
        let t = self.clock(now);
        apply_control(msg, session, transport, &mut self.show, now, t)
    }

    /// True while something changes over time without input (fades,
    /// playing timelines, modulators): the UI should keep repainting.
    #[must_use]
    pub fn animating(&self, project: &Project) -> bool {
        self.show.fading()
            || project
                .show
                .timelines
                .iter()
                .any(|t| self.show.timeline_playing(t.id))
            || project.show.modulators.iter().any(|m| m.enabled)
            // Input arrives without UI events; keep polling it.
            || project.controls.dmx_input.enabled
    }

    /// Without MIDI devices or mDNS advertisement (tests, headless use).
    #[must_use]
    pub fn without_devices() -> Self {
        Self::default()
    }

    /// One frame: configure servers, apply received control (OSC, MIDI),
    /// evaluate the show, publish OSCQuery. Returns the project to render.
    pub fn frame(
        &mut self,
        session: &mut Session,
        transport: &mut Transport,
        now: Instant,
    ) -> Project {
        let mut controls = session.project().controls.clone();
        if self.external_blocked {
            controls.network = false;
            controls.dmx_input.enabled = false;
        }
        self.servers.configure(&controls, self.devices);
        let mut messages = self.servers.poll();
        if self.devices {
            self.midi.rescan(false);
            for event in self.midi.drain() {
                let (msgs, learned) = self.mapper.handle(session.project(), &event);
                messages.extend(msgs);
                if let Some(binding) = learned {
                    let mut controls = session.project().controls.clone();
                    controls.midi.retain(|b| {
                        !(b.port == binding.port
                            && b.message == binding.message
                            && b.channel == binding.channel
                            && b.number == binding.number)
                    });
                    controls.midi.push(binding);
                    if let Err(e) = session.execute(om_command::Command::SetControls { controls }) {
                        self.servers.errors.push(e.to_string());
                    }
                }
            }
        }
        if self.devices {
            self.dmx.configure(&controls.dmx_input);
            let (msgs, learned) = self.dmx.poll(session.project());
            messages.extend(msgs);
            if let Some(binding) = learned {
                let mut controls = session.project().controls.clone();
                controls
                    .dmx_input
                    .bindings
                    .retain(|b| !(b.universe == binding.universe && b.channel == binding.channel));
                controls.dmx_input.bindings.push(binding);
                if let Err(e) = session.execute(om_command::Command::SetControls { controls }) {
                    self.servers.errors.push(e.to_string());
                }
            }
        }
        let t = self.clock(now);
        for msg in coalesce(messages) {
            if let Err(e) = apply_control(&msg, session, transport, &mut self.show, now, t) {
                self.servers.errors.push(e);
            }
        }
        self.overrides = self.show.evaluate(session.project(), t, self.audio);
        let mut effective = effective_project(session.project(), &self.overrides);
        if self.external_blocked {
            crate::trust::restrict(&mut effective);
        }
        self.servers.publish(&effective, false);
        effective
    }
}
