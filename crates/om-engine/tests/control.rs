// SPDX-License-Identifier: Apache-2.0
//! OSC → engine → OSCQuery round trip for every exposed parameter.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use om_command::{Command, params};
use om_engine::live::Live;
use om_engine::{Session, Transport};
use om_project::{Cue, CueValue, Media, MediaSource, ParamId, ParamKind, ParamValue, Surface};
use om_types::{CueId, Finite, MediaId, SurfaceId};
use rosc::{OscMessage, OscPacket, OscType};

fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn http_value(port: u16, address: &str) -> serde_json::Value {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {address}?VALUE HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    s.read_to_string(&mut text).unwrap();
    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap_or("null"))
        .unwrap_or(serde_json::Value::Null)
}

fn send(port: u16, addr: &str, args: Vec<OscType>) {
    let bytes = rosc::encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .unwrap();
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(&bytes, ("127.0.0.1", port))
        .unwrap();
}

fn setup() -> (Session, Transport, Live, u16, u16) {
    let mut session = Session::new("control");
    session
        .execute(Command::AddSurface {
            surface: Surface::new(SurfaceId::from_u128(1), "wall"),
            index: None,
        })
        .unwrap();
    session
        .execute(Command::AddMedia {
            media: Media {
                id: MediaId::from_u128(2),
                name: "clip".into(),
                source: MediaSource::Video {
                    path: "clip.mp4".into(),
                },
                playback: Default::default(),
                extensions: Default::default(),
            },
            index: None,
        })
        .unwrap();
    let (osc, query) = (free_port(), free_tcp_port());
    let mut controls = session.project().controls.clone();
    controls.osc_port = osc;
    controls.oscquery_port = query;
    session.execute(Command::SetControls { controls }).unwrap();
    let mut live = Live::without_devices();
    let mut transport = Transport::default();
    live.frame(&mut session, &mut transport, Instant::now());
    assert_eq!(
        live.servers.ports(),
        (Some(osc), Some(query)),
        "{}",
        live.servers.status
    );
    (session, transport, live, osc, query)
}

/// Runs frames until `done` or timeout.
fn pump(
    session: &mut Session,
    transport: &mut Transport,
    live: &mut Live,
    mut done: impl FnMut(&Session) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(session) && Instant::now() < deadline {
        live.frame(session, transport, Instant::now());
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn every_parameter_round_trips_over_osc_and_oscquery() {
    let (mut session, mut transport, mut live, osc, query) = setup();
    let infos = params::list(session.project());
    assert!(infos.len() >= 6, "{infos:?}");
    for info in infos {
        let (arg, want) = match info.kind {
            ParamKind::Bool => {
                let v = !params::get(session.project(), &info.id).unwrap().as_bool();
                (OscType::Bool(v), ParamValue::Bool(v))
            }
            ParamKind::Float { min, max } => {
                let v = (min.max(-2.0) + max.min(2.0)) / 2.0 + 0.125;
                (
                    OscType::Float(v as f32),
                    ParamValue::Float(v).coerce(info.kind),
                )
            }
        };
        let address = om_osc::address(&info.id);
        send(osc, &address, vec![arg]);
        let target = info.id.clone();
        pump(&mut session, &mut transport, &mut live, |s| {
            params::get(s.project(), &target)
                .is_some_and(|v| (v.as_f64() - want.as_f64()).abs() < 1e-3)
        });
        let got = params::get(session.project(), &info.id).unwrap();
        assert!(
            (got.as_f64() - want.as_f64()).abs() < 1e-3,
            "{address}: {got:?} vs {want:?}"
        );
        // OSCQuery reports the same value.
        live.servers.publish(&session.project().clone(), true);
        let v = http_value(query, &address);
        let reported = match &v["VALUE"][0] {
            serde_json::Value::Bool(b) => f64::from(u8::from(*b)),
            other => other.as_f64().unwrap(),
        };
        assert!(
            (reported - want.as_f64()).abs() < 1e-3,
            "{address}: OSCQuery {v}"
        );
    }
    // Each fader burst coalesced into one undo step per parameter.
    assert!(session.document().can_undo());
}

#[test]
fn invalid_messages_are_rejected_and_counted() {
    let (mut session, mut transport, mut live, osc, _) = setup();
    let before = session.project().clone();
    send(
        osc,
        "/openmapper/surface/00000000000000000000000099/opacity",
        vec![OscType::Float(0.5)],
    );
    send(
        osc,
        "/openmapper/master/opacity",
        vec![OscType::String("loud".into())],
    );
    send(osc, "/elsewhere", vec![]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while live.servers.errors.len() < 3 && Instant::now() < deadline {
        live.frame(&mut session, &mut transport, Instant::now());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(live.servers.errors.len(), 3, "{:?}", live.servers.errors);
    assert_eq!(session.project(), &before, "nothing changed");
}

#[test]
fn cues_and_transport_over_osc() {
    let (mut session, mut transport, mut live, osc, _) = setup();
    let surface = SurfaceId::from_u128(1);
    session
        .execute(Command::PutCue {
            cue: Cue {
                id: CueId::from_u128(7),
                name: "half".into(),
                fade: Finite::ZERO,
                values: vec![CueValue {
                    param: ParamId::SurfaceOpacity(surface),
                    value: ParamValue::Float(0.5),
                }],
            },
            index: None,
        })
        .unwrap();
    send(osc, "/openmapper/transport/play", vec![]);
    send(osc, "/openmapper/cue/go", vec![]);
    pump(&mut session, &mut transport, &mut live, |_| false);
    assert!(transport.is_playing());
    let effective = live.frame(&mut session, &mut transport, Instant::now());
    assert_eq!(
        effective.surface(surface).unwrap().opacity.get(),
        0.5,
        "cue override applied"
    );
    assert_eq!(
        session.project().surface(surface).unwrap().opacity.get(),
        1.0,
        "document untouched"
    );
}

#[test]
fn servers_restart_when_ports_change() {
    let (mut session, mut transport, mut live, _, _) = setup();
    let new_osc = free_port();
    let mut controls = session.project().controls.clone();
    controls.osc_port = new_osc;
    session.execute(Command::SetControls { controls }).unwrap();
    live.frame(&mut session, &mut transport, Instant::now());
    assert_eq!(live.servers.ports().0, Some(new_osc));
    send(
        new_osc,
        "/openmapper/master/blackout",
        vec![OscType::Bool(true)],
    );
    pump(&mut session, &mut transport, &mut live, |s| {
        s.project().master.blackout
    });
    assert!(session.project().master.blackout);
}
