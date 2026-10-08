// SPDX-License-Identifier: Apache-2.0
//! Windows implementation: named shared memory, named mutexes and a
//! semaphore for the directory and frame counting, and legacy DXGI shared
//! textures for the pixels.
//!
//! Frames cross the CPU (upload on send, readback on receive) for now; a
//! zero-copy path into the renderer's device is a later optimisation.
#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::{CString, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use om_media_core::{FrameSink, LiveOpener, LiveSource, MediaError, SinkOpener, StillImage};
use om_project::{LiveInput, Publish};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DESC,
    D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Query, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::System::Memory::{
    CreateFileMappingA, FILE_MAP_ALL_ACCESS, MEM_COMMIT, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingA, PAGE_READWRITE, UnmapViewOfFile,
    VirtualQuery,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueA};
use windows::Win32::System::Threading::{
    CreateMutexA, CreateSemaphoreA, ReleaseMutex, ReleaseSemaphore, WaitForSingleObject,
};
use windows::core::{Interface, PCSTR, s};

use crate::format::{self, ACTIVE_MAP, INFO_LEN, NAME_LEN, NAMES_MAP, TextureInfo, dxgi};

/// How long to wait for a shared-memory or texture lock (four frames at
/// 60 Hz, as other Spout applications do).
const LOCK_MS: u32 = 67;

/// Receivers without a frame counter poll at most this often.
const UNCOUNTED_INTERVAL: Duration = Duration::from_millis(16);

fn win_err(what: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Stream(format!("Spout: {what}: {e}"))
}

fn cstring(s: &str) -> Result<CString, MediaError> {
    CString::new(s).map_err(|_| win_err("name", "contains a NUL byte"))
}

/// An owned kernel handle.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: the handle was returned by a successful Create/Open
            // call, is owned by this wrapper and is closed exactly once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

// SAFETY: kernel handles are process-wide and usable from any thread.
unsafe impl Send for Handle {}

/// Creates or opens the named mutex `name`.
fn named_mutex(name: &str) -> Result<Handle, MediaError> {
    let c = cstring(name)?;
    // SAFETY: `c` is a valid NUL-terminated string that outlives the call.
    let h = unsafe { CreateMutexA(None, false, PCSTR(c.as_ptr().cast())) }
        .map_err(|e| win_err(name, e))?;
    Ok(Handle(h))
}

/// Holds a named mutex until dropped.
struct MutexGuard<'a>(&'a Handle);

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: this thread owns the mutex (acquired in `acquire`).
        let _ = unsafe { ReleaseMutex(self.0.0) };
    }
}

/// Waits up to [`LOCK_MS`] for `mutex`. An abandoned mutex (its previous
/// owner died) is ours now and counts as acquired.
fn acquire(mutex: &Handle) -> Option<MutexGuard<'_>> {
    // SAFETY: `mutex.0` is a valid mutex handle owned by `mutex`.
    let r = unsafe { WaitForSingleObject(mutex.0, LOCK_MS) };
    (r == WAIT_OBJECT_0 || r == WAIT_ABANDONED).then_some(MutexGuard(mutex))
}

/// A named shared-memory block with its `<name>_mutex`.
struct SharedMem {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    mutex: Handle,
    _map: Handle,
}

// SAFETY: the mapped view is process memory valid until `Drop`; every
// access goes through `lock`, which serialises users with the named mutex.
unsafe impl Send for SharedMem {}

impl SharedMem {
    /// Creates `name` with `len` bytes, or opens it if it exists.
    fn create(name: &str, len: usize) -> Result<Self, MediaError> {
        let c = cstring(name)?;
        let size = u32::try_from(len).map_err(|_| win_err(name, "too large"))?;
        // SAFETY: arguments are valid; INVALID_HANDLE_VALUE requests a
        // page-file-backed mapping.
        let map = unsafe {
            CreateFileMappingA(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size,
                PCSTR(c.as_ptr().cast()),
            )
        }
        .map_err(|e| win_err(name, e))?;
        Self::finish(name, Handle(map))
    }

