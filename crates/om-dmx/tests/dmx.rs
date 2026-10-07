// SPDX-License-Identifier: Apache-2.0
//! DMX over real UDP sockets on the loopback interface: pixel mapping end
//! to end, a compressed 30-minute stream, refresh pacing and clean stop.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use om_dmx::mapping::{FrameView, Universes};
use om_dmx::net::{DmxSender, PacketBuilder, Ports, Received, parse};
use om_dmx::{DmxRuntime, sacn};
use om_geom::Point2;
use om_project::Project;
use om_project::dmx::{
    ChannelEncoding, ColourOrder, Dmx, DmxNode, DmxProtocol, Fixture, PixelShape, Wiring,
};
use om_types::{DmxNodeId, FixtureId, UnitInterval};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap()
}

fn receiver() -> (UdpSocket, u16) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let port = s.local_addr().unwrap().port();
    (s, port)
}

fn recv(s: &UdpSocket) -> Option<Received> {
    let mut buf = [0u8; 1500];
    let (n, _) = s.recv_from(&mut buf).ok()?;
    parse(&buf[..n])
}

/// Collects the newest data per universe until `want` universes arrived.
fn collect(s: &UdpSocket, want: usize) -> BTreeMap<u16, Vec<u8>> {
    let mut got = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while got.len() < want && Instant::now() < deadline {
        if let Some(r) = recv(s) {
            got.insert(r.universe, r.data);
        }
    }
    got
}

fn node(id: u128, protocol: DmxProtocol) -> DmxNode {
    DmxNode {
        id: DmxNodeId::from_u128(id),
        name: format!("node {id}"),
        enabled: true,
        protocol,
    }
}

