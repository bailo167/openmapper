// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::mpsc;

use bytemuck::{Pod, Zeroable};
use om_gpu::{GpuContext, WORKING_FORMAT};
use om_media_core::StillImage;
use om_project::Project;
use om_types::MediaId;

use crate::colour::rgba8_srgb_to_linear_premul_f16;
use crate::plan::{Clip, Mapping, RenderPlan, plan};
use om_project::BlendMode;

/// Rendering failure.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU device was lost; the renderer must be recreated")]
    DeviceLost,
    #[error("nothing has been rendered yet")]
    NoCanvas,
    #[error("reading the frame back from the GPU failed: {0}")]
    Readback(String),
    #[error("{what} is {width}x{height}; this GPU supports at most {max} per side")]
    TooLarge {
        what: &'static str,
        width: u32,
        height: u32,
        max: u32,
    },
    #[error("GPU error: {0}")]
    Gpu(String),
}

/// What happened in one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameReport {
    pub plan: RenderPlan,
    pub canvas: (u32, u32),
}

/// Live GPU resources owned by the compositor (for leak tests and diagnostics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceCounts {
    pub media_textures: usize,
    pub media_bytes: u64,
    pub canvas_bytes: u64,
    pub uniform_bytes: u64,
    pub vertex_bytes: u64,
    pub blit_pipelines: usize,
    pub mask_textures: usize,
    pub effect_targets: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ItemUniform {
    c0: [f32; 4],
    c1: [f32; 4],
    c2: [f32; 4],
    l0: [f32; 4],
    l1: [f32; 4],
    l2: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
}

/// A surface's rasterised mask (canvas-sized coverage texture).
struct GpuMask {
    key: u64,
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
}

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// Mask uniform: params vec4 + 512 vec4 of points.
const MASK_UNIFORM_SIZE: u64 = 16 + 512 * 16;

struct GpuImage {
    texture: wgpu::Texture,
    size: (u32, u32),
    bind_group: wgpu::BindGroup,
    /// Same texture bound for `textureLoad` (effect input).
    load_bind: wgpu::BindGroup,
    bytes: u64,
}

/// Per-surface effect targets: two ping-pong textures at media size.
struct SurfaceFx {
    size: (u32, u32),
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    load_binds: [wgpu::BindGroup; 2],
    media_binds: [wgpu::BindGroup; 2],
    uniforms: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Which target holds this frame's result.
    output: Option<usize>,
}

const FX_UNIFORM_SIZE: u64 = 72 * 4;

struct Canvas {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Draws projects with the GPU.
pub struct Compositor {
    gpu: GpuContext,
    /// One composite pipeline per blend mode.
    pipelines: HashMap<BlendMode, wgpu::RenderPipeline>,
    item_layout: wgpu::BindGroupLayout,
    media_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    blit_module: wgpu::ShaderModule,
    blit_pipeline_layout: wgpu::PipelineLayout,
    blit_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
    uniform_stride: u64,
    uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    vertices: Option<(wgpu::Buffer, u64)>,
    images: HashMap<MediaId, GpuImage>,
    canvas: Option<Canvas>,
    mask_layout: wgpu::BindGroupLayout,
    mask_gen_layout: wgpu::BindGroupLayout,
    mask_gen_pipeline: wgpu::RenderPipeline,
    /// Bound for unmasked items (never sampled).
    no_mask: (wgpu::Texture, wgpu::BindGroup),
    masks: HashMap<om_types::SurfaceId, GpuMask>,
    fx_layout: wgpu::BindGroupLayout,
    fx_pipelines: HashMap<&'static str, wgpu::RenderPipeline>,
    fx: HashMap<om_types::SurfaceId, SurfaceFx>,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Compositor({:?})", self.resource_counts())
    }
}

const VERTEX_SIZE: u64 = 8; // vec2<f32>

