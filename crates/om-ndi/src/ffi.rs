// SPDX-License-Identifier: Apache-2.0
//! The C interface of the NDI runtime used by this adapter: the handful of
//! exported functions and plain structures listed in NDI's public
//! documentation. Only these are declared; nothing from the NDI SDK is
//! included or redistributed.

use std::ffi::{c_char, c_float, c_int, c_void};

pub type Instance = *mut c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub p_ndi_name: *const c_char,
    pub p_url_address: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FindCreate {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RecvCreateV3 {
    pub source_to_connect_to: Source,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SendCreate {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VideoFrameV2 {
    pub xres: c_int,
    pub yres: c_int,
    pub four_cc: u32,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: c_float,
    pub frame_format_type: c_int,
    pub timecode: i64,
    pub p_data: *mut u8,
    /// Bytes per row for uncompressed formats.
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for VideoFrameV2 {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            four_cc: 0,
            frame_rate_n: 0,
            frame_rate_d: 0,
            picture_aspect_ratio: 0.0,
            frame_format_type: 0,
            timecode: 0,
            p_data: std::ptr::null_mut(),
            line_stride_in_bytes: 0,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

const fn four_cc(c: [u8; 4]) -> u32 {
    u32::from_le_bytes(c)
}

pub const FOURCC_RGBA: u32 = four_cc(*b"RGBA");
pub const FOURCC_RGBX: u32 = four_cc(*b"RGBX");
pub const FOURCC_BGRA: u32 = four_cc(*b"BGRA");
pub const FOURCC_BGRX: u32 = four_cc(*b"BGRX");

pub const FRAME_TYPE_VIDEO: c_int = 1;
pub const FRAME_TYPE_ERROR: c_int = 4;

/// Receive as RGBX when opaque, RGBA otherwise.
pub const RECV_COLOR_RGBX_RGBA: c_int = 2;
pub const RECV_BANDWIDTH_HIGHEST: c_int = 100;
pub const FRAME_FORMAT_PROGRESSIVE: c_int = 1;
/// Let the runtime fill in the timecode.
pub const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

pub type FnInitialize = unsafe extern "C" fn() -> bool;
pub type FnFindCreate = unsafe extern "C" fn(*const FindCreate) -> Instance;
pub type FnFindDestroy = unsafe extern "C" fn(Instance);
pub type FnFindWait = unsafe extern "C" fn(Instance, u32) -> bool;
pub type FnFindSources = unsafe extern "C" fn(Instance, *mut u32) -> *const Source;
pub type FnRecvCreate = unsafe extern "C" fn(*const RecvCreateV3) -> Instance;
pub type FnRecvDestroy = unsafe extern "C" fn(Instance);
pub type FnRecvCapture =
    unsafe extern "C" fn(Instance, *mut VideoFrameV2, *mut c_void, *mut c_void, u32) -> c_int;
pub type FnRecvFreeVideo = unsafe extern "C" fn(Instance, *const VideoFrameV2);
pub type FnSendCreate = unsafe extern "C" fn(*const SendCreate) -> Instance;
pub type FnSendDestroy = unsafe extern "C" fn(Instance);
pub type FnSendVideo = unsafe extern "C" fn(Instance, *const VideoFrameV2);

#[cfg(test)]
mod tests {
    use super::*;

    /// The layouts the runtime expects on 64-bit platforms.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn struct_layouts() {
        assert_eq!(size_of::<Source>(), 16);
        assert_eq!(size_of::<FindCreate>(), 24);
        assert_eq!(size_of::<RecvCreateV3>(), 40);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, p_ndi_recv_name), 32);
        assert_eq!(size_of::<SendCreate>(), 24);
        assert_eq!(std::mem::offset_of!(SendCreate, clock_audio), 17);
        assert_eq!(size_of::<VideoFrameV2>(), 72);
        assert_eq!(std::mem::offset_of!(VideoFrameV2, timecode), 32);
        assert_eq!(std::mem::offset_of!(VideoFrameV2, p_data), 40);
        assert_eq!(std::mem::offset_of!(VideoFrameV2, line_stride_in_bytes), 48);
        assert_eq!(std::mem::offset_of!(VideoFrameV2, p_metadata), 56);
        assert_eq!(std::mem::offset_of!(VideoFrameV2, timestamp), 64);
    }

    #[test]
    fn four_ccs_are_little_endian_codes() {
        assert_eq!(FOURCC_RGBA, 0x4142_4752);
        assert_eq!(FOURCC_BGRX & 0xff, u32::from(b'B'));
    }
}
