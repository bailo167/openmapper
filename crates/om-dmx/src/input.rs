// SPDX-License-Identifier: Apache-2.0
//! Receiving Art-Net and sACN as a control source.
//!
//! [`DmxInputs`] holds non-blocking sockets that the caller drains once a
//! frame: Art-Net on its port (broadcast or unicast) and sACN on its port,
//! joined to the multicast group of every wanted universe. The newest
//! packet for a universe wins; sources are not merged (HTP/LTP or sACN
//! priority arbitration is not implemented). Bind or join failures are
//! reported by [`DmxInputs::status`], never fatal.

use std::net::{Ipv4Addr, UdpSocket};

use crate::net::{Ports, Received, parse};
use crate::sacn;

/// Datagrams handled per [`DmxInputs::drain`] call at most, so a flood
/// cannot stall the caller's frame.
pub const MAX_PACKETS_PER_DRAIN: usize = 2048;

/// Art-Net and sACN input sockets.
#[derive(Debug)]
pub struct DmxInputs {
    artnet: Option<UdpSocket>,
    sacn: Option<UdpSocket>,
    joined: Vec<u16>,
    bind_errors: Vec<String>,
    join_error: Option<String>,
}

fn bind(port: u16) -> std::io::Result<UdpSocket> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
    s.set_nonblocking(true)?;
    Ok(s)
}

impl DmxInputs {
    /// Listens on `ports` (0 picks a free port, for tests) for the given
    /// sACN universes.
    #[must_use]
    pub fn open(ports: Ports, universes: &[u16]) -> Self {
        let mut errors = Vec::new();
        let mut open = |name: &str, port: u16| match bind(port) {
            Ok(s) => Some(s),
            Err(e) => {
                errors.push(format!("{name} port {port}: {e}"));
                None
            }
        };
        let artnet = open("Art-Net", ports.artnet);
        let sacn = open("sACN", ports.sacn);
        let mut inputs = Self {
            artnet,
            sacn,
            joined: Vec::new(),
            bind_errors: errors,
            join_error: None,
        };
        inputs.set_universes(universes);
        inputs
    }

    /// What is listening, and any problems.
    #[must_use]
    pub fn status(&self) -> String {
        [
            self.artnet_port().map(|p| format!("Art-Net :{p}")),
            self.sacn_port().map(|p| format!("sACN :{p}")),
        ]
        .into_iter()
        .flatten()
        .chain(self.bind_errors.iter().cloned())
        .chain(self.join_error.clone())
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// Bound Art-Net port, if listening.
    #[must_use]
    pub fn artnet_port(&self) -> Option<u16> {
        self.artnet.as_ref()?.local_addr().ok().map(|a| a.port())
    }

    /// Bound sACN port, if listening.
    #[must_use]
    pub fn sacn_port(&self) -> Option<u16> {
        self.sacn.as_ref()?.local_addr().ok().map(|a| a.port())
    }

    /// Joins the sACN multicast groups for `universes` and leaves the
    /// others. Join failures (no multicast route) are noted in the status;
    /// unicast sACN still arrives.
    pub fn set_universes(&mut self, universes: &[u16]) {
        let Some(s) = &self.sacn else {
            return;
        };
        let any = Ipv4Addr::UNSPECIFIED;
        for u in &self.joined {
            if !universes.contains(u) {
                let _ = s.leave_multicast_v4(&sacn::multicast_group(*u), &any);
            }
        }
        let mut failed = Vec::new();
        for u in universes.iter().filter(|u| (1..=63_999).contains(*u)) {
            if !self.joined.contains(u)
                && s.join_multicast_v4(&sacn::multicast_group(*u), &any)
                    .is_err()
            {
                failed.push(u.to_string());
            }
        }
        self.joined = universes.to_vec();
        self.join_error = (!failed.is_empty())
            .then(|| format!("sACN multicast join failed for {}", failed.join(", ")));
    }

    /// Every DMX datagram received since the last call (at most
    /// [`MAX_PACKETS_PER_DRAIN`]), in arrival order per socket.
    pub fn drain(&mut self) -> Vec<Received> {
        let mut out = Vec::new();
        let mut buf = [0u8; 1500];
        let mut budget = MAX_PACKETS_PER_DRAIN;
        for s in [&self.artnet, &self.sacn].into_iter().flatten() {
            while budget > 0 {
                let Ok((n, _)) = s.recv_from(&mut buf) else {
                    break;
                };
                budget -= 1;
                if let Some(r) = parse(&buf[..n]) {
                    out.push(r);
                }
            }
        }
        out
    }
}
