// SPDX-License-Identifier: Apache-2.0
//! MIDI input.
//!
//! [`MidiInputs`] listens on every input port (reconnecting as devices come
//! and go). [`Mapper`] turns incoming messages into control messages using
//! the project's bindings, and implements MIDI learn.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use om_command::params;
use om_project::{MidiBinding, MidiMessageKind, MidiTarget, ParamKind, ParamValue, Project};
use om_show::control::{Action, ControlMessage};

/// A decoded MIDI channel message we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub kind: MidiMessageKind,
    /// 1..=16
    pub channel: u8,
    pub number: u8,
    /// CC value or note velocity (note-off = 0).
    pub value: u8,
}

/// Decodes a raw MIDI message (running status not needed: one message per
/// callback). Other message types return `None`.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<MidiEvent> {
    let (&status, rest) = bytes.split_first()?;
    let channel = (status & 0x0f) + 1;
    let (number, value) = (*rest.first()? & 0x7f, *rest.get(1)? & 0x7f);
    let (kind, value) = match status & 0xf0 {
        0xb0 => (MidiMessageKind::ControlChange, value),
        0x90 => (MidiMessageKind::Note, value),
        0x80 => (MidiMessageKind::Note, 0),
        _ => return None,
    };
    Some(MidiEvent {
        kind,
        channel,
        number,
        value,
    })
}

/// Event from a named port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortEvent {
    pub port: String,
    pub event: MidiEvent,
}

/// Maps MIDI events to control messages; also handles learn.
#[derive(Debug, Default)]
pub struct Mapper {
    /// Last value per control, for trigger edge detection.
    last: HashMap<(String, MidiMessageKind, u8, u8), u8>,
    /// Target waiting for the next control to be moved.
    learning: Option<MidiTarget>,
}

impl Mapper {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms MIDI learn for `target` (the next event creates a binding).
    pub fn learn(&mut self, target: MidiTarget) {
        self.learning = Some(target);
    }

    pub fn cancel_learn(&mut self) {
        self.learning = None;
    }

    #[must_use]
    pub fn learning(&self) -> Option<&MidiTarget> {
        self.learning.as_ref()
    }

    /// Handles one event. Returns control messages to apply and, when
    /// learning, the new binding to add to the project.
    pub fn handle(
        &mut self,
        project: &Project,
        e: &PortEvent,
    ) -> (Vec<ControlMessage>, Option<MidiBinding>) {
        let key = (
            e.port.clone(),
            e.event.kind,
            e.event.channel,
            e.event.number,
        );
        let previous = self.last.insert(key, e.event.value).unwrap_or(0);
        if let Some(target) = self.learning.take() {
            // Learn on any CC, or note-on (ignore note-offs).
            if e.event.kind == MidiMessageKind::ControlChange || e.event.value > 0 {
                let binding = MidiBinding {
                    port: e.port.clone(),
                    message: e.event.kind,
                    channel: e.event.channel,
                    number: e.event.number,
                    target,
                };
                return (Vec::new(), Some(binding));
            }
            self.learning = Some(target);
            return (Vec::new(), None);
        }
        let rising = previous < 64 && e.event.value >= 64
            || (e.event.kind == MidiMessageKind::Note && e.event.value > 0 && previous == 0);
        let mut out = Vec::new();
        for b in &project.controls.midi {
            let matches = (b.port.is_empty() || b.port == e.port)
                && b.message == e.event.kind
                && b.channel == e.event.channel
                && b.number == e.event.number;
            if !matches {
                continue;
            }
            match &b.target {
                MidiTarget::Param { param } => {
                    let Some(kind) = params::kind(project, param) else {
                        continue;
                    };
                    let v = f64::from(e.event.value) / 127.0;
                    let value = match kind {
                        ParamKind::Bool => ParamValue::Bool(e.event.value >= 64),
                        ParamKind::Float { min, max } => {
                            let (lo, hi) = (min.max(-1e6), max.min(1e6));
                            ParamValue::Float(lo + (hi - lo) * v)
                        }
                    };
                    out.push(ControlMessage::Set {
                        param: param.clone(),
                        value,
                    });
                }
                MidiTarget::CueGo if rising => out.push(ControlMessage::Action(Action::CueGoNext)),
                MidiTarget::Cue { cue } if rising => {
                    out.push(ControlMessage::Action(Action::CueGo(*cue)))
                }
                _ => {}
            }
        }
        (out, None)
    }
}

/// All MIDI input ports, kept connected.
pub struct MidiInputs {
    connections: HashMap<String, midir::MidiInputConnection<()>>,
    tx: Sender<PortEvent>,
    rx: Receiver<PortEvent>,
    last_scan: Option<Instant>,
    /// Problems opening ports (for diagnostics).
    pub errors: Vec<String>,
}

impl std::fmt::Debug for MidiInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiInputs")
            .field("ports", &self.connections.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// How often ports are rescanned for hot-plugged devices.
const RESCAN: Duration = Duration::from_secs(2);

impl Default for MidiInputs {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiInputs {
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            connections: HashMap::new(),
            tx,
            rx,
            last_scan: None,
            errors: Vec::new(),
        }
    }

    /// Connected port names.
    #[must_use]
    pub fn ports(&self) -> Vec<String> {
        let mut v: Vec<String> = self.connections.keys().cloned().collect();
        v.sort();
        v
    }

    /// Connects new ports and drops vanished ones (at most every 2 s unless
    /// `force`).
    pub fn rescan(&mut self, force: bool) {
        if !force && self.last_scan.is_some_and(|t| t.elapsed() < RESCAN) {
            return;
        }
        self.last_scan = Some(Instant::now());
        let Ok(probe) = midir::MidiInput::new("OpenMapper probe") else {
            return;
        };
        let available: Vec<(String, midir::MidiInputPort)> = probe
            .ports()
            .into_iter()
            .filter_map(|p| probe.port_name(&p).ok().map(|n| (n, p)))
            .filter(|(n, _)| !n.starts_with("OpenMapper"))
            .collect();
        let names: HashSet<&String> = available.iter().map(|(n, _)| n).collect();
        self.connections.retain(|name, _| names.contains(name));
        for (name, port) in available {
            if self.connections.contains_key(&name) {
                continue;
            }
            let Ok(input) = midir::MidiInput::new("OpenMapper") else {
                continue;
            };
            let tx = self.tx.clone();
            let port_name = name.clone();
            match input.connect(
                &port,
                "openmapper-in",
                move |_, bytes, ()| {
                    if let Some(event) = decode(bytes) {
                        let _ = tx.send(PortEvent {
                            port: port_name.clone(),
                            event,
                        });
                    }
                },
                (),
            ) {
                Ok(conn) => {
                    self.connections.insert(name, conn);
                }
                Err(e) => self.errors.push(format!("{name}: {e}")),
            }
        }
    }

    /// Events received since the last call.
    pub fn drain(&self) -> Vec<PortEvent> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests;