    /// Opens an existing block, or `None` if there is none.
    fn open(name: &str) -> Option<Self> {
        let c = CString::new(name).ok()?;
        // SAFETY: `c` is a valid NUL-terminated string.
        let map =
            unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, PCSTR(c.as_ptr().cast())) }
                .ok()?;
        Self::finish(name, Handle(map)).ok()
    }

    fn finish(name: &str, map: Handle) -> Result<Self, MediaError> {
        // SAFETY: `map` is a valid file-mapping handle; mapping 0 bytes maps
        // the whole object.
        let view = unsafe { MapViewOfFile(map.0, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        if view.Value.is_null() {
            return Err(win_err(name, "could not map shared memory"));
        }
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `view` is a live mapping; `info` is a valid out-pointer of
        // the size passed.
        let got = unsafe {
            VirtualQuery(
                Some(view.Value.cast_const()),
                &raw mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        // Another process may have created the block reserved but not
        // committed: touching it would fault.
        let len = if got == 0 || info.State != MEM_COMMIT {
            0
        } else {
            info.RegionSize
        };
        let mem = Self {
            view,
            len,
            mutex: named_mutex(&format::map_mutex(name))?,
            _map: map,
        };
        if len == 0 {
            return Err(win_err(name, "shared memory has no size"));
        }
        Ok(mem)
    }

    /// Locks the block and runs `f` on a copy of its bytes, writing back
    /// what `f` changed. Other processes can write the block at any time
    /// (the mutex only binds cooperating ones), so it is never borrowed as
    /// a Rust slice: bytes are copied with volatile reads and writes.
    fn with<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        let _guard = acquire(&self.mutex)?;
        let base = self.view.Value.cast::<u8>();
        // SAFETY: the view is `len` bytes of committed mapped memory, valid
        // for the lifetime of `self`; byte accesses need no alignment.
        let mut copy: Vec<u8> = (0..self.len)
            .map(|i| unsafe { base.add(i).read_volatile() })
            .collect();
        let before = copy.clone();
        let r = f(&mut copy);
        for (i, (new, old)) in copy.iter().zip(&before).enumerate() {
            if new != old {
                // SAFETY: as above, `i < len`.
                unsafe { base.add(i).write_volatile(*new) };
            }
        }
        Some(r)
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        // SAFETY: `view` came from MapViewOfFile and is unmapped once.
        let _ = unsafe { UnmapViewOfFile(self.view) };
    }
}

/// Largest texture received (8192 × 8192).
pub const MAX_RECEIVE_PIXELS: u64 = 8192 * 8192;

/// Capacity of the sender-name list configured for this user.
fn max_senders() -> usize {
    let mut value: u32 = 0;
    let mut size = 4u32;
    // SAFETY: out-pointers are valid for the sizes passed.
    let r = unsafe {
        RegGetValueA(
            HKEY_CURRENT_USER,
            s!("Software\\Leading Edge\\Spout"),
            s!("MaxSenders"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast::<c_void>()),
            Some(&raw mut size),
        )
    };
    if r.is_ok() && (1..=4096).contains(&value) {
        value as usize
    } else {
        format::DEFAULT_MAX_SENDERS
    }
}

/// Names of the Spout senders currently registered on this machine.
#[must_use]
pub fn sender_names() -> Vec<String> {
    SharedMem::open(NAMES_MAP)
        .and_then(|m| m.with(|b| format::read_names(b)))
        .map(|set| set.into_iter().collect())
        .unwrap_or_default()
}

/// The description of sender `name`, if it exists.
fn sender_info(name: &str) -> Option<TextureInfo> {
    SharedMem::open(name)?.with(|b| TextureInfo::read(b))?
}

/// A sender's entry in the directory, removed on drop.
struct Registration {
    name: String,
    names: SharedMem,
    info: SharedMem,
    active: Option<SharedMem>,
}

impl Registration {
    fn new(name: &str, info: &TextureInfo) -> Result<Self, MediaError> {
        let names = SharedMem::create(NAMES_MAP, max_senders() * NAME_LEN)?;
        let added = names
            .with(|block| {
                let mut set = format::read_names(block);
                if set.contains(name) && sender_info(name).is_some() {
                    return Err(win_err(
                        name,
                        "a Spout sender with this name already exists",
                    ));
                }
                set.insert(name.to_string());
                if set.len() * NAME_LEN > block.len() {
                    return Err(win_err(name, "the Spout sender list is full"));
                }
                format::write_names(&set, block);
                Ok(())
            })
            .ok_or_else(|| win_err(name, "sender list is locked"))?;
        added?;
        let remove = |names: &SharedMem| {
            names.with(|block| {
                let mut set = format::read_names(block);
                set.remove(name);
                format::write_names(&set, block);
            });
        };
        let info_mem = match SharedMem::create(name, INFO_LEN) {
            Ok(m) => m,
            Err(e) => {
                remove(&names);
                return Err(e);
            }
        };
        info_mem.with(|b| info.write(b));
        let active = SharedMem::create(ACTIVE_MAP, NAME_LEN).ok();
        if let Some(a) = &active {
            a.with(|b| {
                let current = format::read_name(b);
                if current.is_empty() || sender_info(&current).is_none() {
                    format::write_name(name, b);
                }
            });
        }
        Ok(Self {
            name: name.to_string(),
            names,
            info: info_mem,
            active,
        })
    }

    fn update(&self, info: &TextureInfo) {
        self.info.with(|b| info.write(b));
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut remaining = BTreeSet::new();
        self.names.with(|block| {
            let mut set = format::read_names(block);
            set.remove(&self.name);
            format::write_names(&set, block);
            remaining = set;
        });
        // Clear our description so receivers holding it open see us go.
        self.info.with(|b| TextureInfo::default().write(b));
        if let Some(a) = &self.active {
            a.with(|b| {
                if format::read_name(b) == self.name {
                    let next = remaining.iter().next().cloned().unwrap_or_default();
                    format::write_name(&next, b);
                }
            });
        }
    }
}

/// A Direct3D 11 device for sharing (the default adapter, or WARP).
struct Device {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
}

// SAFETY: the device and its immediate context are owned by one sender or
// receiver and used by one thread at a time (the feed's own thread).
unsafe impl Send for Device {}

impl Device {
    fn new() -> Result<Self, MediaError> {
        let drivers: [D3D_DRIVER_TYPE; 2] = [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP];
        let mut last = String::new();
        for driver in drivers {
            let mut device = None;
            let mut ctx = None;
            // SAFETY: out-pointers are valid; no adapter or software module.
            let r = unsafe {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&raw mut device),
                    None,
                    Some(&raw mut ctx),
                )
            };
            match (r, device, ctx) {
                (Ok(()), Some(device), Some(ctx)) => return Ok(Self { device, ctx }),
                (Err(e), _, _) => last = e.to_string(),
                _ => last = "no device returned".into(),
            }
        }
        Err(win_err("Direct3D 11", last))
    }

    /// Blocks until the GPU has executed everything submitted so far, so
    /// another process sees completed texture contents.
    fn finish(&self) -> Result<(), MediaError> {
        let desc = D3D11_QUERY_DESC {
            Query: D3D11_QUERY_EVENT,
            MiscFlags: 0,
        };
        let mut query: Option<ID3D11Query> = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe {
            self.device
                .CreateQuery(&raw const desc, Some(&raw mut query))
        }
        .map_err(|e| win_err("query", e))?;
        let query = query.ok_or_else(|| win_err("query", "none returned"))?;
        // SAFETY: `query` belongs to this device.
        unsafe {
            self.ctx.End(&query);
            self.ctx.Flush();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut done = windows::core::BOOL(0);
            // SAFETY: `done` is a valid BOOL out-pointer of the size passed.
            let r = unsafe {
                self.ctx
                    .GetData(&query, Some((&raw mut done).cast::<c_void>()), 4, 0)
            };
            if r.is_ok() && done.as_bool() {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(win_err("GPU", "timed out waiting for the texture update"));
            }
            std::thread::yield_now();
        }
    }
}