#[test]
fn pixel_mapping_reaches_artnet_and_sacn_nodes() {
    let (art_rx, art_port) = receiver();
    let (sacn_rx, sacn_port) = receiver();
    let art = node(
        1,
        DmxProtocol::ArtNet {
            address: "127.0.0.1".into(),
        },
    );
    let sacn_node = node(
        2,
        DmxProtocol::Sacn {
            address: "127.0.0.1".into(),
            priority: 100,
        },
    );
    let mut project = Project::new("dmx test");
    project.dmx = Dmx {
        rate: 44,
        nodes: vec![art.clone(), sacn_node.clone()],
        fixtures: vec![
            // 200 RGB pixels across the frame: 170 in universe 3, 30 in 4.
            Fixture {
                id: FixtureId::new(),
                name: "strip".into(),
                enabled: true,
                node: art.id,
                universe: 3,
                address: 1,
                order: ColourOrder::Rgb,
                encoding: ChannelEncoding::Srgb,
                brightness: UnitInterval::ONE,
                shape: PixelShape::Line {
                    from: p(0.0, 0.5),
                    to: p(1.0, 0.5),
                    count: 200,
                },
            },
            Fixture {
                id: FixtureId::new(),
                name: "matrix".into(),
                enabled: true,
                node: sacn_node.id,
                universe: 10,
                address: 1,
                order: ColourOrder::Grb,
                encoding: ChannelEncoding::Srgb,
                brightness: UnitInterval::ONE,
                shape: PixelShape::Grid {
                    corners: [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
                    columns: 2,
                    rows: 1,
                    wiring: Wiring::Rows,
                },
            },
        ],
    };
    project.validate().unwrap();
    // Left half red, right half blue.
    let (w, h) = (64u32, 8u32);
    let rgba: Vec<u8> = (0..h)
        .flat_map(|_| {
            (0..w).flat_map(|x| {
                if x < w / 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            })
        })
        .collect();
    let frame = FrameView {
        width: w,
        height: h,
        rgba8: &rgba,
    };
    let mut rt = DmxRuntime::with_ports(Ports {
        artnet: art_port,
        sacn: sacn_port,
    });
    rt.sync(&project);
    assert!(rt.is_active());
    rt.submit(&frame);

    let art_data = collect(&art_rx, 2);
    let u3 = &art_data[&3];
    assert_eq!(u3.len(), 512, "full universes are sent");
    assert_eq!(&u3[..3], &[255, 0, 0], "first pixel is red");
    assert_eq!(&u3[507..510], &[0, 0, 255], "pixel 170 is blue");
    let u4 = &art_data[&4];
    assert_eq!(&u4[87..90], &[0, 0, 255], "pixel 200 is blue");
    assert!(u4[90..].iter().all(|&b| b == 0));

    let sacn_data = collect(&sacn_rx, 1);
    let u10 = &sacn_data[&10];
    assert_eq!(u10.len(), 512);
    assert_eq!(&u10[..6], &[0, 255, 0, 0, 0, 255], "GRB: red then blue");

    // Disabling the node stops its packets.
    let stats = rt.stats().unwrap();
    assert_eq!(stats.errors, 0, "{stats:?}");
    project.dmx.nodes[0].enabled = false;
    project.dmx.nodes[1].enabled = false;
    rt.sync(&project);
    assert!(!rt.is_active());
}

/// A 30-minute stream at 44 Hz (79,200 refreshes of two universes per
/// protocol) through real sockets, compressed in time: every packet
/// arrives, in order, with continuous sequence numbers, and the sender's
/// state does not grow.
#[test]
fn thirty_minute_stream_is_continuous() {
    let (rx, port) = receiver();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = Ports {
        artnet: port,
        sacn: port,
    };
    let nodes = [
        node(
            1,
            DmxProtocol::ArtNet {
                address: "127.0.0.1".into(),
            },
        ),
        node(
            2,
            DmxProtocol::Sacn {
                address: "127.0.0.1".into(),
                priority: 100,
            },
        ),
    ];
    let mut builder = PacketBuilder::new(sacn::Source {
        cid: [1; 16],
        name: "stream".into(),
    });
    let ticks: u32 = 30 * 60 * 44;
    let mut last: BTreeMap<(&str, u16), u8> = BTreeMap::new();
    let mut received = 0u64;
    for t in 0..ticks {
        let mut data = Universes::new();
        for n in &nodes {
            for u in [1u16, 2] {
                let mut slots = [0u8; 512];
                slots[..4].copy_from_slice(&t.to_be_bytes());
                data.insert((n.id, u), slots);
            }
        }
        let packets = builder.build(&nodes, &data, ports);
        assert_eq!(packets.len(), 4);
        for (addr, bytes) in &packets {
            tx.send_to(bytes, addr).unwrap();
        }
        for _ in 0..packets.len() {
            let r = recv(&rx).expect("a packet was lost on loopback");
            assert_eq!(&r.data[..4], &t.to_be_bytes(), "packets arrive in order");
            let key = (r.protocol, r.universe);
            if let Some(prev) = last.insert(key, r.sequence) {
                let expected = match r.protocol {
                    "art-net" if prev == 255 => 1,
                    _ => prev.wrapping_add(1),
                };
                assert_eq!(r.sequence, expected, "{key:?} sequence gap at tick {t}");
            }
            received += 1;
        }
        assert_eq!(builder.tracked(), 4);
    }
    assert_eq!(received, u64::from(ticks) * 4);
}

#[test]
fn sender_paces_refreshes_and_terminates_sacn_on_drop() {
    let (rx, port) = receiver();
    let n = node(
        5,
        DmxProtocol::Sacn {
            address: "127.0.0.1".into(),
            priority: 100,
        },
    );
    let sender = DmxSender::spawn_with(
        sacn::Source {
            cid: [2; 16],
            name: "pacing".into(),
        },
        Ports {
            artnet: port,
            sacn: port,
        },
    );
    sender.configure(std::slice::from_ref(&n), 20);
    let mut data = Universes::new();
    data.insert((n.id, 7), [42; 512]);
    sender.submit(data);
    let start = Instant::now();
    let mut count = 0;
    while start.elapsed() < Duration::from_secs(1) {
        if recv(&rx).is_some_and(|r| r.universe == 7 && !r.terminated) {
            count += 1;
        }
    }
    // 20 Hz for one second, allowing for scheduling jitter on CI.
    assert!(
        (14..=26).contains(&count),
        "{count} refreshes in 1 s at 20 Hz"
    );
    let t = Instant::now();
    drop(sender);
    assert!(t.elapsed() < Duration::from_secs(1), "drop is prompt");
    let mut terminated = 0;
    while let Some(r) = recv(&rx) {
        if r.terminated {
            terminated += 1;
        }
    }
    assert_eq!(terminated, 3, "E1.31 stream termination");
}

#[test]
fn unreachable_destinations_count_errors_without_stopping() {
    let n = node(
        6,
        DmxProtocol::ArtNet {
            // TEST-NET-1: never routed. Sending may fail or vanish silently.
            address: "192.0.2.1".into(),
        },
    );
    let sender = DmxSender::spawn(sacn::Source {
        cid: [3; 16],
        name: "unreachable".into(),
    });
    sender.configure(std::slice::from_ref(&n), 44);
    let mut data = Universes::new();
    data.insert((n.id, 0), [1; 512]);
    sender.submit(data);
    std::thread::sleep(Duration::from_millis(300));
    let s = sender.stats();
    assert!(s.packets + s.errors > 0, "{s:?}");
    assert_eq!(s.universes, 1);
}

#[test]
fn input_receives_artnet_and_sacn_and_skips_junk() {
    let mut inputs = om_dmx::input::DmxInputs::open(Ports { artnet: 0, sacn: 0 }, &[7]);
    let (a, s) = (inputs.artnet_port().unwrap(), inputs.sacn_port().unwrap());
    assert!(inputs.status().contains(&format!("Art-Net :{a}")));
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    let source = sacn::Source {
        cid: [9; 16],
        name: "test".into(),
    };
    tx.send_to(
        &om_dmx::artnet::dmx(3, 1, 0, &[10, 20, 30]),
        ("127.0.0.1", a),
    )
    .unwrap();
    tx.send_to(b"not dmx at all", ("127.0.0.1", a)).unwrap();
    tx.send_to(
        &sacn::data_packet(&source, 7, 100, 1, 0, &[200; 4]),
        ("127.0.0.1", s),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got.len() < 2 && Instant::now() < deadline {
        got.extend(inputs.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    got.sort_by_key(|r| r.universe);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((got[0].protocol, got[0].universe), ("art-net", 3));
    assert_eq!(&got[0].data[..3], &[10, 20, 30]);
    assert_eq!((got[1].protocol, got[1].universe), ("sacn", 7));
    assert_eq!(&got[1].data[..4], &[200; 4]);
    inputs.set_universes(&[]);
    assert!(inputs.drain().is_empty());
}
