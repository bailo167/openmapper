// SPDX-License-Identifier: Apache-2.0

use super::*;
use om_types::{CueId, SurfaceId, TimelineId};

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

#[test]
fn parameter_addresses_round_trip() {
    let p = ParamId::SurfaceOpacity(SurfaceId::from_u128(5));
    let a = address(&p);
    assert_eq!(a, "/openmapper/surface/00000000000000000000000005/opacity");
    assert_eq!(
        translate(&msg(&a, vec![OscType::Float(0.25)])).unwrap(),
        ControlMessage::Set {
            param: p.clone(),
            value: ParamValue::Float(0.25)
        }
    );
    assert_eq!(
        translate(&msg(&a, vec![OscType::Int(1)])).unwrap(),
        ControlMessage::Set {
            param: p,
            value: ParamValue::Float(1.0)
        }
    );
    assert_eq!(
        translate(&msg(
            "/openmapper/master/blackout",
            vec![OscType::Bool(true)]
        ))
        .unwrap(),
        ControlMessage::Set {
            param: ParamId::MasterBlackout,
            value: ParamValue::Bool(true)
        }
    );
}

#[test]
fn actions() {
    let c = CueId::from_u128(3);
    let t = TimelineId::from_u128(4);
    let a = |s: &str, args| translate(&msg(s, args)).unwrap();
    assert_eq!(
        a("/openmapper/transport/play", vec![]),
        ControlMessage::Action(Action::Play)
    );
    assert_eq!(
        a("/openmapper/cue/go", vec![]),
        ControlMessage::Action(Action::CueGoNext)
    );
    assert_eq!(
        a(&format!("/openmapper/cue/{c}/go"), vec![]),
        ControlMessage::Action(Action::CueGo(c))
    );
    assert_eq!(
        a("/openmapper/cue/release", vec![OscType::Double(1.5)]),
        ControlMessage::Action(Action::CueRelease { fade: 1.5 })
    );
    assert_eq!(
        a(
            &format!("/openmapper/timeline/{t}/seek"),
            vec![OscType::Float(2.0)]
        ),
        ControlMessage::Action(Action::TimelineSeek(t, 2.0))
    );
}

#[test]
fn bad_input_is_rejected_deterministically() {
    for (addr, args) in [
        ("/other/thing", vec![OscType::Float(1.0)]),
        (
            "/openmapper/surface/nope/opacity",
            vec![OscType::Float(1.0)],
        ),
        (
            "/openmapper/timeline/00000000000000000000000004/dance",
            vec![],
        ),
    ] {
        assert!(
            matches!(
                translate(&msg(addr, args)),
                Err(OscError::UnknownAddress(_))
            ),
            "{addr}"
        );
    }
    assert!(matches!(
        translate(&msg(
            "/openmapper/master/opacity",
            vec![OscType::String("x".into())]
        )),
        Err(OscError::BadArguments { .. })
    ));
    assert!(matches!(
        translate(&msg(
            "/openmapper/master/opacity",
            vec![OscType::Float(f32::NAN)]
        )),
        Err(OscError::BadArguments { .. })
    ));
    assert!(matches!(decode(b"garbage")[..], [Err(OscError::Decode(_))]));
}

#[test]
fn server_receives_udp_and_restarts_on_same_port() {
    let server = OscServer::start(0).unwrap();
    let port = server.port();
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let packet = rosc::encoder::encode(&OscPacket::Bundle(rosc::OscBundle {
        timetag: rosc::OscTime {
            seconds: 0,
            fractional: 1,
        },
        content: vec![
            OscPacket::Message(msg("/openmapper/transport/play", vec![])),
            OscPacket::Message(msg("/openmapper/master/opacity", vec![OscType::Float(0.5)])),
        ],
    }))
    .unwrap();
    sender.send_to(&packet, ("127.0.0.1", port)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got.len() < 2 && std::time::Instant::now() < deadline {
        got.extend(server.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0], Ok(ControlMessage::Action(Action::Play)));
    // Rebind the same port after stopping (service restart).
    drop(server);
    let again = OscServer::start(port).unwrap();
    assert_eq!(again.port(), port);
}