fn texture_desc(width: u32, height: u32, format: u32, staging: bool) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT(i32::try_from(format).unwrap_or(0)),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0).cast_unsigned()
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0.cast_unsigned()
        } else {
            0
        },
        MiscFlags: if staging {
            0
        } else {
            D3D11_RESOURCE_MISC_SHARED.0.cast_unsigned()
        },
    }
}

/// Path of this program, for the description field.
fn exe_path() -> [u8; 256] {
    let mut out = [0u8; 256];
    if let Ok(p) = std::env::current_exe() {
        let s = p.to_string_lossy();
        let n = s.len().min(255);
        out[..n].copy_from_slice(&s.as_bytes()[..n]);
    }
    out
}

struct SendTexture {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

/// Publishes frames as a Spout sender.
pub struct SpoutSender {
    name: String,
    device: Device,
    access: Handle,
    frames: Option<Handle>,
    texture: Option<SendTexture>,
    registration: Option<Registration>,
    /// Description of a new texture not yet published to receivers.
    unannounced: Option<TextureInfo>,
    scratch: Vec<u8>,
}

impl std::fmt::Debug for SpoutSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpoutSender({})", self.name)
    }
}

/// Creates or opens sender `name`'s frame counter.
fn frame_counter(name: &str) -> Option<Handle> {
    let c = CString::new(format::frame_semaphore(name)).ok()?;
    // SAFETY: valid NUL-terminated name; counts are in range.
    unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR(c.as_ptr().cast())) }
        .ok()
        .map(Handle)
}

