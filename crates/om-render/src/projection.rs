// SPDX-License-Identifier: Apache-2.0
//! 3-D mapping pass: renders a model from a calibrated projector's point of
//! view, textured with the canvas (see `projection.wgsl`).

use std::collections::HashMap;

use om_calibration::{Projector, linalg::rodrigues};
use om_geom::obj::Mesh;

const UNIFORM_SIZE: u64 = 80;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Near and far planes (model units from the projector).
const NEAR: f64 = 0.01;
const FAR: f64 = 10_000.0;

/// Clip-space transform of a projector (column-major 4×4): projector pixel
/// `(u, v)` maps to NDC `(2u/W − 1, 1 − 2v/H)`, depth in `[0, 1]`.
#[must_use]
pub fn view_projection(p: &Projector) -> [f32; 16] {
    let r = rodrigues(p.pose.rotation);
    let t = p.pose.translation;
    let (w, h) = (f64::from(p.width.max(1)), f64::from(p.height.max(1)));
    let k = &p.intrinsics;
    // Rows of the 4×4 projection applied to camera coordinates.
    let proj = [
        [2.0 * k.fx / w, 0.0, 2.0 * k.cx / w - 1.0, 0.0],
        [0.0, -2.0 * k.fy / h, 1.0 - 2.0 * k.cy / h, 0.0],
        [0.0, 0.0, FAR / (FAR - NEAR), -FAR * NEAR / (FAR - NEAR)],
        [0.0, 0.0, 1.0, 0.0],
    ];
    let view = [
        [r[0][0], r[0][1], r[0][2], t[0]],
        [r[1][0], r[1][1], r[1][2], t[1]],
        [r[2][0], r[2][1], r[2][2], t[2]],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut out = [0f32; 16];
    for row in 0..4 {
        for col in 0..4 {
            let v: f64 = (0..4).map(|i| proj[row][i] * view[i][col]).sum();
            #[allow(clippy::cast_possible_truncation)]
            {
                out[col * 4 + row] = v as f32;
            }
        }
    }
    out
}

struct GpuMesh {
    buffer: wgpu::Buffer,
    vertices: u32,
}

pub(crate) struct ProjectionPass {
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
    meshes: HashMap<String, GpuMesh>,
    depth: Option<((u32, u32), wgpu::TextureView)>,
}

impl ProjectionPass {
    pub(crate) fn new(device: &wgpu::Device, texture_layout: &wgpu::BindGroupLayout) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om projection"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let module = device.create_shader_module(wgpu::include_wgsl!("projection.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om projection"),
            bind_group_layouts: &[Some(texture_layout), Some(&layout)],
            immediate_size: 0,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om projection"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om projection"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            module,
            pipeline_layout,
            pipelines: HashMap::new(),
            buffer,
            bind,
            meshes: HashMap::new(),
            depth: None,
        }
    }

    pub(crate) fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        mesh: &Mesh,
    ) {
        let mut data = Vec::with_capacity(mesh.positions.len() * 5);
        for (p, t) in mesh.positions.iter().zip(&mesh.uvs) {
            data.extend_from_slice(&[p[0], p[1], p[2], t[0], t[1]]);
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om model"),
            size: (data.len() * 4).max(20) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&data));
        self.meshes.insert(
            key.to_owned(),
            GpuMesh {
                buffer,
                vertices: u32::try_from(mesh.positions.len()).unwrap_or(0),
            },
        );
    }

    pub(crate) fn remove_mesh(&mut self, key: &str) {
        self.meshes.remove(key);
    }

    pub(crate) fn has_mesh(&self, key: &str) -> bool {
        self.meshes.contains_key(key)
    }

    pub(crate) fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipeline_layout);
        self.pipelines
            .entry(format)
            .or_insert_with(|| {
                let encode = if format.is_srgb() { 0.0 } else { 1.0 };
                let constants = [("encode_srgb", encode)];
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("om projection"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 20,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                        })],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants: &constants,
                            ..Default::default()
                        },
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
            .clone()
    }

    fn depth_view(&mut self, device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
        if let Some((s, v)) = &self.depth
            && *s == size
        {
            return v.clone();
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("om projection depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some((size, view.clone()));
        view
    }

    /// Draws `key`'s mesh through `projector` onto `target` (cleared to
    /// black). Draws nothing (black) if the mesh is not loaded.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        format: wgpu::TextureFormat,
        canvas_bind: &wgpu::BindGroup,
        key: &str,
        projector: &Projector,
        gain: f32,
    ) {
        let mut uniform = [0f32; 20];
        uniform[..16].copy_from_slice(&view_projection(projector));
        uniform[16] = gain;
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&uniform));
        let pipeline = self.pipeline(device, format);
        let depth = self.depth_view(device, target_size);
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("om projection"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        let Some(mesh) = self.meshes.get(key) else {
            return;
        };
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, canvas_bind, &[]);
        rp.set_bind_group(1, &self.bind, &[]);
        rp.set_vertex_buffer(0, mesh.buffer.slice(..));
        rp.draw(0..mesh.vertices, 0..1);
    }
}