impl Compositor {
    #[must_use]
    pub fn new(gpu: GpuContext) -> Self {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::include_wgsl!("composite.wgsl"));
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om item"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ItemUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let media_layout = texture_layout(device, "om media");
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om mask"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let mask_gen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om mask gen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(MASK_UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let mask_module = device.create_shader_module(wgpu::include_wgsl!("mask.wgsl"));
        let mask_gen_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("om mask gen"),
                bind_group_layouts: &[Some(&mask_gen_layout)],
                immediate_size: 0,
            });
        let mask_gen_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("om mask gen"),
            layout: Some(&mask_gen_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mask_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mask_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let no_mask_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("om no mask"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let no_mask_view = no_mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let no_mask_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om no mask"),
            layout: &mask_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&no_mask_view),
            }],
        });
        let no_mask = (no_mask_tex, no_mask_bind);
        let fx_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om effect"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(FX_UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let fx_module = device.create_shader_module(wgpu::include_wgsl!("effects.wgsl"));
        let fx_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om effect"),
            bind_group_layouts: &[Some(&fx_layout), Some(&mask_layout)],
            immediate_size: 0,
        });
        let fx_pipelines = [
            "fs_color",
            "fs_invert",
            "fs_blur_h",
            "fs_blur_v",
            "fs_pixelate",
        ]
        .into_iter()
        .map(|entry| {
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&fx_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &fx_module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &fx_module,
                    entry_point: Some(entry),
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
            (entry, p)
        })
        .collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om composite"),
            bind_group_layouts: &[Some(&item_layout), Some(&media_layout), Some(&mask_layout)],
            immediate_size: 0,
        });
        let make = |blend: BlendMode| {
            let (color, alpha) = blend_state(blend);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("om composite"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None, // mirrored surfaces flip winding
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORKING_FORMAT,
                        blend: Some(wgpu::BlendState { color, alpha }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            BlendMode::Normal,
            BlendMode::Add,
            BlendMode::Screen,
            BlendMode::Multiply,
        ]
        .into_iter()
        .map(|b| (b, make(b)))
        .collect();
        let blit_layout = texture_layout(device, "om blit");
        let blit_module = device.create_shader_module(wgpu::include_wgsl!("blit.wgsl"));
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om blit"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("om linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let size = std::mem::size_of::<ItemUniform>() as u64;
        let uniform_stride = size.div_ceil(align) * align;
        Self {
            gpu,
            pipelines,
            item_layout,
            media_layout,
            blit_layout,
            blit_module,
            blit_pipeline_layout,
            blit_pipelines: HashMap::new(),
            sampler,
            uniform_stride,
            uniforms: None,
            vertices: None,
            images: HashMap::new(),
            canvas: None,
            mask_layout,
            mask_gen_layout,
            mask_gen_pipeline,
            no_mask,
            masks: HashMap::new(),
            fx_layout,
            fx_pipelines,
            fx: HashMap::new(),
        }
    }

    #[must_use]
    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    /// Uploads (or replaces) a media image. Images larger than the GPU
    /// supports are rejected (and any previous upload for `id` removed).
    pub fn set_image(&mut self, id: MediaId, image: &StillImage) -> Result<(), RenderError> {
        let max = self.gpu.max_texture_dimension();
        if image.width() > max || image.height() > max {
            self.images.remove(&id);
            return Err(RenderError::TooLarge {
                what: "media image",
                width: image.width(),
                height: image.height(),
                max,
            });
        }
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texels = rgba8_srgb_to_linear_premul_f16(image.rgba8());
        let dims = (image.width(), image.height());
        // Video uploads every frame: reuse the texture when the size matches.
        if !self.images.get(&id).is_some_and(|g| g.size == dims) {
            let device = self.gpu.device();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("om media"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = texture_bind_group(device, &self.media_layout, &view, &self.sampler);
            let load_bind = load_bind_group(device, &self.mask_layout, &view);
            self.images.insert(
                id,
                GpuImage {
                    texture,
                    size: dims,
                    bind_group,
                    load_bind,
                    bytes: texels.len() as u64,
                },
            );
        }
        let Some(img) = self.images.get(&id) else {
            return Ok(());
        };
        self.gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &img.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * 8),
                rows_per_image: Some(image.height()),
            },
            size,
        );
        Ok(())
    }

    pub fn remove_image(&mut self, id: MediaId) {
        self.images.remove(&id);
    }

    /// Drops every image whose id fails `keep`.
    pub fn retain_images(&mut self, mut keep: impl FnMut(MediaId) -> bool) {
        self.images.retain(|id, _| keep(*id));
    }

    #[must_use]
    pub fn has_image(&self, id: MediaId) -> bool {
        self.images.contains_key(&id)
    }

    /// Renders `project` into the working canvas.
    pub fn render(&mut self, project: &Project) -> Result<FrameReport, RenderError> {
        if self.gpu.is_lost() {
            return Err(RenderError::DeviceLost);
        }
        if let Some(e) = self.gpu.take_error() {
            return Err(RenderError::Gpu(e));
        }
        let size = (project.canvas.width, project.canvas.height);
        let max = self.gpu.max_texture_dimension();
        if size.0 > max || size.1 > max {
            return Err(RenderError::TooLarge {
                what: "canvas",
                width: size.0,
                height: size.1,
                max,
            });
        }
        self.ensure_canvas(size);
        let frame_plan = plan(project, |m| self.images.contains_key(&m));
        let mut mask_encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("om masks"),
                });
        self.update_masks(&frame_plan, size, &mut mask_encoder);
        self.update_effects(&frame_plan, &mut mask_encoder);
        self.gpu.queue().submit([mask_encoder.finish()]);

        // Geometry: triangle fans flattened into a list; one uniform per item.
        let mut verts: Vec<f32> = Vec::new();
        let mut ranges = Vec::with_capacity(frame_plan.items.len());
        let mut uniforms: Vec<u8> = Vec::new();
        #[allow(clippy::cast_possible_truncation)]
        for item in &frame_plan.items {
            let start = verts.len() / 2;
            let p = &item.polygon;
            for i in 1..p.len() - 1 {
                for v in [p[0], p[i], p[i + 1]] {
                    verts.extend_from_slice(&[v.0 as f32, v.1 as f32]);
                }
            }
            ranges.push(start as u32..(verts.len() / 2) as u32);
            // Mapping kind in params.w: 0 projective, 1 projective + ellipse
            // clip (l0..l2 = canvas->local), 2 bilinear (c0,c1 = corners,
            // l0,l1 = UVs, as xy pairs P00 P10 | P11 P01).
            let (kind, [c0, c1, c2], [l0, l1, l2]) = match (item.mapping, item.clip) {
                (Mapping::Projective(h), Clip::None) => (0.0, h.to_gpu_cols(), [[0.0; 4]; 3]),
                (Mapping::Projective(h), Clip::Ellipse { canvas_to_local }) => {
                    (1.0, h.to_gpu_cols(), canvas_to_local.to_gpu_cols())
                }
                (Mapping::Bilinear { corners: q, uv }, _) => {
                    let f = |a: (f64, f64), b: (f64, f64)| {
                        [a.0 as f32, a.1 as f32, b.0 as f32, b.1 as f32]
                    };
                    (
                        2.0,
                        [f(q[0], q[1]), f(q[2], q[3]), [0.0; 4]],
                        [f(uv[0], uv[1]), f(uv[2], uv[3]), [0.0; 4]],
                    )
                }
            };
            let clip_kind = kind;
            let u = ItemUniform {
                c0,
                c1,
                c2,
                l0,
                l1,
                l2,
                params: [item.opacity, size.0 as f32, size.1 as f32, clip_kind],
                extra: [if item.mask.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            };
            let mut block = vec![0u8; self.uniform_stride as usize];
            block[..std::mem::size_of::<ItemUniform>()].copy_from_slice(bytemuck::bytes_of(&u));
            uniforms.extend_from_slice(&block);
        }
        self.ensure_uniforms(uniforms.len() as u64);
        self.ensure_vertices((verts.len() as u64) * 4);
        let queue = self.gpu.queue();
        if let (Some((ubuf, ..)), false) = (&self.uniforms, uniforms.is_empty()) {
            queue.write_buffer(ubuf, 0, &uniforms);
        }
        if let (Some((vbuf, _)), false) = (&self.vertices, verts.is_empty()) {
            queue.write_buffer(vbuf, 0, bytemuck::cast_slice(&verts));
        }

        let Some(canvas) = &self.canvas else {
            return Err(RenderError::NoCanvas);
        };
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("om frame"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("om composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &canvas.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let (Some((_, ubind, _)), Some((vbuf, _))) = (&self.uniforms, &self.vertices) {
                pass.set_vertex_buffer(0, vbuf.slice(..));
                for (i, (item, range)) in frame_plan.items.iter().zip(&ranges).enumerate() {
                    let Some(pipeline) = self.pipelines.get(&item.blend) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    let Some(img) = self.images.get(&item.media) else {
                        continue;
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let offset = (i as u64 * self.uniform_stride) as u32;
                    pass.set_bind_group(0, ubind, &[offset]);
                    let media_bind = item
                        .effects
                        .as_ref()
                        .and_then(|_| self.fx.get(&item.surface))
                        .and_then(|fx| fx.output.map(|i| &fx.media_binds[i]))
                        .unwrap_or(&img.bind_group);
                    pass.set_bind_group(1, media_bind, &[]);
                    let mask_bind = self
                        .masks
                        .get(&item.surface)
                        .filter(|_| item.mask.is_some())
                        .map_or(&self.no_mask.1, |m| &m.bind_group);
                    pass.set_bind_group(2, mask_bind, &[]);
                    pass.draw(range.clone(), 0..1);
                }
            }
        }
        self.gpu.queue().submit([encoder.finish()]);
        Ok(FrameReport {
            plan: frame_plan,
            canvas: size,
        })
    }

    /// Size of the working canvas, once something has been rendered.
    #[must_use]
    pub fn canvas_size(&self) -> Option<(u32, u32)> {
        self.canvas.as_ref().map(|c| c.size)
    }

    /// Draws the canvas onto `target` (scaled to fill it), composited over
    /// black. `format` is the target's texture format.
    pub fn present(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) -> Result<(), RenderError> {
        let pipeline = self.blit_pipeline(format).clone();
        let canvas = self.canvas.as_ref().ok_or(RenderError::NoCanvas)?;
        let bind = texture_bind_group(
            self.gpu.device(),
            &self.blit_layout,
            &canvas.view,
            &self.sampler,
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("om present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    /// Reads the presented canvas back as 8-bit sRGB RGBA (alpha 255),
    /// row-major, top row first.
    pub fn read_rgba8(&mut self) -> Result<Vec<u8>, RenderError> {
        let (w, h) = self.canvas_size().ok_or(RenderError::NoCanvas)?;
        let device = self.gpu.device().clone();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("om readback"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let row = w * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("om readback"),
        });
        self.present(&mut encoder, &view, format)?;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue().submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let data = buffer
            .get_mapped_range(..)
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }

    #[must_use]
    pub fn resource_counts(&self) -> ResourceCounts {
        ResourceCounts {
            media_textures: self.images.len(),
            media_bytes: self.images.values().map(|i| i.bytes).sum(),
            canvas_bytes: self
                .canvas
                .as_ref()
                .map_or(0, |c| u64::from(c.size.0) * u64::from(c.size.1) * 8),
            uniform_bytes: self.uniforms.as_ref().map_or(0, |u| u.2),
            vertex_bytes: self.vertices.as_ref().map_or(0, |v| v.1),
            blit_pipelines: self.blit_pipelines.len(),
            mask_textures: self.masks.len(),
            effect_targets: self.fx.len(),
        }
    }

    /// Runs each surface's effect chain on its media into ping-pong targets.
    fn update_effects(&mut self, frame_plan: &RenderPlan, encoder: &mut wgpu::CommandEncoder) {
        let mut wanted: Vec<(
            om_types::SurfaceId,
            MediaId,
            std::sync::Arc<Vec<om_project::EffectKind>>,
        )> = Vec::new();
        for item in &frame_plan.items {
            if let Some(e) = &item.effects
                && !wanted.iter().any(|(s, ..)| *s == item.surface)
            {
                wanted.push((item.surface, item.media, e.clone()));
            }
        }
        self.fx.retain(|id, _| wanted.iter().any(|(s, ..)| s == id));
        for (surface, media, effects) in wanted {
            let passes = crate::effects::passes(effects.iter());
            let Some(img_size) = self.images.get(&media).map(|i| i.size) else {
                continue;
            };
            if passes.is_empty() {
                if let Some(fx) = self.fx.get_mut(&surface) {
                    fx.output = None;
                }
                continue;
            }
            if !self.fx.get(&surface).is_some_and(|f| f.size == img_size) {
                let fx = self.create_fx(img_size);
                self.fx.insert(surface, fx);
            }
            while self
                .fx
                .get(&surface)
                .is_some_and(|f| f.uniforms.len() < passes.len())
            {
                let u = self.create_fx_uniform();
                if let Some(f) = self.fx.get_mut(&surface) {
                    f.uniforms.push(u);
                }
            }
            let (Some(fx), Some(img)) = (self.fx.get(&surface), self.images.get(&media)) else {
                continue;
            };
            let mut input: &wgpu::BindGroup = &img.load_bind;
            let mut target = 0usize;
            for (k, pass) in passes.iter().enumerate() {
                let entry = match pass {
                    crate::effects::Pass::Color { .. } => "fs_color",
                    crate::effects::Pass::Invert => "fs_invert",
                    crate::effects::Pass::BlurH { .. } => "fs_blur_h",
                    crate::effects::Pass::BlurV { .. } => "fs_blur_v",
                    crate::effects::Pass::Pixelate { .. } => "fs_pixelate",
                };
                let Some(pipeline) = self.fx_pipelines.get(entry) else {
                    continue;
                };
                let (ubuf, ubind) = &fx.uniforms[k];
                self.gpu.queue().write_buffer(
                    ubuf,
                    0,
                    bytemuck::cast_slice(&crate::effects::uniform(pass)),
                );
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("om effect"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fx.views[target],
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rp.set_pipeline(pipeline);
                rp.set_bind_group(0, ubind, &[]);
                rp.set_bind_group(1, input, &[]);
                rp.draw(0..3, 0..1);
                drop(rp);
                input = &fx.load_binds[target];
                target = 1 - target;
            }
            let out = 1 - target;
            if let Some(fx) = self.fx.get_mut(&surface) {
                fx.output = Some(out);
            }
        }
    }

    fn create_fx(&self, size: (u32, u32)) -> SurfaceFx {
        let device = self.gpu.device();
        let make = || {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("om effect target"),
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
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let v = t.create_view(&wgpu::TextureViewDescriptor::default());
            (t, v)
        };
        let (t0, v0) = make();
        let (t1, v1) = make();
        let load_binds = [
            load_bind_group(device, &self.mask_layout, &v0),
            load_bind_group(device, &self.mask_layout, &v1),
        ];
        let media_binds = [
            texture_bind_group(device, &self.media_layout, &v0, &self.sampler),
            texture_bind_group(device, &self.media_layout, &v1, &self.sampler),
        ];
        SurfaceFx {
            size,
            _textures: [t0, t1],
            views: [v0, v1],
            load_binds,
            media_binds,
            uniforms: Vec::new(),
            output: None,
        }
    }

    fn create_fx_uniform(&self) -> (wgpu::Buffer, wgpu::BindGroup) {
        let device = self.gpu.device();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om effect uniform"),
            size: FX_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om effect uniform"),
            layout: &self.fx_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        (buffer, bind)
    }

    /// Rasterises masks that are new or changed; drops masks no longer used.
    fn update_masks(
        &mut self,
        frame_plan: &RenderPlan,
        size: (u32, u32),
        encoder: &mut wgpu::CommandEncoder,
    ) {
        use std::hash::{Hash, Hasher};
        let mut wanted: HashMap<om_types::SurfaceId, &crate::plan::MaskShape> = HashMap::new();
        for item in &frame_plan.items {
            if let Some(m) = &item.mask {
                wanted.insert(item.surface, m);
            }
        }
        self.masks.retain(|id, _| wanted.contains_key(id));
        for (id, mask) in wanted {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            size.hash(&mut h);
            mask.invert.hash(&mut h);
            mask.feather.to_bits().hash(&mut h);
            for (x, y) in &mask.polygon {
                x.to_bits().hash(&mut h);
                y.to_bits().hash(&mut h);
            }
            let key = h.finish();
            if self.masks.get(&id).is_some_and(|m| m.key == key) {
                continue;
            }
            if !self.masks.get(&id).is_some_and(|m| m.size == size) {
                let m = self.create_mask(size);
                self.masks.insert(id, m);
            }
            let Some(m) = self.masks.get_mut(&id) else {
                continue;
            };
            m.key = key;
            // Uniform: count, feather (px), invert; then points in pixels.
            let mut data = vec![0f32; (MASK_UNIFORM_SIZE / 4) as usize];
            let n = mask.polygon.len().min(crate::plan::MAX_MASK_VERTICES);
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            {
                data[0] = n as f32;
                data[1] = (mask.feather * f64::from(size.1)) as f32;
                data[2] = if mask.invert { 1.0 } else { 0.0 };
                for (i, (x, y)) in mask.polygon.iter().take(n).enumerate() {
                    data[4 + i * 2] = (x * f64::from(size.0)) as f32;
                    data[4 + i * 2 + 1] = (y * f64::from(size.1)) as f32;
                }
            }
            self.gpu
                .queue()
                .write_buffer(&m.uniform, 0, bytemuck::cast_slice(&data));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("om mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &m.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.mask_gen_pipeline);
            pass.set_bind_group(0, &m.uniform_bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn create_mask(&self, size: (u32, u32)) -> GpuMask {
        let device = self.gpu.device();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("om mask"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om mask"),
            layout: &self.mask_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om mask uniform"),
            size: MASK_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om mask uniform"),
            layout: &self.mask_gen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        GpuMask {
            key: 0,
            size,
            _texture: texture,
            view,
            bind_group,
            uniform,
            uniform_bind,
        }
    }

    fn ensure_canvas(&mut self, size: (u32, u32)) {
        if self.canvas.as_ref().is_some_and(|c| c.size == size) {
            return;
        }
        let texture = self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("om canvas"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.canvas = Some(Canvas {
            size,
            _texture: texture,
            view,
        });
    }

    /// Grows (never shrinks below need) the uniform buffer; capacity doubles
    /// so steady-state frames allocate nothing.
    fn ensure_uniforms(&mut self, needed: u64) {
        let needed = needed.max(self.uniform_stride);
        if self.uniforms.as_ref().is_some_and(|u| u.2 >= needed) {
            return;
        }
        let cap = needed.next_power_of_two();
        let device = self.gpu.device();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om item uniforms"),
            size: cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om item"),
            layout: &self.item_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ItemUniform>() as u64),
                }),
            }],
        });
        self.uniforms = Some((buffer, bind, cap));
    }

    fn ensure_vertices(&mut self, needed: u64) {
        let needed = needed.max(VERTEX_SIZE * 6);
        if self.vertices.as_ref().is_some_and(|v| v.1 >= needed) {
            return;
        }
        let cap = needed.next_power_of_two();
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("om vertices"),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertices = Some((buffer, cap));
    }

    fn blit_pipeline(&mut self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let device = self.gpu.device();
        let (module, layout) = (&self.blit_module, &self.blit_pipeline_layout);
        self.blit_pipelines.entry(format).or_insert_with(|| {
            // Hardware sRGB formats encode on store; others need it in the shader.
            let encode = if format.is_srgb() { 0.0 } else { 1.0 };
            let constants = [("encode_srgb", encode)];
            let options = wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("om present"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }
}

/// Fixed-function blending for each mode, on premultiplied linear colour.
/// The CPU reference (`reference::blend`) implements the same equations.
fn blend_state(mode: BlendMode) -> (wgpu::BlendComponent, wgpu::BlendComponent) {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as Op};
    let c = |src_factor, dst_factor| C {
        src_factor,
        dst_factor,
        operation: Op::Add,
    };
    let alpha_over = c(F::One, F::OneMinusSrcAlpha);
    let color = match mode {
        BlendMode::Normal => c(F::One, F::OneMinusSrcAlpha),
        BlendMode::Add => c(F::One, F::One),
        BlendMode::Screen => c(F::One, F::OneMinusSrc),
        BlendMode::Multiply => c(F::Dst, F::OneMinusSrcAlpha),
    };
    (color, alpha_over)
}

fn texture_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn load_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

fn texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