impl SpoutSender {
    /// Prepares sender `name`; it is announced with the first frame.
    pub fn new(name: &str) -> Result<Self, MediaError> {
        format::validate_name(name).map_err(|e| win_err(name, e))?;
        if sender_info(name).is_some() && sender_names().iter().any(|n| n == name) {
            return Err(win_err(
                name,
                "a Spout sender with this name already exists",
            ));
        }
        Ok(Self {
            name: name.to_string(),
            device: Device::new()?,
            access: named_mutex(&format::access_mutex(name))?,
            frames: frame_counter(name),
            texture: None,
            registration: None,
            unannounced: None,
            scratch: Vec::new(),
        })
    }

    fn ensure_texture(&mut self, width: u32, height: u32) -> Result<(), MediaError> {
        if self
            .texture
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height)
        {
            return Ok(());
        }
        let desc = texture_desc(width, height, dxgi::B8G8R8A8_UNORM, false);
        let mut texture = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe {
            self.device
                .device
                .CreateTexture2D(&raw const desc, None, Some(&raw mut texture))
        }
        .map_err(|e| win_err("texture", e))?;
        let texture = texture.ok_or_else(|| win_err("texture", "none returned"))?;
        let resource: IDXGIResource = texture.cast().map_err(|e| win_err("texture", e))?;
        // SAFETY: the texture was created with D3D11_RESOURCE_MISC_SHARED.
        let shared = unsafe { resource.GetSharedHandle() }.map_err(|e| win_err("share", e))?;
        // Legacy shared handles are 32-bit values (HandleToLong).
        #[allow(clippy::cast_possible_truncation)]
        let handle = shared.0 as usize as u32;
        let info = TextureInfo {
            share_handle: handle,
            width,
            height,
            format: dxgi::B8G8R8A8_UNORM,
            usage: 0,
            description: exe_path(),
            partner_id: 0,
        };
        // Announced only once it holds a frame (see `send`), so receivers
        // never copy an unwritten texture.
        self.unannounced = Some(info);
        self.texture = Some(SendTexture {
            texture,
            width,
            height,
        });
        Ok(())
    }
}

impl FrameSink for SpoutSender {
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError> {
        let (w, h) = (image.width(), image.height());
        self.ensure_texture(w, h)?;
        self.scratch.resize(image.rgba8().len(), 0);
        format::rgba8_to_bgra8(image.rgba8(), &mut self.scratch);
        let Some(tex) = &self.texture else {
            return Ok(());
        };
        // A receiver holding the texture for longer than a few frames:
        // skip this frame rather than stall the output.
        let Some(_guard) = acquire(&self.access) else {
            return Ok(());
        };
        // SAFETY: `scratch` holds w*h BGRA pixels with row pitch w*4; the
        // texture is w×h BGRA8 on this device.
        unsafe {
            self.device.ctx.UpdateSubresource(
                &tex.texture,
                0,
                None,
                self.scratch.as_ptr().cast::<c_void>(),
                w * 4,
                0,
            );
        }
        self.device.finish()?;
        drop(_guard);
        if let Some(info) = self.unannounced.take() {
            match &self.registration {
                Some(r) => r.update(&info),
                None => self.registration = Some(Registration::new(&self.name, &info)?),
            }
        }
        if let Some(sem) = &self.frames {
            // Net +1: take one, give two (the Spout frame-count protocol).
            // SAFETY: `sem` is a valid semaphore handle.
            unsafe {
                if WaitForSingleObject(sem.0, 0) == WAIT_OBJECT_0 {
                    let _ = ReleaseSemaphore(sem.0, 2, None);
                }
            }
        }
        Ok(())
    }

    fn describe(&self) -> String {
        match &self.texture {
            Some(t) => format!("Spout \"{}\" {}×{} BGRA", self.name, t.width, t.height),
            None => format!("Spout \"{}\"", self.name),
        }
    }
}

