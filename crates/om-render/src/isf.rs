// SPDX-License-Identifier: Apache-2.0
//! GPU runtime for translated ISF shaders (see `om_isf`).
//!
//! ISF shaders see sRGB-encoded, straight-alpha colour (as in GL tools); the
//! caller converts around them. Pass targets are `Rgba16Float`; persistent
//! targets are double-buffered so a pass can read last frame's contents
//! while writing this frame's.

use std::borrow::Cow;
use std::collections::HashMap;

use om_gpu::{GpuContext, WORKING_FORMAT};
use om_isf::{Compiled, InputKind, UniformKind};

use crate::RenderError;

/// A value for an ISF input (scalars, points, colours).
#[derive(Debug, Clone, PartialEq)]
pub enum IsfValue {
    Bool(bool),
    Number(f64),
    Vector(Vec<f64>),
}

struct Target {
    name: String,
    size: (u32, u32),
    persistent: bool,
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// Index written this frame; the other holds last frame (persistent).
    current: usize,
}

/// Everything one frame of an ISF program needs.
#[derive(Debug, Clone, Copy)]
pub struct IsfFrame<'a> {
    pub size: (u32, u32),
    /// Seconds for `TIME`.
    pub time: f64,
    pub values: &'a HashMap<String, IsfValue>,
    /// Image inputs (e.g. `inputImage`) as sRGB straight-alpha views.
    pub images: &'a HashMap<String, &'a wgpu::TextureView>,
    /// Where the final pass writes.
    pub output: &'a wgpu::TextureView,
}

/// One compiled ISF shader ready to run.
pub struct IsfProgram {
    compiled: Compiled,
    pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blank: (wgpu::Texture, wgpu::TextureView),
    uniforms: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    targets: Vec<Target>,
    frame: u32,
    last_time: Option<f64>,
}

impl std::fmt::Debug for IsfProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IsfProgram({} passes)", self.compiled.doc.passes.len())
    }
}

