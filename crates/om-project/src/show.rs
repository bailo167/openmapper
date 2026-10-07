// SPDX-License-Identifier: Apache-2.0
//! Show-control document types: cues, timelines, modulators, master levels
//! and control bindings. Their runtime behaviour lives in `om-show`.

use om_time::RationalTime;
use om_types::{CueId, Finite, ModulatorId, TimelineId, UnitInterval};
use serde::{Deserialize, Serialize};

use crate::param::{ParamId, ParamValue};

/// Show-control state of a project.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    /// The cue list, in GO order.
    #[serde(default)]
    pub cues: Vec<Cue>,
    #[serde(default)]
    pub timelines: Vec<Timeline>,
    #[serde(default)]
    pub modulators: Vec<Modulator>,
}

/// A look: target values reached over `fade` seconds when the cue runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    pub id: CueId,
    pub name: String,
    /// Fade time in seconds (0 = snap).
    #[serde(default)]
    pub fade: Finite,
    #[serde(default)]
    pub values: Vec<CueValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CueValue {
    pub param: ParamId,
    pub value: ParamValue,
}

/// Keyframe automation of parameters over exact time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timeline {
    pub id: TimelineId,
    pub name: String,
    pub duration: RationalTime,
    #[serde(default)]
    pub looping: bool,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub param: ParamId,
    /// Sorted by time when evaluated; duplicates keep the later entry.
    #[serde(default)]
    pub keys: Vec<Keyframe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
    pub at: RationalTime,
    pub value: ParamValue,
    /// How the value moves from this key to the next.
    #[serde(default)]
    pub ease: Ease,
}

/// Interpolation from one key to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    #[default]
    Linear,
    /// Hold until the next key.
    Step,
    /// Smoothstep (ease in and out).
    Smooth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub at: RationalTime,
    pub name: String,
}

/// Continuously drives a parameter: `value = offset + depth × signal`, with
/// `signal` in 0..=1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modulator {
    pub id: ModulatorId,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub param: ParamId,
    pub source: ModSource,
    #[serde(default = "one")]
    pub depth: Finite,
    #[serde(default)]
    pub offset: Finite,
}

/// Where a modulator's signal comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModSource {
    /// Low-frequency oscillator on the show clock.
    Lfo {
        shape: LfoShape,
        /// Cycles per second.
        rate: Finite,
        /// Phase offset in cycles (0..1).
        #[serde(default)]
        phase: Finite,
    },
    /// Audio analysis level (0..1) of the selected band.
    Audio {
        band: AudioBand,
        #[serde(default = "one")]
        gain: Finite,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape {
    Sine,
    Triangle,
    Square,
    Saw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBand {
    /// Overall level.
    Level,
    Low,
    Mid,
    High,
}

/// Master output controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Master {
    #[serde(default)]
    pub opacity: UnitInterval,
    #[serde(default)]
    pub blackout: bool,
}

impl Default for Master {
    fn default() -> Self {
        Self {
            opacity: UnitInterval::ONE,
            blackout: false,
        }
    }
}

/// External control configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Controls {
    /// UDP port for incoming OSC (0 disables OSC).
    #[serde(default = "default_osc_port")]
    pub osc_port: u16,
    /// HTTP port for the OSCQuery service (0 disables it).
    #[serde(default = "default_oscquery_port")]
    pub oscquery_port: u16,
    /// Accept OSC and OSCQuery from other machines (all interfaces, mDNS
    /// advertisement). Off: this computer only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// MIDI-to-parameter mappings.
    #[serde(default)]
    pub midi: Vec<MidiBinding>,
    /// Art-Net / sACN input as a control source (off by default).
    #[serde(default, skip_serializing_if = "DmxInput::is_default")]
    pub dmx_input: DmxInput,
}

impl Controls {
    /// Checks binding ranges.
    pub fn validate(&self) -> Result<(), String> {
        if self.midi.len() > MAX_CONTROL_BINDINGS {
            return Err(format!(
                "{} MIDI bindings (at most {MAX_CONTROL_BINDINGS})",
                self.midi.len()
            ));
        }
        for b in &self.midi {
            if !(1..=16).contains(&b.channel) || b.number > 127 {
                return Err(format!(
                    "MIDI binding channel {} / number {} out of range",
                    b.channel, b.number
                ));
            }
        }
        self.dmx_input.validate()
    }
}

/// Most MIDI or DMX bindings a project may hold.
pub const MAX_CONTROL_BINDINGS: usize = 4096;

/// Art-Net / sACN input mapped to parameters and cues.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmxInput {
    /// Listen for Art-Net (UDP 6454) and sACN (UDP 5568, multicast for
    /// the bound universes).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bindings: Vec<DmxBinding>,
}

impl DmxInput {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Checks binding ranges.
    pub fn validate(&self) -> Result<(), String> {
        if self.bindings.len() > MAX_CONTROL_BINDINGS {
            return Err(format!(
                "{} DMX input bindings (at most {MAX_CONTROL_BINDINGS})",
                self.bindings.len()
            ));
        }
        for b in &self.bindings {
            let last = if b.fine { 511 } else { 512 };
            if !(1..=last).contains(&b.channel) || b.universe > MAX_INPUT_UNIVERSE {
                return Err(format!(
                    "DMX input binding universe {} channel {}{} out of range",
                    b.universe,
                    b.channel,
                    if b.fine { " (16-bit)" } else { "" }
                ));
            }
        }
        Ok(())
    }

    /// Distinct universes the bindings use, ascending.
    #[must_use]
    pub fn universes(&self) -> Vec<u16> {
        let mut u: Vec<u16> = self.bindings.iter().map(|b| b.universe).collect();
        u.sort_unstable();
        u.dedup();
        u
    }
}

/// Highest input universe number (the sACN limit; Art-Net port addresses
/// stop at 32767).
pub const MAX_INPUT_UNIVERSE: u16 = 63_999;

/// Maps one DMX channel (or a 16-bit channel pair) to a parameter or cue.
/// The universe number is the Art-Net port address or the sACN universe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmxBinding {
    pub universe: u16,
    /// 1..=512 (coarse byte when `fine`).
    pub channel: u16,
    /// 16-bit: `channel` is the coarse byte and `channel + 1` the fine.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fine: bool,
    /// Full range scales onto a parameter's range (bools: ≥ 50 % is on);
    /// cue targets fire when the value rises through 50 %.
    pub target: MidiTarget,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            osc_port: default_osc_port(),
            oscquery_port: default_oscquery_port(),
            network: false,
            midi: Vec::new(),
            dmx_input: DmxInput::default(),
        }
    }
}

/// Maps one MIDI control to a parameter or action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiBinding {
    /// Input port name (empty = any port).
    #[serde(default)]
    pub port: String,
    pub message: MidiMessageKind,
    /// 1..=16
    pub channel: u8,
    /// Controller or note number 0..=127.
    pub number: u8,
    pub target: MidiTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiMessageKind {
    ControlChange,
    Note,
}

/// What a MIDI control drives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MidiTarget {
    /// 0..127 scaled onto the parameter's range (bools: ≥ 64 is on).
    Param { param: ParamId },
    /// Runs the next cue on note-on / value ≥ 64.
    CueGo,
    /// Runs a specific cue.
    Cue { cue: CueId },
}

fn default_osc_port() -> u16 {
    8010
}

fn default_oscquery_port() -> u16 {
    8011
}

fn yes() -> bool {
    true
}

fn one() -> Finite {
    Finite::ONE
}
