// SPDX-License-Identifier: Apache-2.0
//! Output mapping pass: shows a canvas region on an output through a
//! corner pin, with soft edges (see `output.wgsl` and
//! docs/output-mapping.md).

use std::collections::HashMap;

use om_geom::Homography;
use om_project::OutputMapping;

/// Uniform block size (two 3×3 matrices as padded columns + two vec4).
const UNIFORM_SIZE: u64 = 128;

/// The uniform contents for `mapping`, or `None` if its quads cannot be
/// mapped (validation prevents this for stored projects).
#[must_use]
pub fn uniform(mapping: &OutputMapping) -> Option<[f32; 32]> {
    let warp_inv = Homography::square_to_quad(&mapping.warp)
        .ok()?
        .inverse()
        .ok()?;
    let region = Homography::square_to_quad(&mapping.region).ok()?;
    let mut u = [0f32; 32];
    let mut put = |at: usize, cols: [[f32; 4]; 3]| {
        for (c, col) in cols.iter().enumerate() {
            u[at + 4 * c..at + 4 * c + 4].copy_from_slice(col);
        }
    };
    put(0, warp_inv.to_gpu_cols());
    put(12, region.to_gpu_cols());
    let e = &mapping.soft_edge;
    #[allow(clippy::cast_possible_truncation)]
    {
        u[24] = e.left.get() as f32;
        u[25] = e.right.get() as f32;
        u[26] = e.top.get() as f32;
        u[27] = e.bottom.get() as f32;
        u[28] = e.curve.get() as f32;
        u[29] = e.gamma.get() as f32;
    }
    Some(u)
}

pub(crate) struct OutputPass {
    _layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl OutputPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        texture_layout: &wgpu::BindGroupLayout,
        master_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("om output map"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let module = device.create_shader_module(wgpu::include_wgsl!("output.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("om output map"),
            bind_group_layouts: &[Some(texture_layout), Some(master_layout), Some(&layout)],
            immediate_size: 0,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("om output map"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("om output map"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            _layout: layout,
            module,
            pipeline_layout,
            pipelines: HashMap::new(),
            buffer,
            bind,
        }
    }

    pub(crate) fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipeline_layout);
        self.pipelines.entry(format).or_insert_with(|| {
            let encode = if format.is_srgb() { 0.0 } else { 1.0 };
            let constants = [("encode_srgb", encode)];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("om output map"),
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
    }

    pub(crate) fn write(&self, queue: &wgpu::Queue, uniform: &[f32; 32]) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(uniform));
    }

    pub(crate) fn bind(&self) -> &wgpu::BindGroup {
        &self.bind
    }
}