impl IsfProgram {
    /// Creates the GPU pipeline. Validation already happened in `om_isf`;
    /// GPU-side failures are captured and reported, never panicked.
    pub fn new(gpu: &GpuContext, compiled: &Compiled) -> Result<Self, RenderError> {
        let device = gpu.device();
        let _ = gpu.take_error();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("om isf"),
            source: wgpu::ShaderSource::Naga(Cow::Owned(compiled.module.clone())),
        });
        let vs = device.create_shader_module(wgpu::include_wgsl!("fullscreen.wgsl"));
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om isf uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(u64::from(compiled.uniform_size)),
                },
                count: None,
            }],
        });
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }];
        for k in 0..compiled.images.len() {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: u32::try_from(k + 1).unwrap_or(u32::MAX),
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om isf images"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om isf"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("om isf"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORKING_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        if let Some(e) = gpu.take_error() {
            return Err(RenderError::Gpu(e));
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("om isf"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank_tex = make_texture(device, (1, 1), "om isf blank");
        let blank_view = blank_tex.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            compiled: compiled.clone(),
            pipeline,
            uniform_layout,
            image_layout,
            sampler,
            blank: (blank_tex, blank_view),
            uniforms: Vec::new(),
            targets: Vec::new(),
            frame: 0,
            last_time: None,
        })
    }

    #[must_use]
    pub fn compiled(&self) -> &Compiled {
        &self.compiled
    }

    /// Clears persistent buffers and the frame counter.
    pub fn reset(&mut self) {
        self.targets.clear();
        self.frame = 0;
        self.last_time = None;
    }

    /// Runs every pass. `images` supplies image inputs (e.g. `inputImage`)
    /// as sRGB straight-alpha views; the final pass writes `output`.
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        frame: &IsfFrame<'_>,
    ) -> Result<(), RenderError> {
        let IsfFrame {
            size,
            time,
            values,
            images,
            output,
        } = *frame;
        let device = gpu.device();
        let passes = self.compiled.doc.passes.clone();
        // Numeric inputs available to size expressions.
        let mut vars = HashMap::new();
        for i in &self.compiled.doc.inputs {
            if let Some(IsfValue::Number(v)) = values.get(&i.name).or(None) {
                vars.insert(i.name.clone(), *v);
            } else if let Some(v) = i.default.as_ref().and_then(serde_json::Value::as_f64) {
                vars.insert(i.name.clone(), v);
            }
        }
        // (Re)create targets whose size changed.
        for p in &passes {
            let Some(name) = &p.target else { continue };
            let w = match &p.width {
                Some(e) => om_isf::eval_dimension(e, f64::from(size.0), f64::from(size.1), &vars)
                    .map_err(|e| RenderError::Gpu(e.to_string()))?,
                None => size.0,
            };
            let h = match &p.height {
                Some(e) => om_isf::eval_dimension(e, f64::from(size.0), f64::from(size.1), &vars)
                    .map_err(|e| RenderError::Gpu(e.to_string()))?,
                None => size.1,
            };
            let fresh = !self
                .targets
                .iter()
                .any(|t| &t.name == name && t.size == (w, h));
            if fresh {
                self.targets.retain(|t| &t.name != name);
                let a = make_texture(device, (w, h), "om isf target");
                let b = make_texture(device, (w, h), "om isf target");
                let views = [
                    a.create_view(&wgpu::TextureViewDescriptor::default()),
                    b.create_view(&wgpu::TextureViewDescriptor::default()),
                ];
                for v in &views {
                    clear(encoder, v);
                }
                self.targets.push(Target {
                    name: name.clone(),
                    size: (w, h),
                    persistent: p.persistent,
                    _textures: [a, b],
                    views,
                    current: 0,
                });
            }
        }
        // Persistent targets: write the other buffer this frame.
        for t in &mut self.targets {
            if t.persistent {
                t.current = 1 - t.current;
            }
        }
        while self.uniforms.len() < passes.len() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("om isf uniforms"),
                size: u64::from(self.compiled.uniform_size),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("om isf uniforms"),
                layout: &self.uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.uniforms.push((buffer, bind));
        }
        let delta = self.last_time.map_or(0.0, |l| (time - l).max(0.0));
        self.last_time = Some(time);
        let last = passes.len() - 1;
        for (index, pass) in passes.iter().enumerate() {
            let target = pass
                .target
                .as_ref()
                .and_then(|n| self.targets.iter().find(|t| &t.name == n));
            let render_size = target.map_or(size, |t| t.size);
            let bytes = self.uniform_bytes(index, render_size, time, delta, values);
            self.gpu_write(gpu, index, &bytes);
            // Bind images: inputs, then pass targets. A target written by
            // a later pass (or by this pass) is read from its other buffer
            // (last frame's contents — the feedback case, and never the
            // texture being rendered to); one written by an earlier pass is
            // read as written this frame.
            let mut views: Vec<&wgpu::TextureView> = Vec::new();
            for name in &self.compiled.images {
                let v = if let Some(v) = images.get(name) {
                    *v
                } else if let Some(t) = self.targets.iter().find(|t| &t.name == name) {
                    let writer = passes.iter().position(|p| p.target.as_ref() == Some(name));
                    if writer.is_some_and(|j| j < index) {
                        &t.views[t.current]
                    } else {
                        &t.views[1 - t.current]
                    }
                } else {
                    &self.blank.1
                };
                views.push(v);
            }
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            }];
            for (k, v) in views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: u32::try_from(k + 1).unwrap_or(u32::MAX),
                    resource: wgpu::BindingResource::TextureView(v),
                });
            }
            let image_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("om isf images"),
                layout: &self.image_layout,
                entries: &entries,
            });
            let mut outs: Vec<&wgpu::TextureView> = Vec::new();
            if let Some(t) = target {
                outs.push(&t.views[t.current]);
            }
            if index == last {
                outs.push(output);
            }
            for out in outs {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("om isf pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: out,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rp.set_pipeline(&self.pipeline);
                rp.set_bind_group(0, &self.uniforms[index].1, &[]);
                rp.set_bind_group(1, &image_bind, &[]);
                rp.draw(0..3, 0..1);
            }
        }
        self.frame = self.frame.wrapping_add(1);
        Ok(())
    }

    fn gpu_write(&self, gpu: &GpuContext, index: usize, bytes: &[u8]) {
        gpu.queue().write_buffer(&self.uniforms[index].0, 0, bytes);
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap
    )]
    fn uniform_bytes(
        &self,
        pass: usize,
        size: (u32, u32),
        time: f64,
        delta: f64,
        values: &HashMap<String, IsfValue>,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; self.compiled.uniform_size as usize];
        let mut put = |offset: u32, data: &[u8]| {
            let o = offset as usize;
            if let Some(slot) = buf.get_mut(o..o + data.len()) {
                slot.copy_from_slice(data);
            }
        };
        for f in &self.compiled.uniforms {
            let input = self.compiled.doc.inputs.iter().find(|i| i.name == f.name);
            let value: Vec<f64> = match f.name.as_str() {
                "TIME" if input.is_none() => vec![time],
                "TIMEDELTA" if input.is_none() => vec![delta],
                "FRAMEINDEX" if input.is_none() => vec![f64::from(self.frame)],
                "PASSINDEX" if input.is_none() => vec![pass as f64],
                "RENDERSIZE" if input.is_none() => vec![f64::from(size.0), f64::from(size.1)],
                "DATE" if input.is_none() => vec![0.0, 0.0, 0.0, 0.0],
                _ => {
                    let given = values.get(&f.name);
                    let default = input.and_then(|i| i.default.clone());
                    resolve(input.map(|i| i.kind), given, default.as_ref())
                }
            };
            match f.kind {
                UniformKind::Float => put(
                    f.offset,
                    &(value.first().copied().unwrap_or(0.0) as f32).to_le_bytes(),
                ),
                UniformKind::Int => put(
                    f.offset,
                    &(value.first().copied().unwrap_or(0.0).round() as i32).to_le_bytes(),
                ),
                UniformKind::Vec2 | UniformKind::Vec4 => {
                    let n = if f.kind == UniformKind::Vec2 { 2 } else { 4 };
                    for k in 0..n {
                        let v = value
                            .get(k)
                            .copied()
                            .unwrap_or(if k == 3 { 1.0 } else { 0.0 })
                            as f32;
                        put(f.offset + 4 * k as u32, &v.to_le_bytes());
                    }
                }
            }
        }
        buf
    }
}

/// Resolves an input's value: explicit, else ISF DEFAULT, else zero.
fn resolve(
    kind: Option<InputKind>,
    given: Option<&IsfValue>,
    default: Option<&serde_json::Value>,
) -> Vec<f64> {
    match given {
        Some(IsfValue::Bool(b)) => return vec![if *b { 1.0 } else { 0.0 }],
        Some(IsfValue::Number(n)) => return vec![*n],
        Some(IsfValue::Vector(v)) => return v.clone(),
        None => {}
    }
    match default {
        Some(serde_json::Value::Bool(b)) => vec![if *b { 1.0 } else { 0.0 }],
        Some(serde_json::Value::Number(n)) => vec![n.as_f64().unwrap_or(0.0)],
        Some(serde_json::Value::Array(a)) => {
            a.iter().filter_map(serde_json::Value::as_f64).collect()
        }
        _ => match kind {
            Some(InputKind::Color) => vec![0.0, 0.0, 0.0, 1.0],
            _ => vec![0.0],
        },
    }
}

fn make_texture(device: &wgpu::Device, size: (u32, u32), label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("om isf clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
}
