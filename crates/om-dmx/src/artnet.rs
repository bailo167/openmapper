// SPDX-License-Identifier: Apache-2.0
//! Art-Net 4 packets (public specification, Artistic Licence): `ArtDmx`,
//! `ArtSync`, `ArtPoll` and `ArtPollReply`. Multi-byte fields are
//! little-endian except where the specification says otherwise (protocol
//! version and DMX length are big-endian).

use std::net::Ipv4Addr;

/// UDP port for all Art-Net traffic.
pub const PORT: u16 = 6454;

const ID: &[u8; 8] = b"Art-Net\0";
const PROTOCOL_VERSION: u16 = 14;

pub const OP_POLL: u16 = 0x2000;
pub const OP_POLL_REPLY: u16 = 0x2100;
pub const OP_DMX: u16 = 0x5000;
pub const OP_SYNC: u16 = 0x5200;

/// Largest 15-bit Port-Address (Net 0–127, Sub-Net 0–15, Universe 0–15).
pub const MAX_PORT_ADDRESS: u16 = 0x7fff;

/// Why a datagram is not a usable Art-Net packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("not an Art-Net packet")]
    NotArtNet,
    #[error("Art-Net packet too short")]
    Truncated,
    #[error("unsupported Art-Net opcode {0:#06x}")]
    Unsupported(u16),
    #[error("Art-Net protocol version {0} is too old")]
    OldVersion(u16),
    #[error("Art-Net DMX length {0} is invalid")]
    BadLength(usize),
}

/// The opcode of an Art-Net datagram, if it is one.
#[must_use]
pub fn opcode(packet: &[u8]) -> Option<u16> {
    if packet.len() < 10 || &packet[..8] != ID {
        return None;
    }
    Some(u16::from_le_bytes([packet[8], packet[9]]))
}

fn header(op: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(ID);
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
}

/// Encodes an `ArtDmx` packet for 15-bit `port_address`. `sequence` 0
/// disables reordering on the receiver; senders use 1–255. Data longer than
/// 512 slots is truncated; an odd or short length is padded with zeros to
/// an even length of at least 2, as the specification requires.
#[must_use]
pub fn dmx(port_address: u16, sequence: u8, physical: u8, data: &[u8]) -> Vec<u8> {
    let slots = &data[..data.len().min(512)];
    let len = (slots.len().max(2) + 1) & !1;
    let mut out = Vec::with_capacity(18 + len);
    header(OP_DMX, &mut out);
    out.push(sequence);
    out.push(physical);
    let pa = port_address & MAX_PORT_ADDRESS;
    out.push((pa & 0xff) as u8); // SubUni
    out.push((pa >> 8) as u8); // Net
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(slots);
    out.resize(18 + len, 0);
    out
}

/// A decoded `ArtDmx` packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dmx {
    pub sequence: u8,
    pub physical: u8,
    pub port_address: u16,
    pub data: Vec<u8>,
}

/// Decodes an `ArtDmx` packet.
pub fn parse_dmx(packet: &[u8]) -> Result<Dmx, ParseError> {
    match opcode(packet) {
        None => return Err(ParseError::NotArtNet),
        Some(OP_DMX) => {}
        Some(op) => return Err(ParseError::Unsupported(op)),
    }
    if packet.len() < 18 {
        return Err(ParseError::Truncated);
    }
    let version = u16::from_be_bytes([packet[10], packet[11]]);
    if version < PROTOCOL_VERSION {
        return Err(ParseError::OldVersion(version));
    }
    let len = usize::from(u16::from_be_bytes([packet[16], packet[17]]));
    if !(2..=512).contains(&len) {
        return Err(ParseError::BadLength(len));
    }
    let data = packet.get(18..18 + len).ok_or(ParseError::Truncated)?;
    Ok(Dmx {
        sequence: packet[12],
        physical: packet[13],
        port_address: (u16::from(packet[15] & 0x7f) << 8) | u16::from(packet[14]),
        data: data.to_vec(),
    })
}

/// Encodes an `ArtSync` packet (latch all universes sent since the last).
#[must_use]
pub fn sync() -> Vec<u8> {
    let mut out = Vec::with_capacity(14);
    header(OP_SYNC, &mut out);
    out.extend_from_slice(&[0, 0]); // Aux1, Aux2
    out
}

/// Encodes an `ArtPoll` packet asking every node to reply.
#[must_use]
pub fn poll() -> Vec<u8> {
    let mut out = Vec::with_capacity(14);
    header(OP_POLL, &mut out);
    // Flags: 0 (reply only when polled); DiagPriority: 0.
    out.extend_from_slice(&[0, 0]);
    out
}

