// SPDX-License-Identifier: Apache-2.0
//! Art-Net / sACN input as a control source: received channel values drive
//! parameters and cues through the project's [`DmxBinding`]s, and DMX learn
//! binds the next channel that moves.
//!
//! DMX repeats every universe tens of times a second, so a binding only
//! produces a control message when its value changes (and once when the
//! universe is first seen, to pick up the desk's position). Cue targets
//! fire when the value rises through 50 %, never on first sight.

use std::collections::HashMap;

use om_command::params;
use om_dmx::input::DmxInputs;
use om_dmx::net::Ports;
use om_project::{DmxBinding, DmxInput, MidiTarget, ParamKind, ParamValue, Project};
use om_show::control::{Action, ControlMessage};

/// Smallest change of an 8-bit channel that DMX learn treats as a move
/// (ignores flicker from noisy consoles).
pub const LEARN_THRESHOLD: u8 = 8;

/// Most universes whose last data is kept.
pub const MAX_TRACKED: usize = 512;

/// Value of `b` in 0..=1 from a universe's slots (missing slots read 0).
#[must_use]
pub fn binding_value(b: &DmxBinding, data: &[u8]) -> f64 {
    let slot = |ch: u16| {
        usize::from(ch)
            .checked_sub(1)
            .and_then(|i| data.get(i))
            .copied()
            .unwrap_or(0)
    };
    if b.fine {
        let v = u16::from(slot(b.channel)) << 8 | u16::from(slot(b.channel + 1));
        f64::from(v) / 65_535.0
    } else {
        f64::from(slot(b.channel)) / 255.0
    }
}

