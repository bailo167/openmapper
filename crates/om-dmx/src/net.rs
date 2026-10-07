// SPDX-License-Identifier: Apache-2.0
//! Sending and receiving DMX over UDP.
//!
//! [`DmxSender`] runs on its own thread and sends the newest universe data
//! to every enabled node at a fixed rate (DMX receivers expect a steady
//! refresh, and sACN receivers treat 2.5 s of silence as loss). Submitting
//! never blocks. Network errors are counted, not fatal: a cable pulled
//! mid-show recovers by itself.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use om_project::dmx::{DmxNode, DmxProtocol};
use om_types::DmxNodeId;

use crate::mapping::Universes;
use crate::{artnet, sacn};

/// Destination ports (standard unless overridden for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub artnet: u16,
    pub sacn: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            artnet: artnet::PORT,
            sacn: sacn::PORT,
        }
    }
}

/// Counters for status display and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SenderStats {
    pub packets: u64,
    pub errors: u64,
    pub last_error: Option<String>,
    /// Universes currently being sent.
    pub universes: usize,
}

/// Builds packets for every universe, with per-universe sequence numbers.
#[derive(Debug)]
pub struct PacketBuilder {
    source: sacn::Source,
    sequences: BTreeMap<(DmxNodeId, u16), u8>,
}

impl PacketBuilder {
    #[must_use]
    pub fn new(source: sacn::Source) -> Self {
        Self {
            source,
            sequences: BTreeMap::new(),
        }
    }

    /// Packets for one refresh: `(destination, bytes)` for every universe
    /// of every enabled node, in node then universe order.
    pub fn build(
        &mut self,
        nodes: &[DmxNode],
        data: &Universes,
        ports: Ports,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        for ((node_id, universe), slots) in data {
            let Some(node) = nodes.iter().find(|n| n.id == *node_id && n.enabled) else {
                continue;
            };
            let seq = self.sequences.entry((*node_id, *universe)).or_insert(0);
            match &node.protocol {
                DmxProtocol::ArtNet { address } => {
                    let Ok(ip) = address.parse::<Ipv4Addr>() else {
                        continue;
                    };
                    // Art-Net sequences run 1..=255 (0 disables reordering).
                    *seq = if *seq == 255 { 1 } else { *seq + 1 };
                    out.push((
                        SocketAddr::V4(SocketAddrV4::new(ip, ports.artnet)),
                        artnet::dmx(*universe, *seq, 0, slots),
                    ));
                }
                DmxProtocol::Sacn { address, priority } => {
                    if !sacn::UNIVERSES.contains(universe) {
                        continue;
                    }
                    let ip = if address.is_empty() {
                        sacn::multicast_group(*universe)
                    } else if let Ok(ip) = address.parse() {
                        ip
                    } else {
                        continue;
                    };
                    *seq = seq.wrapping_add(1);
                    out.push((
                        SocketAddr::V4(SocketAddrV4::new(ip, ports.sacn)),
                        sacn::data_packet(&self.source, *universe, *priority, *seq, 0, slots),
                    ));
                }
            }
        }
        // Forget universes no longer sent, so state stays bounded.
        self.sequences.retain(|k, _| data.contains_key(k));
        out
    }

    /// Stream-termination packets for the sACN universes in `data`
    /// (E1.31 6.2.6: three packets with the terminated bit).
    pub fn terminate(
        &mut self,
        nodes: &[DmxNode],
        data: &Universes,
        ports: Ports,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        for ((node_id, universe), slots) in data {
            let Some(node) = nodes.iter().find(|n| n.id == *node_id && n.enabled) else {
                continue;
            };
            let DmxProtocol::Sacn { address, priority } = &node.protocol else {
                continue;
            };
            if !sacn::UNIVERSES.contains(universe) {
                continue;
            }
            let ip = if address.is_empty() {
                sacn::multicast_group(*universe)
            } else if let Ok(ip) = address.parse() {
                ip
            } else {
                continue;
            };
            let seq = self.sequences.entry((*node_id, *universe)).or_insert(0);
            for _ in 0..3 {
                *seq = seq.wrapping_add(1);
                out.push((
                    SocketAddr::V4(SocketAddrV4::new(ip, ports.sacn)),
                    sacn::data_packet(
                        &self.source,
                        *universe,
                        *priority,
                        *seq,
                        sacn::OPT_TERMINATED,
                        slots,
                    ),
                ));
            }
        }
        out
    }

    /// Number of universes with sequence state (bounded by what is sent).
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.sequences.len()
    }
}

#[derive(Default)]
struct State {
    nodes: Vec<DmxNode>,
    data: Universes,
    rate: u32,
    done: bool,
    stats: SenderStats,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Sends DMX to the network on its own thread.
pub struct DmxSender {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for DmxSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DmxSender")
    }
}

