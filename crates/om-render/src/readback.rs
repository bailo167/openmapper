// SPDX-License-Identifier: Apache-2.0
//! Non-blocking GPU → CPU frame readback for publishing.
//!
//! Frames are copied into a small ring of mappable buffers and collected a
//! frame or two later, so the render loop never waits for the GPU. When
//! every buffer is still in flight a capture is skipped (publishing is
//! "newest frame wins" anyway).

use std::sync::{Arc, Mutex};

use om_media_core::StillImage;

/// Buffers in flight at once.
const SLOTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Free,
    InFlight,
    Mapped,
    Failed,
}

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<Mutex<SlotState>>,
    /// Capture order, to return the newest frame.
    seq: u64,
}

/// Reads back RGBA8 frames (8-bit, 4 bytes per pixel textures).
pub struct FrameReader {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: (u32, u32),
    slots: Vec<Slot>,
    seq: u64,
    /// Captures skipped because every buffer was busy.
    pub skipped: u64,
}

impl std::fmt::Debug for FrameReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameReader({}×{})", self.size.0, self.size.1)
    }
}

fn padded_row(width: u32) -> u32 {
    (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn set(state: &Mutex<SlotState>, s: SlotState) {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = s;
}

fn get(state: &Mutex<SlotState>) -> SlotState {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FrameReader {
    #[must_use]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            size: (0, 0),
            slots: Vec::new(),
            seq: 0,
            skipped: 0,
        }
    }

    fn resize(&mut self, size: (u32, u32)) {
        if self.size == size && !self.slots.is_empty() {
            return;
        }
        self.size = size;
        let bytes = u64::from(padded_row(size.0)) * u64::from(size.1);
        self.slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("om frame readback"),
                    size: bytes.max(4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(Mutex::new(SlotState::Free)),
                seq: 0,
            })
            .collect();
    }

    /// Queues a copy of `texture` (which needs `COPY_SRC` usage and a
    /// 4-byte-per-pixel format). Call after the frame's render submission.
    pub fn capture(&mut self, texture: &wgpu::Texture) {
        let size = (texture.width(), texture.height());
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        self.resize(size);
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| matches!(get(&s.state), SlotState::Free | SlotState::Failed))
        else {
            self.skipped += 1;
            return;
        };
        self.seq += 1;
        slot.seq = self.seq;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("om frame readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &slot.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row(size.0)),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        set(&slot.state, SlotState::InFlight);
        let state = Arc::clone(&slot.state);
        slot.buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            set(
                &state,
                if r.is_ok() {
                    SlotState::Mapped
                } else {
                    SlotState::Failed
                },
            );
        });
    }

    /// Collects finished copies; returns the newest frame, if any finished.
    pub fn poll(&mut self) -> Option<StillImage> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.collect()
    }

    /// Like [`FrameReader::poll`] but waits for every copy in flight (tests,
    /// offline use).
    pub fn wait(&mut self) -> Option<StillImage> {
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        });
        self.collect()
    }

    fn collect(&mut self) -> Option<StillImage> {
        let (w, h) = self.size;
        let row = (w * 4) as usize;
        let padded = padded_row(w) as usize;
        let mut newest: Option<(u64, StillImage)> = None;
        for slot in &mut self.slots {
            if get(&slot.state) != SlotState::Mapped {
                continue;
            }
            let newer = newest.as_ref().is_none_or(|(s, _)| slot.seq > *s);
            if newer && let Ok(data) = slot.buffer.get_mapped_range(..) {
                let mut px = Vec::with_capacity(row * h as usize);
                for y in 0..h as usize {
                    px.extend_from_slice(&data[y * padded..y * padded + row]);
                }
                drop(data);
                if let Ok(img) = StillImage::from_rgba8(w, h, px) {
                    newest = Some((slot.seq, img));
                }
            }
            slot.buffer.unmap();
            set(&slot.state, SlotState::Free);
        }
        newest.map(|(_, img)| img)
    }
}