/// Control messages for one received universe, given the previous data
/// for it (`None` when first seen).
#[must_use]
pub fn map_universe(
    project: &Project,
    universe: u16,
    previous: Option<&[u8]>,
    data: &[u8],
) -> Vec<ControlMessage> {
    let mut out = Vec::new();
    for b in &project.controls.dmx_input.bindings {
        if b.universe != universe {
            continue;
        }
        let v = binding_value(b, data);
        let old = previous.map(|p| binding_value(b, p));
        if old == Some(v) {
            continue;
        }
        match &b.target {
            MidiTarget::Param { param } => {
                let Some(kind) = params::kind(project, param) else {
                    continue;
                };
                let value = match kind {
                    ParamKind::Bool => ParamValue::Bool(v >= 0.5),
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
            target => {
                let rising = old.is_some_and(|o| o < 0.5) && v >= 0.5;
                match target {
                    MidiTarget::CueGo if rising => {
                        out.push(ControlMessage::Action(Action::CueGoNext));
                    }
                    MidiTarget::Cue { cue } if rising => {
                        out.push(ControlMessage::Action(Action::CueGo(*cue)));
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// The first channel that moved by at least [`LEARN_THRESHOLD`].
#[must_use]
pub fn moved_channel(previous: &[u8], data: &[u8]) -> Option<u16> {
    previous
        .iter()
        .zip(data)
        .position(|(a, b)| a.abs_diff(*b) >= LEARN_THRESHOLD)
        .and_then(|i| u16::try_from(i + 1).ok())
}

/// Input sockets plus the last data per universe.
#[derive(Debug, Default)]
pub struct DmxControl {
    inputs: Option<DmxInputs>,
    universes: Vec<u16>,
    last: HashMap<u16, Vec<u8>>,
    learning: Option<MidiTarget>,
    /// Packets received since enabled.
    pub received: u64,
}

impl DmxControl {
    /// Arms DMX learn for `target` (the next channel that moves is bound).
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

    /// Listening status ("off" when disabled).
    #[must_use]
    pub fn status(&self) -> String {
        self.inputs
            .as_ref()
            .map_or_else(|| "off".into(), DmxInputs::status)
    }

    /// Opens, re-joins or closes sockets to match `config` on the
    /// standard ports.
    pub fn configure(&mut self, config: &DmxInput) {
        self.configure_with(config, Ports::default());
    }

    /// As [`Self::configure`], on explicit ports (tests).
    pub fn configure_with(&mut self, config: &DmxInput, ports: Ports) {
        if !config.enabled {
            self.inputs = None;
            self.last.clear();
            self.learning = None;
            return;
        }
        let universes = config.universes();
        match &mut self.inputs {
            None => {
                self.inputs = Some(DmxInputs::open(ports, &universes));
                self.universes = universes;
            }
            Some(inputs) if universes != self.universes => {
                inputs.set_universes(&universes);
                self.universes = universes;
            }
            Some(_) => {}
        }
    }

    /// Drains received packets. Returns control messages and, when
    /// learning, the new binding to add.
    pub fn poll(&mut self, project: &Project) -> (Vec<ControlMessage>, Option<DmxBinding>) {
        let Some(inputs) = &mut self.inputs else {
            return (Vec::new(), None);
        };
        let mut out = Vec::new();
        let mut learned = None;
        for r in inputs.drain() {
            self.received += 1;
            if r.terminated {
                continue;
            }
            // Keep data only for universes that matter (decided before learn
            // completes, so the learning packet itself is kept), bounded
            // against floods of arbitrary universe numbers.
            let wanted =
                self.universes.binary_search(&r.universe).is_ok() || self.learning.is_some();
            let previous = self.last.get(&r.universe);
            if let (Some(target), Some(prev)) = (&self.learning, previous)
                && let Some(channel) = moved_channel(prev, &r.data)
            {
                learned = Some(DmxBinding {
                    universe: r.universe,
                    channel,
                    fine: false,
                    target: target.clone(),
                });
                self.learning = None;
            }
            out.extend(map_universe(
                project,
                r.universe,
                previous.map(Vec::as_slice),
                &r.data,
            ));
            if wanted && (self.last.len() < MAX_TRACKED || self.last.contains_key(&r.universe)) {
                self.last.insert(r.universe, r.data);
            }
        }
        (out, learned)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::{Duration, Instant};

    use om_project::ParamId;
    use om_types::CueId;

    use super::*;

    fn binding(channel: u16, fine: bool, target: MidiTarget) -> DmxBinding {
        DmxBinding {
            universe: 1,
            channel,
            fine,
            target,
        }
    }

    fn project(bindings: Vec<DmxBinding>) -> Project {
        let mut p = Project::new("dmx in");
        p.controls.dmx_input = DmxInput {
            enabled: true,
            bindings,
        };
        p.validate().unwrap();
        p
    }

    fn opacity() -> MidiTarget {
        MidiTarget::Param {
            param: ParamId::MasterOpacity,
        }
    }

    fn set(msgs: &[ControlMessage]) -> Vec<ParamValue> {
        msgs.iter()
            .filter_map(|m| match m {
                ControlMessage::Set { value, .. } => Some(*value),
                ControlMessage::Action(_) => None,
            })
            .collect()
    }

    #[test]
    fn values_scale_and_only_changes_are_sent() {
        let p = project(vec![
            binding(1, false, opacity()),
            binding(
                3,
                false,
                MidiTarget::Param {
                    param: ParamId::MasterBlackout,
                },
            ),
        ]);
        let a = [255, 0, 127, 0];
        // First sight syncs every binding.
        assert_eq!(
            set(&map_universe(&p, 1, None, &a)),
            vec![ParamValue::Float(1.0), ParamValue::Bool(false)]
        );
        // Unchanged: nothing; another universe: nothing.
        assert!(map_universe(&p, 1, Some(&a), &a).is_empty());
        assert!(map_universe(&p, 2, None, &a).is_empty());
        let b = [51, 0, 128, 0];
        let out = set(&map_universe(&p, 1, Some(&a), &b));
        assert_eq!(out[1], ParamValue::Bool(true), "128/255 ≥ 50 %");
        assert!(matches!(out[0], ParamValue::Float(v) if (v - 0.2).abs() < 1e-12));
        // A short universe reads missing slots as 0.
        assert_eq!(
            set(&map_universe(&p, 1, Some(&a), &[255])),
            vec![ParamValue::Bool(false)]
        );
    }

    #[test]
    fn sixteen_bit_channels_use_coarse_then_fine() {
        let b = binding(511, true, opacity());
        assert!((binding_value(&b, &[0; 512]) - 0.0).abs() < 1e-12);
        let mut data = [0u8; 512];
        data[510] = 0x80;
        data[511] = 0x00;
        assert!((binding_value(&b, &data) - 32_768.0 / 65_535.0).abs() < 1e-12);
        data[511] = 0x01;
        assert!((binding_value(&b, &data) - 32_769.0 / 65_535.0).abs() < 1e-12);
        // Channel 512 cannot be 16-bit.
        let mut p = project(Vec::new());
        p.controls
            .dmx_input
            .bindings
            .push(binding(512, true, opacity()));
        assert!(p.validate().is_err());
        p.controls.dmx_input.bindings[0] = binding(0, false, opacity());
        assert!(p.validate().is_err());
    }

    #[test]
    fn cues_fire_on_rising_edges_only() {
        let cue = CueId::from_u128(5);
        let p = project(vec![
            binding(1, false, MidiTarget::CueGo),
            binding(2, false, MidiTarget::Cue { cue }),
        ]);
        let hi = [255, 255];
        assert!(
            map_universe(&p, 1, None, &hi).is_empty(),
            "never on first sight"
        );
        let lo = [0, 0];
        assert!(map_universe(&p, 1, Some(&hi), &lo).is_empty(), "falling");
        assert_eq!(
            map_universe(&p, 1, Some(&lo), &hi),
            vec![
                ControlMessage::Action(Action::CueGoNext),
                ControlMessage::Action(Action::CueGo(cue)),
            ]
        );
        assert!(
            map_universe(&p, 1, Some(&hi), &[200, 200]).is_empty(),
            "stays high"
        );
    }

    #[test]
    fn learn_binds_the_channel_that_moves_over_the_network() {
        let mut control = DmxControl::default();
        let mut p = project(Vec::new());
        control.configure_with(&p.controls.dmx_input, Ports { artnet: 0, sacn: 0 });
        let port = control.inputs.as_ref().unwrap().artnet_port().unwrap();
        let tx = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let send = |data: &[u8]| {
            tx.send_to(&om_dmx::artnet::dmx(1, 1, 0, data), ("127.0.0.1", port))
                .unwrap();
        };
        let wait = |control: &mut DmxControl, p: &Project| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let before = control.received;
            loop {
                let r = control.poll(p);
                if control.received > before || Instant::now() > deadline {
                    return r;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        control.learn(opacity());
        send(&[0, 0, 0, 0]);
        assert!(wait(&mut control, &p).1.is_none(), "first sight is no move");
        send(&[0, 0, 3, 0]);
        assert!(
            wait(&mut control, &p).1.is_none(),
            "flicker below threshold"
        );
        send(&[0, 0, 3, 200]);
        let learned = wait(&mut control, &p).1.unwrap();
        assert_eq!((learned.universe, learned.channel), (1, 4));
        assert!(control.learning().is_none());

        // The binding now drives the parameter.
        p.controls.dmx_input.bindings.push(learned);
        control.configure_with(&p.controls.dmx_input, Ports { artnet: 0, sacn: 0 });
        send(&[0, 0, 3, 0]);
        let (msgs, _) = wait(&mut control, &p);
        assert_eq!(set(&msgs), vec![ParamValue::Float(0.0)]);

        // Disabling closes the sockets.
        p.controls.dmx_input.enabled = false;
        control.configure(&p.controls.dmx_input);
        assert_eq!(control.status(), "off");
    }
}