impl DmxSender {
    /// Starts sending as `source` to the standard ports.
    #[must_use]
    pub fn spawn(source: sacn::Source) -> Self {
        Self::spawn_with(source, Ports::default())
    }

    #[must_use]
    pub fn spawn_with(source: sacn::Source, ports: Ports) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                rate: 40,
                ..State::default()
            }),
            wake: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("om-dmx".into())
                .spawn(move || {
                    run(&shared, &stop, source, ports);
                    shared.lock().done = true;
                })
                .ok()
        };
        Self {
            shared,
            stop,
            thread,
        }
    }

    /// Sets the nodes and refresh rate (from the project).
    pub fn configure(&self, nodes: &[DmxNode], rate: u32) {
        let mut s = self.shared.lock();
        s.nodes = nodes.to_vec();
        s.rate = rate.clamp(1, 44);
    }

    /// Replaces the data sent from now on. Never blocks.
    pub fn submit(&self, data: Universes) {
        self.shared.lock().data = data;
    }

    #[must_use]
    pub fn stats(&self) -> SenderStats {
        self.shared.lock().stats.clone()
    }
}

impl Drop for DmxSender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.shared.lock().done && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if self.shared.lock().done {
                let _ = t.join();
            }
        }
    }
}

fn open_socket() -> std::io::Result<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    socket.set_write_timeout(Some(Duration::from_millis(100)))?;
    Ok(socket)
}

fn run(shared: &Shared, stop: &AtomicBool, source: sacn::Source, ports: Ports) {
    let mut builder = PacketBuilder::new(source);
    let mut socket: Option<UdpSocket> = None;
    let mut next = Instant::now();
    loop {
        let (nodes, data, rate) = {
            let mut s = shared.lock();
            let now = Instant::now();
            if now < next && !stop.load(Ordering::Relaxed) {
                let (guard, _) = shared
                    .wake
                    .wait_timeout(s, next - now)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                s = guard;
            }
            (s.nodes.clone(), s.data.clone(), s.rate)
        };
        let stopping = stop.load(Ordering::Relaxed);
        if !stopping && Instant::now() < next {
            continue;
        }
        let packets = if stopping {
            builder.terminate(&nodes, &data, ports)
        } else {
            builder.build(&nodes, &data, ports)
        };
        let mut sent = 0;
        let mut errors = 0;
        let mut last_error = None;
        if socket.is_none() && !packets.is_empty() {
            match open_socket() {
                Ok(s) => socket = Some(s),
                Err(e) => {
                    errors += 1;
                    last_error = Some(e.to_string());
                }
            }
        }
        if let Some(sock) = &socket {
            for (addr, bytes) in &packets {
                match sock.send_to(bytes, addr) {
                    Ok(_) => sent += 1,
                    Err(e) => {
                        errors += 1;
                        last_error = Some(format!("{addr}: {e}"));
                    }
                }
            }
        }
        {
            let mut s = shared.lock();
            s.stats.packets += sent;
            s.stats.errors += errors;
            if last_error.is_some() {
                s.stats.last_error = last_error;
            }
            s.stats.universes = data.len();
        }
        if stopping {
            return;
        }
        next += Duration::from_secs(1) / rate.max(1);
        // After a stall (suspended laptop), do not burst to catch up.
        if next + Duration::from_secs(1) < Instant::now() {
            next = Instant::now();
        }
    }
}

/// One received DMX universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub protocol: &'static str,
    pub universe: u16,
    pub sequence: u8,
    pub data: Vec<u8>,
    pub terminated: bool,
}

/// Parses an Art-Net or sACN datagram carrying DMX.
#[must_use]
pub fn parse(packet: &[u8]) -> Option<Received> {
    if let Ok(d) = artnet::parse_dmx(packet) {
        return Some(Received {
            protocol: "art-net",
            universe: d.port_address,
            sequence: d.sequence,
            data: d.data,
            terminated: false,
        });
    }
    sacn::parse_data(packet).ok().map(|d| Received {
        protocol: "sacn",
        universe: d.universe,
        sequence: d.sequence,
        terminated: d.options & sacn::OPT_TERMINATED != 0,
        data: d.data,
    })
}

