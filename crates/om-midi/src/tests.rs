// SPDX-License-Identifier: Apache-2.0

use super::*;
use om_project::{ParamId, Surface};
use om_types::{CueId, SurfaceId};

fn ev(kind: MidiMessageKind, number: u8, value: u8) -> PortEvent {
    PortEvent {
        port: "Pad".into(),
        event: MidiEvent {
            kind,
            channel: 1,
            number,
            value,
        },
    }
}

fn project() -> Project {
    let mut p = Project::new("midi");
    p.surfaces.push(Surface::new(SurfaceId::from_u128(1), "a"));
    p.controls.midi = vec![
        MidiBinding {
            port: String::new(),
            message: MidiMessageKind::ControlChange,
            channel: 1,
            number: 7,
            target: MidiTarget::Param {
                param: ParamId::SurfaceOpacity(SurfaceId::from_u128(1)),
            },
        },
        MidiBinding {
            port: "Pad".into(),
            message: MidiMessageKind::Note,
            channel: 1,
            number: 36,
            target: MidiTarget::CueGo,
        },
        MidiBinding {
            port: "Other".into(),
            message: MidiMessageKind::Note,
            channel: 1,
            number: 37,
            target: MidiTarget::Cue {
                cue: CueId::from_u128(9),
            },
        },
    ];
    p
}

#[test]
fn decodes_channel_messages() {
    assert_eq!(
        decode(&[0xb3, 7, 100]),
        Some(MidiEvent {
            kind: MidiMessageKind::ControlChange,
            channel: 4,
            number: 7,
            value: 100
        })
    );
    assert_eq!(decode(&[0x80, 60, 64]).unwrap().value, 0, "note off");
    assert_eq!(
        decode(&[0x90, 60, 0]).unwrap().value,
        0,
        "note on velocity 0"
    );
    assert_eq!(decode(&[0xe0, 0, 64]), None, "pitch bend ignored");
    assert_eq!(decode(&[0xb0, 7]), None, "truncated");
}

#[test]
fn bindings_scale_and_trigger_on_rising_edges() {
    let p = project();
    let mut m = Mapper::new();
    let (out, _) = m.handle(&p, &ev(MidiMessageKind::ControlChange, 7, 127));
    assert_eq!(
        out,
        vec![ControlMessage::Set {
            param: ParamId::SurfaceOpacity(SurfaceId::from_u128(1)),
            value: ParamValue::Float(1.0),
        }]
    );
    let (out, _) = m.handle(&p, &ev(MidiMessageKind::Note, 36, 100));
    assert_eq!(out, vec![ControlMessage::Action(Action::CueGoNext)]);
    // Held/repeated note-on does not re-trigger; release then press does.
    assert!(
        m.handle(&p, &ev(MidiMessageKind::Note, 36, 90))
            .0
            .is_empty()
    );
    assert!(m.handle(&p, &ev(MidiMessageKind::Note, 36, 0)).0.is_empty());
    assert_eq!(m.handle(&p, &ev(MidiMessageKind::Note, 36, 80)).0.len(), 1);
    // Port-specific binding ignores other ports.
    assert!(
        m.handle(&p, &ev(MidiMessageKind::Note, 37, 100))
            .0
            .is_empty()
    );
}

#[test]
fn learn_creates_a_binding_from_the_next_control() {
    let p = project();
    let mut m = Mapper::new();
    m.learn(MidiTarget::Param {
        param: ParamId::MasterOpacity,
    });
    // Note-off is ignored while learning.
    assert_eq!(m.handle(&p, &ev(MidiMessageKind::Note, 40, 0)).1, None);
    let (out, binding) = m.handle(&p, &ev(MidiMessageKind::ControlChange, 21, 5));
    assert!(out.is_empty(), "learning consumes the event");
    let b = binding.unwrap();
    assert_eq!((b.port.as_str(), b.channel, b.number), ("Pad", 1, 21));
    assert!(m.learning().is_none());
}

/// Real loopback through the OS MIDI layer (virtual ports exist on macOS
/// and Linux/ALSA; skipped where unavailable, e.g. CI without ALSA seq).
#[cfg(not(windows))]
#[test]
fn virtual_port_loopback() {
    use midir::os::unix::VirtualOutput;
    let Ok(out) = midir::MidiOutput::new("loopback-test") else {
        return;
    };
    let Ok(mut conn) = out.create_virtual("OM Loopback Test") else {
        eprintln!("skipping: virtual MIDI ports unavailable");
        return;
    };
    let mut inputs = MidiInputs::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !inputs
        .ports()
        .iter()
        .any(|p| p.contains("OM Loopback Test"))
        && Instant::now() < deadline
    {
        inputs.rescan(true);
        std::thread::sleep(Duration::from_millis(50));
    }
    if !inputs
        .ports()
        .iter()
        .any(|p| p.contains("OM Loopback Test"))
    {
        eprintln!(
            "skipping: virtual port not visible to inputs ({:?})",
            inputs.ports()
        );
        return;
    }
    conn.send(&[0xb0, 7, 99]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got.is_empty() && Instant::now() < deadline {
        got = inputs.drain();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].event,
        MidiEvent {
            kind: MidiMessageKind::ControlChange,
            channel: 1,
            number: 7,
            value: 99
        }
    );
    // Unplug: the port disappears from the next rescan.
    drop(conn);
    std::thread::sleep(Duration::from_millis(200));
    inputs.rescan(true);
    assert!(
        !inputs
            .ports()
            .iter()
            .any(|p| p.contains("OM Loopback Test"))
    );
}
