// SPDX-License-Identifier: Apache-2.0
//! Audio as textures for ISF `audio` and `audioFFT` inputs: one row per
//! channel (channel 0 at the bottom, y near 0), one texel per sample or
//! bin, the value in RGB (alpha 1).
//! Waveforms are stored as `0.5 + 0.5 × sample` (silence is 0.5, the
//! convention ISF audio shaders expect); spectra as magnitudes in 0..=1.

use std::collections::HashMap;

use half::f16;
use om_gpu::{GpuContext, WORKING_FORMAT};
use om_isf::{Compiled, InputKind};

/// Waveform rows (−1..=1) and spectrum rows (0..=1), one row per channel.
#[derive(Debug, Clone, Copy)]
pub struct AudioFrame<'a> {
    pub wave: &'a [Vec<f32>],
    pub fft: &'a [Vec<f32>],
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// The two audio textures, created on first upload.
#[derive(Default)]
pub(crate) struct AudioTextures {
    wave: Option<Tex>,
    fft: Option<Tex>,
}

impl AudioTextures {
    pub(crate) fn upload(&mut self, gpu: &GpuContext, frame: AudioFrame<'_>) {
        upload(
            gpu,
            &mut self.wave,
            frame.wave,
            |v| 0.5 + 0.5 * v,
            "om audio wave",
        );
        upload(gpu, &mut self.fft, frame.fft, |v| v, "om audio fft");
    }

    /// Adds the audio textures for `compiled`'s audio inputs to `images`
    /// (inputs left out are bound to blank by the ISF program).
    pub(crate) fn bind<'a>(
        &'a self,
        compiled: &Compiled,
        images: &mut HashMap<String, &'a wgpu::TextureView>,
    ) {
        for input in &compiled.doc.inputs {
            let tex = match input.kind {
                InputKind::Audio => &self.wave,
                InputKind::AudioFft => &self.fft,
                _ => continue,
            };
            if let Some(t) = tex {
                images.insert(input.name.clone(), &t.view);
            }
        }
    }
}

fn upload(
    gpu: &GpuContext,
    slot: &mut Option<Tex>,
    rows: &[Vec<f32>],
    map: impl Fn(f32) -> f32,
    label: &str,
) {
    let width = rows.first().map_or(0, Vec::len);
    let max = gpu.max_texture_dimension() as usize;
    if width == 0 || width > max || rows.len() > max || rows.iter().any(|r| r.len() != width) {
        return;
    }
    #[allow(clippy::cast_possible_truncation)]
    let size = (width as u32, rows.len() as u32);
    if !slot.as_ref().is_some_and(|t| t.size == size) {
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        *slot = Some(Tex {
            texture,
            view,
            size,
        });
    }
    let Some(t) = slot else {
        return;
    };
    // ISF texture coordinates have a bottom-left origin: channel 0 is the
    // bottom row (y near 0), so rows are stored last channel first.
    let mut texels = Vec::with_capacity(width * rows.len() * 8);
    for v in rows.iter().rev().flatten() {
        let v = if v.is_finite() { map(*v) } else { map(0.0) };
        let h = f16::from_f32(v).to_le_bytes();
        for c in [h, h, h, f16::ONE.to_le_bytes()] {
            texels.extend_from_slice(&c);
        }
    }
    gpu.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &t.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * 8),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}