/// Finds Art-Net nodes: broadcasts `ArtPoll` to `target` and collects
/// `ArtPollReply`s arriving on `bind_port` for `wait`. Nodes reply to port
/// 6454, so real discovery binds that port; it fails cleanly if another
/// program holds it.
pub fn discover_artnet(
    bind_port: u16,
    target: SocketAddr,
    wait: Duration,
) -> std::io::Result<Vec<artnet::Node>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, bind_port))?;
    socket.set_broadcast(true)?;
    socket.send_to(&artnet::poll(), target)?;
    let deadline = Instant::now() + wait;
    let mut nodes: Vec<artnet::Node> = Vec::new();
    let mut buf = [0u8; 1500];
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        socket.set_read_timeout(Some((deadline - now).max(Duration::from_millis(1))))?;
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Ok(node) = artnet::parse_poll_reply(&buf[..n])
                    && !nodes.contains(&node)
                {
                    nodes.push(node);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(nodes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn source() -> sacn::Source {
        sacn::Source {
            cid: [7; 16],
            name: "test".into(),
        }
    }

    fn node(protocol: DmxProtocol) -> DmxNode {
        DmxNode {
            id: DmxNodeId::from_u128(9),
            name: "n".into(),
            enabled: true,
            protocol,
        }
    }

    #[test]
    fn sequences_wrap_per_protocol_and_state_stays_bounded() {
        let art = node(DmxProtocol::ArtNet {
            address: "127.0.0.1".into(),
        });
        let mut data = Universes::new();
        data.insert((art.id, 3), [1; 512]);
        let mut b = PacketBuilder::new(source());
        let mut seqs = Vec::new();
        for _ in 0..600 {
            let p = b.build(std::slice::from_ref(&art), &data, Ports::default());
            assert_eq!(p.len(), 1);
            seqs.push(artnet::parse_dmx(&p[0].1).unwrap().sequence);
        }
        assert!(!seqs.contains(&0), "Art-Net never sends sequence 0");
        assert_eq!(seqs[254], 255);
        assert_eq!(seqs[255], 1);

        let s = node(DmxProtocol::Sacn {
            address: String::new(),
            priority: 100,
        });
        let mut data = Universes::new();
        data.insert((s.id, 1), [2; 512]);
        let mut last = None;
        for _ in 0..600 {
            let p = b.build(std::slice::from_ref(&s), &data, Ports::default());
            assert_eq!(p[0].0, "239.255.0.1:5568".parse().unwrap());
            let d = sacn::parse_data(&p[0].1).unwrap();
            if let Some(l) = last {
                assert!(sacn::is_newer(l, d.sequence));
            }
            last = Some(d.sequence);
        }
        assert_eq!(b.tracked(), 1, "the Art-Net universe was forgotten");
    }

    #[test]
    fn disabled_and_unknown_nodes_send_nothing() {
        let mut n = node(DmxProtocol::ArtNet {
            address: "127.0.0.1".into(),
        });
        let mut data = Universes::new();
        data.insert((n.id, 0), [0; 512]);
        data.insert((DmxNodeId::from_u128(1), 0), [0; 512]);
        let mut b = PacketBuilder::new(source());
        assert_eq!(
            b.build(std::slice::from_ref(&n), &data, Ports::default())
                .len(),
            1
        );
        n.enabled = false;
        assert!(b.build(&[n], &data, Ports::default()).is_empty());
    }

    #[test]
    fn artpoll_discovery_collects_replies() {
        // A fake node answering polls on a loopback port.
        let node_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let node_addr = node_socket.local_addr().unwrap();
        let answer = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            node_socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let (n, from) = node_socket.recv_from(&mut buf).unwrap();
            assert_eq!(artnet::opcode(&buf[..n]), Some(artnet::OP_POLL));
            let reply = artnet::poll_reply(&artnet::Node {
                ip: Ipv4Addr::LOCALHOST,
                short_name: "Fake".into(),
                long_name: "Fake pixel node".into(),
                outputs: vec![0, 1],
            });
            node_socket.send_to(&reply, from).unwrap();
            node_socket.send_to(&reply, from).unwrap(); // duplicates collapse
            node_socket.send_to(b"noise", from).unwrap();
        });
        let free = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let nodes = discover_artnet(port, node_addr, Duration::from_millis(500)).unwrap();
        answer.join().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].short_name, "Fake");
        assert_eq!(nodes[0].outputs, vec![0, 1]);
    }

    #[test]
    fn termination_sends_three_flagged_packets() {
        let s = node(DmxProtocol::Sacn {
            address: "10.1.2.3".into(),
            priority: 150,
        });
        let mut data = Universes::new();
        data.insert((s.id, 5), [9; 512]);
        let mut b = PacketBuilder::new(source());
        let p = b.terminate(&[s], &data, Ports::default());
        assert_eq!(p.len(), 3);
        for (addr, bytes) in p {
            assert_eq!(addr, "10.1.2.3:5568".parse().unwrap());
            let r = parse(&bytes).unwrap();
            assert!(r.terminated);
            assert_eq!(r.universe, 5);
        }
    }
}
