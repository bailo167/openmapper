// SPDX-License-Identifier: Apache-2.0
//! GPU device management.
//!
//! [`GpuContext`] bundles the adapter, device and queue the renderer uses.
//! It is either created headless (CLI, tests) or wraps the device the desktop
//! UI already owns, so the mapping renderer and the UI share one GPU device.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Texture format of the internal linear-light working surface.
pub const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// GPU initialisation failure.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no usable GPU adapter found ({0})")]
    NoAdapter(String),
    #[error("could not open GPU device: {0}")]
    Device(String),
    #[error("GPU adapter cannot render to and blend {WORKING_FORMAT:?}")]
    MissingWorkingFormat,
}

/// Summary of what the active GPU supports, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub backend: String,
    pub adapter: String,
    pub driver: String,
    pub device_type: String,
    pub max_texture_dimension: u32,
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} on {} ({}, driver {}), max texture {}",
            self.backend, self.adapter, self.device_type, self.driver, self.max_texture_dimension
        )
    }
}

/// Adapter + device + queue.
#[derive(Clone)]
pub struct GpuContext {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    lost: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GpuContext({})", self.capabilities())
    }
}

impl GpuContext {
    /// Creates a headless context: a hardware adapter if available, otherwise
    /// a software fallback (WARP, lavapipe, llvmpipe).
    pub fn headless() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let mut last_err = String::from("no adapters");
        for force_fallback_adapter in [false, true] {
            let options = wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter,
                compatible_surface: None,
                ..Default::default()
            };
            match pollster::block_on(instance.request_adapter(&options)) {
                Ok(adapter) => return Self::from_adapter(adapter),
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(GpuError::NoAdapter(last_err))
    }

    fn from_adapter(adapter: wgpu::Adapter) -> Result<Self, GpuError> {
        check_working_format(&adapter)?;
        let desc = wgpu::DeviceDescriptor {
            label: Some("openmapper"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&desc))
            .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self::from_parts(adapter, device, queue))
    }

    /// Wraps an existing device (e.g. the UI's).
    #[must_use]
    pub fn from_parts(adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let lost = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |_reason, _msg| flag.store(true, Ordering::SeqCst));
        // wgpu's default handler panics on validation errors; a live show must
        // keep running, so record the error for diagnostics instead.
        let last_error = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&last_error);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(e.to_string());
            }
        }));
        Self {
            adapter,
            device,
            queue,
            lost,
            last_error,
        }
    }

    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    #[must_use]
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// True once the driver has reported the device lost. Renderers must be
    /// rebuilt on a new context; project state is unaffected.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// Takes the most recent uncaptured GPU error, if any.
    #[must_use]
    pub fn take_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|mut s| s.take())
    }

    /// Largest supported 2-D texture side.
    #[must_use]
    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        let info = self.adapter.get_info();
        Capabilities {
            backend: format!("{:?}", info.backend),
            adapter: info.name,
            driver: if info.driver_info.is_empty() {
                info.driver
            } else {
                format!("{} {}", info.driver, info.driver_info)
            },
            device_type: format!("{:?}", info.device_type),
            max_texture_dimension: self.device.limits().max_texture_dimension_2d,
        }
    }
}

fn check_working_format(adapter: &wgpu::Adapter) -> Result<(), GpuError> {
    let f = adapter.get_texture_format_features(WORKING_FORMAT);
    let needed = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    if f.allowed_usages.contains(needed)
        && f.flags.contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
        && f.flags
            .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
    {
        Ok(())
    } else {
        Err(GpuError::MissingWorkingFormat)
    }
}