/// What a node reports about itself in `ArtPollReply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub ip: Ipv4Addr,
    pub short_name: String,
    pub long_name: String,
    /// Port-Addresses of the node's output ports (up to 4).
    pub outputs: Vec<u16>,
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Decodes the fields of an `ArtPollReply` that identify a node.
pub fn parse_poll_reply(packet: &[u8]) -> Result<Node, ParseError> {
    match opcode(packet) {
        None => return Err(ParseError::NotArtNet),
        Some(OP_POLL_REPLY) => {}
        Some(op) => return Err(ParseError::Unsupported(op)),
    }
    if packet.len() < 194 {
        return Err(ParseError::Truncated);
    }
    let ip = Ipv4Addr::new(packet[10], packet[11], packet[12], packet[13]);
    let net = u16::from(packet[18] & 0x7f);
    let sub = u16::from(packet[19] & 0x0f);
    let ports = usize::from(packet[173]).min(4);
    let outputs = (0..ports)
        .filter(|&i| packet[174 + i] & 0x80 != 0) // port can output DMX
        .map(|i| (net << 8) | (sub << 4) | u16::from(packet[190 + i] & 0x0f))
        .collect();
    Ok(Node {
        ip,
        short_name: c_string(&packet[26..44]),
        long_name: c_string(&packet[44..108]),
        outputs,
    })
}

/// Encodes an `ArtPollReply` (used to answer polls and by tests).
#[must_use]
pub fn poll_reply(node: &Node) -> Vec<u8> {
    let mut out = vec![0u8; 239];
    out[..8].copy_from_slice(ID);
    out[8..10].copy_from_slice(&OP_POLL_REPLY.to_le_bytes());
    out[10..14].copy_from_slice(&node.ip.octets());
    out[14..16].copy_from_slice(&PORT.to_le_bytes());
    let first = node.outputs.first().copied().unwrap_or(0);
    out[18] = (first >> 8) as u8 & 0x7f;
    out[19] = (first >> 4) as u8 & 0x0f;
    let short = node.short_name.as_bytes();
    out[26..26 + short.len().min(17)].copy_from_slice(&short[..short.len().min(17)]);
    let long = node.long_name.as_bytes();
    out[44..44 + long.len().min(63)].copy_from_slice(&long[..long.len().min(63)]);
    let n = node.outputs.len().min(4);
    #[allow(clippy::cast_possible_truncation)]
    {
        out[173] = n as u8;
    }
    for (i, pa) in node.outputs.iter().take(4).enumerate() {
        out[174 + i] = 0x80; // DMX512 output
        out[182 + i] = 0x80; // data being transmitted
        out[190 + i] = (*pa & 0x0f) as u8;
    }
    out[200] = 0x00; // Style: StNode
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Byte-for-byte golden from the field layout in the Art-Net 4
    /// specification.
    #[test]
    fn artdmx_golden() {
        // Port-Address 0x1234 = Net 0x12, Sub-Net 3, Universe 4.
        let p = dmx(0x1234, 7, 1, &[10, 20, 30]);
        let expected: Vec<u8> = [
            b'A', b'r', b't', b'-', b'N', b'e', b't', 0, // ID
            0x00, 0x50, // OpDmx, little-endian
            0, 14, // protocol version, big-endian
            7,  // sequence
            1,  // physical
            0x34, 0x12, // SubUni, Net
            0, 4, // length 4 (3 padded to even), big-endian
            10, 20, 30, 0,
        ]
        .to_vec();
        assert_eq!(p, expected);
    }

    #[test]
    fn artdmx_round_trips_and_bounds_length() {
        let data: Vec<u8> = (0..600u32).map(|i| (i % 251) as u8).collect();
        let p = dmx(0x7fff, 255, 0, &data);
        assert_eq!(p.len(), 18 + 512);
        let d = parse_dmx(&p).unwrap();
        assert_eq!(d.port_address, 0x7fff);
        assert_eq!(d.sequence, 255);
        assert_eq!(d.data, data[..512]);
        assert_eq!(dmx(0, 1, 0, &[]).len(), 20, "minimum length 2");
        // The top bit of Net is not part of the Port-Address.
        assert_eq!(dmx(0xffff, 1, 0, &[1, 2])[15], 0x7f);
    }

    #[test]
    fn malformed_packets_are_rejected() {
        assert_eq!(parse_dmx(b"hello"), Err(ParseError::NotArtNet));
        assert_eq!(parse_dmx(&poll()), Err(ParseError::Unsupported(OP_POLL)));
        let mut p = dmx(1, 1, 0, &[1, 2, 3, 4]);
        p.truncate(20);
        assert_eq!(parse_dmx(&p), Err(ParseError::Truncated));
        let mut p = dmx(1, 1, 0, &[1, 2]);
        p[16] = 0x02;
        p[17] = 0x02; // 514
        assert_eq!(parse_dmx(&p), Err(ParseError::BadLength(514)));
        let mut p = dmx(1, 1, 0, &[1, 2]);
        p[11] = 13;
        assert_eq!(parse_dmx(&p), Err(ParseError::OldVersion(13)));
    }

    #[test]
    fn sync_and_poll_golden() {
        assert_eq!(
            sync(),
            [
                b'A', b'r', b't', b'-', b'N', b'e', b't', 0, 0x00, 0x52, 0, 14, 0, 0
            ]
        );
        assert_eq!(&poll()[8..12], &[0x00, 0x20, 0, 14]);
    }

    #[test]
    fn poll_reply_round_trips() {
        let node = Node {
            ip: Ipv4Addr::new(10, 0, 0, 42),
            short_name: "Pixel node".into(),
            long_name: "Test pixel controller".into(),
            outputs: vec![0x0120, 0x0121],
        };
        let r = poll_reply(&node);
        assert_eq!(parse_poll_reply(&r).unwrap(), node);
        assert_eq!(parse_poll_reply(&r[..100]), Err(ParseError::Truncated));
    }
}
