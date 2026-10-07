// SPDX-License-Identifier: Apache-2.0
//! The platform-independent parts of the Spout 2 protocol: the shared
//! sender-name list, the per-sender texture description and pixel-format
//! conversion. Layouts follow the Spout 2 SDK (BSD-2-Clause; see
//! docs/live-io.md).

use std::collections::BTreeSet;

/// Bytes per entry in the sender-name list (a NUL-terminated name).
pub const NAME_LEN: usize = 256;

/// Default capacity of the sender-name list (overridable per user in the
/// registry value `HKCU\Software\Leading Edge\Spout\MaxSenders`).
pub const DEFAULT_MAX_SENDERS: usize = 64;

/// Size of the per-sender description block.
pub const INFO_LEN: usize = 280;

/// Name of the shared sender-name list.
pub const NAMES_MAP: &str = "SpoutSenderNames";

/// Name of the shared "active sender" slot.
pub const ACTIVE_MAP: &str = "ActiveSenderName";

/// `partnerId` bit: the sender shares through the CPU, not a texture.
pub const PARTNER_CPU: u32 = 0x8000_0000;

/// Mutex guarding a shared-memory block named `map`.
#[must_use]
pub fn map_mutex(map: &str) -> String {
    format!("{map}_mutex")
}

/// Mutex guarding a sender's texture.
#[must_use]
pub fn access_mutex(sender: &str) -> String {
    format!("{sender}_SpoutAccessMutex")
}

/// Semaphore counting a sender's frames.
#[must_use]
pub fn frame_semaphore(sender: &str) -> String {
    format!("{sender}_Count_Semaphore")
}

/// Checks that `name` can be a Spout sender name.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Spout sender name is empty".into());
    }
    if name.len() >= NAME_LEN {
        return Err(format!(
            "Spout sender name is longer than {} bytes",
            NAME_LEN - 1
        ));
    }
    if name.bytes().any(|b| b == 0) {
        return Err("Spout sender name contains a NUL byte".into());
    }
    // A backslash would address another kernel object namespace
    // (`Global\…`), and the reserved names are the shared registry blocks
    // themselves (a sender would overwrite the machine-wide list).
    if name.contains('\\') {
        return Err("Spout sender name contains a backslash".into());
    }
    if [NAMES_MAP, ACTIVE_MAP]
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
    {
        return Err(format!("{name:?} is reserved by Spout"));
    }
    Ok(())
}

/// Reads the sender-name list: consecutive [`NAME_LEN`]-byte slots, ended
/// by an empty slot or the end of the block.
#[must_use]
pub fn read_names(block: &[u8]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for slot in block.as_chunks::<NAME_LEN>().0 {
        let len = slot.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        if len == 0 {
            break;
        }
        names.insert(String::from_utf8_lossy(&slot[..len]).into_owned());
    }
    names
}

/// Writes `names` in sorted order (as other Spout applications do),
/// zeroing the rest of the block. Names that do not fit are left out.
pub fn write_names(names: &BTreeSet<String>, block: &mut [u8]) {
    block.fill(0);
    for (name, slot) in names.iter().zip(block.as_chunks_mut::<NAME_LEN>().0) {
        let n = name.len().min(NAME_LEN - 1);
        slot[..n].copy_from_slice(&name.as_bytes()[..n]);
    }
}

/// Reads a NUL-terminated name from a block.
#[must_use]
pub fn read_name(block: &[u8]) -> String {
    let len = block.iter().position(|&b| b == 0).unwrap_or(block.len());
    String::from_utf8_lossy(&block[..len]).into_owned()
}

/// Writes a NUL-terminated name into a block (truncated to fit).
pub fn write_name(name: &str, block: &mut [u8]) {
    block.fill(0);
    let n = name.len().min(block.len().saturating_sub(1));
    block[..n].copy_from_slice(&name.as_bytes()[..n]);
}

/// A sender's description block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInfo {
    /// Legacy (32-bit) DXGI shared handle of the sender's texture.
    pub share_handle: u32,
    pub width: u32,
    pub height: u32,
    /// DXGI format (0 and D3D9 formats mean BGRA).
    pub format: u32,
    pub usage: u32,
    /// Path of the sending program (bytes).
    pub description: [u8; 256],
    pub partner_id: u32,
}