struct RecvTexture {
    info: TextureInfo,
    shared: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

/// Receives frames from a Spout sender.
pub struct SpoutReceiver {
    name: String,
    device: Device,
    info: SharedMem,
    names: SharedMem,
    access: Handle,
    frames: Option<Handle>,
    texture: Option<RecvTexture>,
    last_count: Option<i32>,
    last_copy: Option<Instant>,
}

impl std::fmt::Debug for SpoutReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpoutReceiver({})", self.name)
    }
}

impl SpoutReceiver {
    /// Connects to sender `name` (it must be announced).
    pub fn new(name: &str) -> Result<Self, MediaError> {
        format::validate_name(name).map_err(|e| win_err(name, e))?;
        let missing = || MediaError::Open {
            path: format!("Spout \"{name}\""),
            message: "no Spout sender with this name is running".into(),
        };
        let names = SharedMem::open(NAMES_MAP).ok_or_else(missing)?;
        let listed = names
            .with(|b| format::read_names(b).contains(name))
            .unwrap_or(false);
        if !listed {
            return Err(missing());
        }
        let info = SharedMem::open(name).ok_or_else(missing)?;
        Ok(Self {
            name: name.to_string(),
            device: Device::new()?,
            info,
            names,
            access: named_mutex(&format::access_mutex(name))?,
            frames: frame_counter(name),
            texture: None,
            last_count: None,
            last_copy: None,
        })
    }

    fn still_listed(&self) -> bool {
        // A locked list (another program is editing it) counts as present.
        self.names
            .with(|b| format::read_names(b).contains(&self.name))
            .unwrap_or(true)
    }

    /// The sender's frame count, or `None` if it does not count frames.
    fn frame_count(&self) -> Option<i32> {
        let sem = self.frames.as_ref()?;
        let mut previous = 0i32;
        // SAFETY: `sem` is a valid semaphore handle; `previous` is a valid
        // out-pointer. Take one and give it back, reading the count.
        unsafe {
            if WaitForSingleObject(sem.0, 0) != WAIT_OBJECT_0 {
                return None;
            }
            ReleaseSemaphore(sem.0, 1, Some(&raw mut previous)).ok()?;
        }
        (previous > 0).then_some(previous)
    }

