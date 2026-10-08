// SPDX-License-Identifier: Apache-2.0
//! sACN data packets (ANSI E1.31-2018, public standard): root layer,
//! framing layer and DMP layer. All multi-byte fields are big-endian.

use std::net::Ipv4Addr;

/// UDP port for sACN.
pub const PORT: u16 = 5568;

/// Valid data universes.
pub const UNIVERSES: std::ops::RangeInclusive<u16> = 1..=63999;

/// Default priority (0–200).
pub const DEFAULT_PRIORITY: u8 = 100;

const ACN_ID: [u8; 12] = [
    0x41, 0x53, 0x43, 0x2d, 0x45, 0x31, 0x2e, 0x31, 0x37, 0x00, 0x00, 0x00,
];
const VECTOR_ROOT_E131_DATA: u32 = 0x0000_0004;
const VECTOR_E131_DATA_PACKET: u32 = 0x0000_0002;
const VECTOR_DMP_SET_PROPERTY: u8 = 0x02;

/// Framing-layer option bits.
pub const OPT_PREVIEW: u8 = 0x80;
pub const OPT_TERMINATED: u8 = 0x40;

/// The multicast group for `universe` (239.255.hi.lo).
#[must_use]
pub fn multicast_group(universe: u16) -> Ipv4Addr {
    let [hi, lo] = universe.to_be_bytes();
    Ipv4Addr::new(239, 255, hi, lo)
}

/// Why a datagram is not a usable sACN data packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("not an sACN packet")]
    NotSacn,
    #[error("sACN packet too short or inconsistent")]
    Truncated,
    #[error("not an sACN data packet")]
    NotData,
    #[error("sACN universe {0} is out of range")]
    BadUniverse(u16),
    #[error("sACN start code {0:#04x} is not DMX")]
    StartCode(u8),
}

/// One sACN source's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Component identifier (a UUID, stable per installation).
    pub cid: [u8; 16],
    /// Shown by receivers (at most 63 bytes are sent).
    pub name: String,
}

fn flags_length(len: usize) -> [u8; 2] {
    // Lengths are bounded by the 512-slot maximum (638 bytes).
    #[allow(clippy::cast_possible_truncation)]
    (0x7000 | (len as u16 & 0x0fff)).to_be_bytes()
}

/// Encodes a data packet. `data` holds up to 512 slots after the start
/// code (longer input is truncated).
#[must_use]
pub fn data_packet(
    source: &Source,
    universe: u16,
    priority: u8,
    sequence: u8,
    options: u8,
    data: &[u8],
) -> Vec<u8> {
    let slots = &data[..data.len().min(512)];
    let total = 126 + slots.len();
    let mut p = Vec::with_capacity(total);
    // Root layer.
    p.extend_from_slice(&0x0010u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&ACN_ID);
    p.extend_from_slice(&flags_length(total - 16));
    p.extend_from_slice(&VECTOR_ROOT_E131_DATA.to_be_bytes());
    p.extend_from_slice(&source.cid);
    // Framing layer.
    p.extend_from_slice(&flags_length(total - 38));
    p.extend_from_slice(&VECTOR_E131_DATA_PACKET.to_be_bytes());
    let mut name = [0u8; 64];
    let n = source.name.len().min(63);
    name[..n].copy_from_slice(&source.name.as_bytes()[..n]);
    p.extend_from_slice(&name);
    p.push(priority.min(200));
    p.extend_from_slice(&0u16.to_be_bytes()); // synchronization address
    p.push(sequence);
    p.push(options);
    p.extend_from_slice(&universe.to_be_bytes());
    // DMP layer.
    p.extend_from_slice(&flags_length(total - 115));
    p.push(VECTOR_DMP_SET_PROPERTY);
    p.push(0xa1); // address and data type
    p.extend_from_slice(&0u16.to_be_bytes()); // first property address
    p.extend_from_slice(&1u16.to_be_bytes()); // address increment
    #[allow(clippy::cast_possible_truncation)]
    p.extend_from_slice(&((slots.len() + 1) as u16).to_be_bytes());
    p.push(0); // DMX start code
    p.extend_from_slice(slots);
    p
}

/// A decoded data packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    pub cid: [u8; 16],
    pub source_name: String,
    pub priority: u8,
    pub sequence: u8,
    pub options: u8,
    pub universe: u16,
    pub data: Vec<u8>,
}

fn be16(p: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([p[at], p[at + 1]])
}

fn be32(p: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]])
}

/// Decodes a data packet carrying DMX (start code 0).
pub fn parse_data(p: &[u8]) -> Result<Data, ParseError> {
    if p.len() < 16 || p[4..16] != ACN_ID || be16(p, 0) != 0x0010 {
        return Err(ParseError::NotSacn);
    }
    if p.len() < 126 {
        return Err(ParseError::Truncated);
    }
    if be32(p, 18) != VECTOR_ROOT_E131_DATA || be32(p, 40) != VECTOR_E131_DATA_PACKET {
        return Err(ParseError::NotData);
    }
    if p[117] != VECTOR_DMP_SET_PROPERTY || p[118] != 0xa1 {
        return Err(ParseError::NotData);
    }
    let count = usize::from(be16(p, 123));
    if count == 0 || count > 513 || p.len() < 125 + count {
        return Err(ParseError::Truncated);
    }
    let layer_len = |at: usize| usize::from(be16(p, at) & 0x0fff);
    if layer_len(16) + 16 != 125 + count
        || layer_len(38) + 38 != 125 + count
        || layer_len(115) + 115 != 125 + count
    {
        return Err(ParseError::Truncated);
    }
    let universe = be16(p, 113);
    if !UNIVERSES.contains(&universe) {
        return Err(ParseError::BadUniverse(universe));
    }
    if p[125] != 0 {
        return Err(ParseError::StartCode(p[125]));
    }
    let mut cid = [0u8; 16];
    cid.copy_from_slice(&p[22..38]);
    let name = &p[44..108];
    let end = name.iter().position(|&b| b == 0).unwrap_or(64);
    Ok(Data {
        cid,
        source_name: String::from_utf8_lossy(&name[..end]).into_owned(),
        priority: p[108],
        sequence: p[111],
        options: p[112],
        universe,
        data: p[126..125 + count].to_vec(),
    })
}