impl Default for TextureInfo {
    fn default() -> Self {
        Self {
            share_handle: 0,
            width: 0,
            height: 0,
            format: 0,
            usage: 0,
            description: [0; 256],
            partner_id: 0,
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut w = [0u8; 4];
    w.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(w)
}

impl TextureInfo {
    /// Parses a block of at least [`INFO_LEN`] bytes.
    #[must_use]
    pub fn read(block: &[u8]) -> Option<Self> {
        if block.len() < INFO_LEN {
            return None;
        }
        let mut description = [0u8; 256];
        description.copy_from_slice(&block[20..276]);
        Some(Self {
            share_handle: u32_at(block, 0),
            width: u32_at(block, 4),
            height: u32_at(block, 8),
            format: u32_at(block, 12),
            usage: u32_at(block, 16),
            description,
            partner_id: u32_at(block, 276),
        })
    }

    /// Writes into a block of at least [`INFO_LEN`] bytes.
    pub fn write(&self, block: &mut [u8]) {
        if block.len() < INFO_LEN {
            return;
        }
        block[0..4].copy_from_slice(&self.share_handle.to_le_bytes());
        block[4..8].copy_from_slice(&self.width.to_le_bytes());
        block[8..12].copy_from_slice(&self.height.to_le_bytes());
        block[12..16].copy_from_slice(&self.format.to_le_bytes());
        block[16..20].copy_from_slice(&self.usage.to_le_bytes());
        block[20..276].copy_from_slice(&self.description);
        block[276..280].copy_from_slice(&self.partner_id.to_le_bytes());
    }
}

/// DXGI formats a Spout texture may use.
pub mod dxgi {
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8X8_UNORM: u32 = 88;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const B8G8R8X8_UNORM_SRGB: u32 = 93;
    /// Direct3D 9 `A8R8G8B8` / `X8R8G8B8`, written by older senders.
    pub const D3D9_A8R8G8B8: u32 = 21;
    pub const D3D9_X8R8G8B8: u32 = 22;
}

/// Bytes per pixel of a supported format, or `None` if unsupported.
#[must_use]
pub fn bytes_per_pixel(format: u32) -> Option<usize> {
    use dxgi::*;
    match format {
        0 | D3D9_A8R8G8B8 | D3D9_X8R8G8B8 | R10G10B10A2_UNORM | R8G8B8A8_UNORM
        | R8G8B8A8_UNORM_SRGB | B8G8R8A8_UNORM | B8G8R8X8_UNORM | B8G8R8A8_UNORM_SRGB
        | B8G8R8X8_UNORM_SRGB => Some(4),
        R16G16B16A16_FLOAT | R16G16B16A16_UNORM => Some(8),
        R32G32B32A32_FLOAT => Some(16),
        _ => None,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn unit_to_u8(v: f32) -> u8 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Converts one row of `format` pixels into straight-alpha RGBA8 (`out`
/// holds `width * 4` bytes). Returns false for unsupported formats.
///
/// Float and 16-bit formats are taken as already display-encoded values in
/// 0..1, as Spout senders produce them.
#[must_use]
pub fn row_to_rgba8(format: u32, row: &[u8], out: &mut [u8]) -> bool {
    use dxgi::*;
    let Some(bpp) = bytes_per_pixel(format) else {
        return false;
    };
    let width = out.len() / 4;
    if row.len() < width * bpp {
        return false;
    }
    let opaque = matches!(format, B8G8R8X8_UNORM | B8G8R8X8_UNORM_SRGB | D3D9_X8R8G8B8);
    for (src, dst) in row.chunks_exact(bpp).zip(out.as_chunks_mut::<4>().0) {
        match format {
            R8G8B8A8_UNORM | R8G8B8A8_UNORM_SRGB => dst.copy_from_slice(src),
            R10G10B10A2_UNORM => {
                let w = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
                #[allow(clippy::cast_precision_loss)]
                let c = |shift: u32| unit_to_u8(((w >> shift) & 0x3ff) as f32 / 1023.0);
                dst[0] = c(0);
                dst[1] = c(10);
                dst[2] = c(20);
                dst[3] = unit_to_u8(f32::from(u8::try_from(w >> 30).unwrap_or(3)) / 3.0);
            }
            R16G16B16A16_FLOAT => {
                for (i, d) in dst.iter_mut().enumerate() {
                    let h = half::f16::from_le_bytes([src[2 * i], src[2 * i + 1]]);
                    *d = unit_to_u8(h.to_f32());
                }
            }
            R16G16B16A16_UNORM => {
                for (i, d) in dst.iter_mut().enumerate() {
                    let v = u16::from_le_bytes([src[2 * i], src[2 * i + 1]]);
                    *d = unit_to_u8(f32::from(v) / 65535.0);
                }
            }
            R32G32B32A32_FLOAT => {
                for (i, d) in dst.iter_mut().enumerate() {
                    let b = [src[4 * i], src[4 * i + 1], src[4 * i + 2], src[4 * i + 3]];
                    *d = unit_to_u8(f32::from_le_bytes(b));
                }
            }
            // BGRA byte order (DXGI B8G8R8A8 and D3D9 A8R8G8B8 alike).
            _ => {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = if opaque { 255 } else { src[3] };
            }
        }
    }
    true
}

/// Converts straight-alpha RGBA8 to BGRA8 (the format OpenMapper sends).
pub fn rgba8_to_bgra8(rgba: &[u8], out: &mut [u8]) {
    for (s, d) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
    {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn name_list_round_trips_sorted_and_bounded() {
        let mut block = vec![0xAAu8; NAME_LEN * 3];
        let names: BTreeSet<String> = ["zeta", "Alpha", "mid dle"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        write_names(&names, &mut block);
        assert_eq!(&block[..6], b"Alpha\0", "sorted byte-wise, like the SDK");
        assert_eq!(read_names(&block), names);

        let mut four = names.clone();
        four.insert("extra".into());
        write_names(&four, &mut block);
        assert_eq!(read_names(&block).len(), 3, "never overflows the block");

        // An empty first slot ends the list; trailing garbage is ignored.
        let mut block = vec![0u8; NAME_LEN * 2];
        block[NAME_LEN] = b'x';
        assert!(read_names(&block).is_empty());
    }

    #[test]
    fn unterminated_name_slot_is_bounded() {
        let block = vec![b'a'; NAME_LEN];
        let names = read_names(&block);
        assert_eq!(names.iter().next().unwrap().len(), NAME_LEN);
    }

    #[test]
    fn texture_info_layout_matches_the_sdk() {
        let mut info = TextureInfo {
            share_handle: 0x1234_5678,
            width: 1920,
            height: 1080,
            format: dxgi::B8G8R8A8_UNORM,
            usage: 0,
            partner_id: PARTNER_CPU,
            ..TextureInfo::default()
        };
        info.description[..4].copy_from_slice(b"C:\\a");
        let mut block = [0u8; INFO_LEN];
        info.write(&mut block);
        assert_eq!(&block[0..4], &[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(u32_at(&block, 4), 1920);
        assert_eq!(u32_at(&block, 12), 87);
        assert_eq!(&block[20..24], b"C:\\a");
        assert_eq!(u32_at(&block, 276), PARTNER_CPU);
        assert_eq!(TextureInfo::read(&block), Some(info));
        assert_eq!(TextureInfo::read(&block[..279]), None);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("OpenMapper").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(255)).is_ok());
        assert!(validate_name(&"a".repeat(256)).is_err());
        assert!(validate_name("a\0b").is_err());
        assert!(validate_name("Global\\x").is_err(), "no namespaces");
        assert!(validate_name("SpoutSenderNames").is_err(), "reserved");
        assert!(validate_name("activesendername").is_err(), "reserved");
        assert_eq!(access_mutex("X"), "X_SpoutAccessMutex");
        assert_eq!(frame_semaphore("X"), "X_Count_Semaphore");
        assert_eq!(map_mutex(NAMES_MAP), "SpoutSenderNames_mutex");
    }

    #[test]
    fn pixel_formats_convert_to_rgba8() {
        let mut out = [0u8; 4];
        assert!(row_to_rgba8(dxgi::B8G8R8A8_UNORM, &[1, 2, 3, 4], &mut out));
        assert_eq!(out, [3, 2, 1, 4]);
        assert!(row_to_rgba8(dxgi::B8G8R8X8_UNORM, &[1, 2, 3, 4], &mut out));
        assert_eq!(out, [3, 2, 1, 255]);
        assert!(row_to_rgba8(0, &[1, 2, 3, 4], &mut out));
        assert_eq!(out, [3, 2, 1, 4]);
        assert!(row_to_rgba8(dxgi::R8G8B8A8_UNORM, &[1, 2, 3, 4], &mut out));
        assert_eq!(out, [1, 2, 3, 4]);

        let w: u32 = 1023 | (512 << 10) | (3 << 30); // blue = 0
        assert!(row_to_rgba8(
            dxgi::R10G10B10A2_UNORM,
            &w.to_le_bytes(),
            &mut out
        ));
        assert_eq!(out, [255, 128, 0, 255]);

        let mut h = Vec::new();
        for v in [1.0f32, 0.5, 0.0, 2.0] {
            h.extend_from_slice(&half::f16::from_f32(v).to_le_bytes());
        }
        assert!(row_to_rgba8(dxgi::R16G16B16A16_FLOAT, &h, &mut out));
        assert_eq!(out, [255, 128, 0, 255], "out-of-range values clamp");

        let mut f = Vec::new();
        for v in [0.0f32, f32::NAN, 1.0, -1.0] {
            f.extend_from_slice(&v.to_le_bytes());
        }
        assert!(row_to_rgba8(dxgi::R32G32B32A32_FLOAT, &f, &mut out));
        assert_eq!(out, [0, 0, 255, 0]);

        let mut u = Vec::new();
        for v in [65535u16, 0, 32768, 65535] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        assert!(row_to_rgba8(dxgi::R16G16B16A16_UNORM, &u, &mut out));
        assert_eq!(out, [255, 0, 128, 255]);

        assert!(!row_to_rgba8(9999, &[0; 4], &mut out));
        assert!(!row_to_rgba8(dxgi::R32G32B32A32_FLOAT, &[0; 4], &mut out));
    }

    #[test]
    fn rgba_to_bgra_swaps_red_and_blue() {
        let mut out = [0u8; 8];
        rgba8_to_bgra8(&[1, 2, 3, 4, 5, 6, 7, 8], &mut out);
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8]);
    }
}