    fn open_texture(&mut self, info: &TextureInfo) -> Result<(), MediaError> {
        if self.texture.as_ref().is_some_and(|t| t.info == *info) {
            return Ok(());
        }
        self.texture = None;
        if format::bytes_per_pixel(info.format).is_none() {
            return Err(win_err(
                &self.name,
                format!("unsupported texture format {}", info.format),
            ));
        }
        // LongToHandle: the 32-bit value sign-extends.
        #[allow(clippy::cast_possible_wrap)]
        let handle = HANDLE(info.share_handle as i32 as isize as *mut c_void);
        let mut shared: Option<ID3D11Texture2D> = None;
        // SAFETY: `handle` is a shared-resource handle published by the
        // sender; the runtime validates it and fails cleanly if stale.
        unsafe {
            self.device
                .device
                .OpenSharedResource(handle, &raw mut shared)
        }
        .map_err(|e| win_err("open shared texture", e))?;
        let shared = shared.ok_or_else(|| win_err("open shared texture", "none returned"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-pointer.
        unsafe { shared.GetDesc(&raw mut desc) };
        // Sized by another process: refuse what would force huge copies.
        if u64::from(desc.Width) * u64::from(desc.Height) > MAX_RECEIVE_PIXELS {
            return Err(win_err(
                &self.name,
                format!("texture {}x{} is too large", desc.Width, desc.Height),
            ));
        }
        let staging_desc =
            texture_desc(desc.Width, desc.Height, desc.Format.0.cast_unsigned(), true);
        let mut staging = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe {
            self.device.device.CreateTexture2D(
                &raw const staging_desc,
                None,
                Some(&raw mut staging),
            )
        }
        .map_err(|e| win_err("staging texture", e))?;
        let staging = staging.ok_or_else(|| win_err("staging texture", "none returned"))?;
        // `info` (as published) is kept for change detection; reads use
        // the texture's own size and format.
        self.texture = Some(RecvTexture {
            info: info.clone(),
            shared,
            staging,
        });
        Ok(())
    }

    fn copy(&mut self) -> Result<Option<Arc<StillImage>>, MediaError> {
        let Some(tex) = &self.texture else {
            return Ok(None);
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid out-pointer.
        unsafe { tex.staging.GetDesc(&raw mut desc) };
        let format = desc.Format.0.cast_unsigned();
        let (w, h) = (desc.Width, desc.Height);
        let Some(bpp) = format::bytes_per_pixel(format) else {
            return Err(win_err(
                &self.name,
                format!("unsupported texture format {format}"),
            ));
        };
        let Some(_guard) = acquire(&self.access) else {
            return Ok(None);
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures have identical size and format on this
        // device; Map blocks until the copy has completed.
        unsafe {
            self.device.ctx.CopyResource(&tex.staging, &tex.shared);
            self.device
                .ctx
                .Map(&tex.staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
        }
        .map_err(|e| win_err("read texture", e))?;
        let row_bytes = w as usize * bpp;
        let pitch = mapped.RowPitch as usize;
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        let mut ok = pitch >= row_bytes && !mapped.pData.is_null();
        if ok {
            for (y, out) in rgba.chunks_exact_mut(w as usize * 4).enumerate() {
                // SAFETY: the mapped staging texture has `h` rows of `pitch`
                // bytes, each at least `row_bytes` long.
                let row = unsafe {
                    std::slice::from_raw_parts(
                        mapped.pData.cast::<u8>().add(y * pitch).cast_const(),
                        row_bytes,
                    )
                };
                ok &= format::row_to_rgba8(format, row, out);
            }
        }
        // SAFETY: mapped above.
        unsafe { self.device.ctx.Unmap(&tex.staging, 0) };
        drop(_guard);
        if !ok {
            return Err(win_err(&self.name, "could not read the shared texture"));
        }
        StillImage::from_rgba8(w, h, rgba).map(|i| Some(Arc::new(i)))
    }
}

impl LiveSource for SpoutReceiver {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.still_listed() {
                return Err(win_err(&self.name, "the sender closed"));
            }
            let info = self.info.with(|b| TextureInfo::read(b)).flatten();
            if let Some(info) = info.filter(|i| i.share_handle != 0 && i.width > 0 && i.height > 0)
            {
                let changed = self.texture.as_ref().is_none_or(|t| t.info != info);
                self.open_texture(&info)?;
                let count = self.frame_count();
                let new = match count {
                    Some(c) => changed || self.last_count != Some(c),
                    None => self
                        .last_copy
                        .is_none_or(|t| t.elapsed() >= UNCOUNTED_INTERVAL),
                };
                if new && let Some(image) = self.copy()? {
                    self.last_count = count;
                    self.last_copy = Some(Instant::now());
                    return Ok(Some(image));
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn describe(&self) -> String {
        match &self.texture {
            Some(t) => format!(
                "{}×{} Spout (format {})",
                t.info.width, t.info.height, t.info.format
            ),
            None => format!("Spout \"{}\"", self.name),
        }
    }

    /// Spout senders may publish only when their picture changes; a sender
    /// that quits leaves the directory, which `next_frame` reports.
    fn stall_timeout(&self) -> Option<Duration> {
        None
    }
}

/// Spout inputs and outputs.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpoutOpener;

impl LiveOpener for SpoutOpener {
    fn supports(&self, input: &LiveInput) -> bool {
        matches!(input, LiveInput::Spout { .. })
    }

    fn open_live(
        &self,
        input: &LiveInput,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        match input {
            LiveInput::Spout { sender } => Ok(Box::new(SpoutReceiver::new(sender)?)),
            other => Err(MediaError::Open {
                path: other.label(),
                message: "not a Spout input".into(),
            }),
        }
    }

    fn discover(&self) -> Vec<LiveInput> {
        sender_names()
            .into_iter()
            .map(|sender| LiveInput::Spout { sender })
            .collect()
    }
}

impl SinkOpener for SpoutOpener {
    fn supports(&self, target: &Publish) -> bool {
        matches!(target, Publish::Spout { .. })
    }

    fn open_sink(
        &self,
        target: &Publish,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        if stop.load(Ordering::Relaxed) {
            return Err(win_err("open", "stopped"));
        }
        match target {
            Publish::Spout { name } => Ok(Box::new(SpoutSender::new(name)?)),
            other => Err(MediaError::Open {
                path: other.label(),
                message: "not a Spout output".into(),
            }),
        }
    }
}