/// E1.31 6.7.2: whether `new` follows `last` (out-of-order packets within
/// the last 20 are discarded; anything else counts as new, including
/// wrap-around and a restarted source).
#[must_use]
pub fn is_newer(last: u8, new: u8) -> bool {
    #[allow(clippy::cast_possible_wrap)]
    let diff = new.wrapping_sub(last) as i8;
    !(-20..=0).contains(&diff)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            cid: [
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10,
            ],
            name: "OpenMapper".into(),
        }
    }

    /// Byte-for-byte golden of a full-universe packet, from the field
    /// layout in ANSI E1.31-2018 section 4 (table 4-1).
    #[test]
    fn full_universe_golden() {
        let data: Vec<u8> = (0..512u32).map(|i| (i % 256) as u8).collect();
        let p = data_packet(&source(), 0x0102, 100, 9, 0, &data);
        assert_eq!(p.len(), 638);
        assert_eq!(&p[0..4], &[0x00, 0x10, 0x00, 0x00]);
        assert_eq!(&p[4..16], b"ASC-E1.17\0\0\0");
        assert_eq!(&p[16..18], &[0x72, 0x6e], "root flags+length 0x7000|622");
        assert_eq!(&p[18..22], &[0, 0, 0, 4]);
        assert_eq!(&p[22..38], &source().cid);
        assert_eq!(&p[38..40], &[0x72, 0x58], "framing flags+length 0x7000|600");
        assert_eq!(&p[40..44], &[0, 0, 0, 2]);
        assert_eq!(&p[44..55], b"OpenMapper\0");
        assert!(p[55..108].iter().all(|&b| b == 0));
        assert_eq!(p[108], 100, "priority");
        assert_eq!(&p[109..111], &[0, 0], "sync address");
        assert_eq!(p[111], 9, "sequence");
        assert_eq!(p[112], 0, "options");
        assert_eq!(&p[113..115], &[0x01, 0x02], "universe");
        assert_eq!(&p[115..117], &[0x72, 0x0b], "DMP flags+length 0x7000|523");
        assert_eq!(&p[117..125], &[0x02, 0xa1, 0, 0, 0, 1, 0x02, 0x01]);
        assert_eq!(p[125], 0, "start code");
        assert_eq!(&p[126..], &data[..]);
    }

    #[test]
    fn packets_round_trip_at_every_length() {
        for n in [0usize, 1, 2, 3, 170, 510, 511, 512] {
            let data: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
            let p = data_packet(&source(), 63999, 200, 255, OPT_TERMINATED, &data);
            let d = parse_data(&p).unwrap();
            assert_eq!(d.data, data);
            assert_eq!(d.universe, 63999);
            assert_eq!(d.priority, 200);
            assert_eq!(d.options, OPT_TERMINATED);
            assert_eq!(d.source_name, "OpenMapper");
        }
    }

    #[test]
    fn malformed_packets_are_rejected() {
        let p = data_packet(&source(), 1, 100, 0, 0, &[1, 2, 3]);
        assert_eq!(parse_data(&p[..100]), Err(ParseError::Truncated));
        assert_eq!(parse_data(b"x"), Err(ParseError::NotSacn));
        let mut bad = p.clone();
        bad[113] = 0;
        bad[114] = 0;
        assert_eq!(parse_data(&bad), Err(ParseError::BadUniverse(0)));
        let mut bad = p.clone();
        bad[125] = 0xdd;
        assert_eq!(parse_data(&bad), Err(ParseError::StartCode(0xdd)));
        let mut bad = p.clone();
        bad[21] = 8; // extended (discovery) root vector
        assert_eq!(parse_data(&bad), Err(ParseError::NotData));
        let mut bad = p.clone();
        bad[17] ^= 1; // root length disagrees with the packet
        assert_eq!(parse_data(&bad), Err(ParseError::Truncated));
        let mut padded = p;
        padded.push(0); // trailing padding after the declared data is ignored
        assert_eq!(parse_data(&padded).unwrap().data, [1, 2, 3]);
    }

    #[test]
    fn multicast_groups() {
        assert_eq!(multicast_group(1), Ipv4Addr::new(239, 255, 0, 1));
        assert_eq!(multicast_group(63999), Ipv4Addr::new(239, 255, 249, 255));
    }

    #[test]
    fn sequence_ordering_follows_the_standard() {
        assert!(is_newer(10, 11));
        assert!(is_newer(255, 0), "wraps");
        assert!(!is_newer(10, 10), "duplicate");
        assert!(!is_newer(10, 9), "late");
        assert!(!is_newer(10, 246 /* 10 - 20 */));
        assert!(is_newer(10, 245), "far behind counts as a restart");
    }
}
