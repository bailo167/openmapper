// SPDX-License-Identifier: Apache-2.0
//! ISF runtime conformance: corpus shaders on the GPU vs analytic results.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::HashMap;

use om_gpu::{GpuContext, WORKING_FORMAT};
use om_render::isf::{IsfProgram, IsfValue};

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(g) => Some(g),
        Err(e) if std::env::var("OM_REQUIRE_GPU").as_deref() == Ok("1") => panic!("{e}"),
        Err(_) => None,
    }
}

fn program(g: &GpuContext, name: &str) -> IsfProgram {
    let src = std::fs::read_to_string(format!(
        "{}/../om-isf/tests/corpus/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let compiled = om_isf::compile(&om_isf::parse(&src).unwrap()).unwrap();
    IsfProgram::new(g, &compiled).unwrap()
}

fn texture(g: &GpuContext, w: u32, h: u32) -> wgpu::Texture {
    g.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Reads an Rgba16Float texture as f32 RGBA, top row first.
fn read(g: &GpuContext, tex: &wgpu::Texture) -> Vec<[f32; 4]> {
    let (w, h) = (tex.width(), tex.height());
    let row = w * 8;
    let padded = row.div_ceil(256) * 256;
    let buf = g.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device().create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
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
    g.queue().submit([enc.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |_| {});
    g.device()
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let data = buf.get_mapped_range(..).unwrap();
    let mut out = Vec::new();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = y * padded as usize + x * 8;
            let c = |k: usize| {
                half::f16::from_le_bytes([data[o + 2 * k], data[o + 2 * k + 1]]).to_f32()
            };
            out.push([c(0), c(1), c(2), c(3)]);
        }
    }
    out
}

fn run(
    g: &GpuContext,
    p: &mut IsfProgram,
    size: (u32, u32),
    time: f64,
    values: &HashMap<String, IsfValue>,
    images: &HashMap<String, &wgpu::TextureView>,
) -> Vec<[f32; 4]> {
    let out = texture(g, size.0, size.1);
    let view = out.create_view(&Default::default());
    let mut enc = g.device().create_command_encoder(&Default::default());
    let frame = om_render::isf::IsfFrame {
        size,
        time,
        values,
        images,
        output: &view,
    };
    p.render(g, &mut enc, &frame).unwrap();
    g.queue().submit([enc.finish()]);
    read(g, &out)
}

fn close(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tol)
}

#[test]
fn solid_and_inputs() {
    let Some(g) = gpu() else { return };
    let none = HashMap::new();
    let mut p = program(&g, "solid.fs");
    let px = run(&g, &mut p, (4, 3), 0.0, &HashMap::new(), &none);
    assert!(
        px.iter().all(|c| close(*c, [0.25, 0.5, 0.75, 1.0], 1e-3)),
        "{:?}",
        px[0]
    );

    let mut p = program(&g, "inputs.fs");
    let px = run(&g, &mut p, (2, 2), 0.0, &HashMap::new(), &none);
    assert!(
        close(px[0], [0.5, 0.25, 0.75, 0.75], 1e-3),
        "defaults: {:?}",
        px[0]
    );
    let values = HashMap::from([
        ("level".to_owned(), IsfValue::Number(0.125)),
        ("mode".to_owned(), IsfValue::Number(2.0)),
        ("flag".to_owned(), IsfValue::Bool(false)),
        ("pos".to_owned(), IsfValue::Vector(vec![0.1, 0.9])),
        ("trigger".to_owned(), IsfValue::Bool(true)),
    ]);
    let px = run(&g, &mut p, (2, 2), 0.0, &values, &none);
    assert!(
        close(px[0], [0.125, 0.5, 0.1, 0.4], 1e-3),
        "overrides: {:?}",
        px[0]
    );
}

#[test]
fn coordinates_follow_gl_conventions() {
    let Some(g) = gpu() else { return };
    let mut p = program(&g, "gradient.fs");
    let (w, h) = (8u32, 4u32);
    let px = run(&g, &mut p, (w, h), 0.0, &HashMap::new(), &HashMap::new());
    for y in 0..h {
        for x in 0..w {
            let nx = (x as f32 + 0.5) / w as f32;
            let ny = (h as f32 - (y as f32 + 0.5)) / h as f32; // bottom-left origin
            let c = px[(y * w + x) as usize];
            assert!(close(c, [nx, ny, 0.0, 1.0], 2e-3), "({x},{y}): {c:?}");
        }
    }
}

#[test]
fn filter_reads_input_image_in_place() {
    let Some(g) = gpu() else { return };
    let mut p = program(&g, "passthrough.fs");
    let (w, h) = (6u32, 4u32);
    // Input: each pixel encodes its position.
    let input = texture(&g, w, h);
    let mut texels = Vec::new();
    for y in 0..h {
        for x in 0..w {
            for v in [x as f32 / 8.0, y as f32 / 8.0, 0.5, 1.0] {
                texels.extend_from_slice(&half::f16::from_f32(v).to_le_bytes());
            }
        }
    }
    g.queue().write_texture(
        input.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 8),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = input.create_view(&Default::default());
    let images = HashMap::from([("inputImage".to_owned(), &view)]);
    let values = HashMap::from([
        ("gain".to_owned(), IsfValue::Number(2.0)),
        ("swap".to_owned(), IsfValue::Bool(true)),
    ]);
    let px = run(&g, &mut p, (w, h), 0.0, &values, &images);
    for y in 0..h {
        for x in 0..w {
            // swap = bgra, then rgb * 2
            let expected = [0.5 * 2.0, y as f32 / 8.0 * 2.0, x as f32 / 8.0 * 2.0, 1.0];
            let c = px[(y * w + x) as usize];
            assert!(close(c, expected, 2e-3), "({x},{y}): {c:?} vs {expected:?}");
        }
    }
}

#[test]
fn time_frames_and_size() {
    let Some(g) = gpu() else { return };
    let mut p = program(&g, "time.fs");
    let none = HashMap::new();
    for frame in 0..3 {
        let px = run(
            &g,
            &mut p,
            (500, 2),
            2.25 + f64::from(frame),
            &HashMap::new(),
            &none,
        );
        assert!(
            close(px[0], [0.25, frame as f32 / 100.0, 0.5, 1.0], 1e-3),
            "frame {frame}: {:?}",
            px[0]
        );
    }
}

#[test]
fn multipass_targets_are_sized_and_readable() {
    let Some(g) = gpu() else { return };
    let mut p = program(&g, "multipass.fs");
    let (w, h) = (64u32, 32u32);
    let px = run(&g, &mut p, (w, h), 0.0, &HashMap::new(), &HashMap::new());
    for y in [1u32, 15, 30] {
        let c = px[(y * w + 10) as usize];
        let ny = (h as f32 - (y as f32 + 0.5)) / h as f32;
        assert!(
            (c[0] - 1.0).abs() < 2e-3,
            "pass 1 doubles pass 0's 0.5: {c:?}"
        );
        assert!(
            (c[1] - ny).abs() < 2.0 / h as f32,
            "y gradient through half-res target: {c:?} vs {ny}"
        );
        assert!(
            (c[2] - 0.032).abs() < 1e-3,
            "half target is {}px wide: {c:?}",
            w / 2
        );
    }
}

#[test]
fn persistent_buffers_accumulate_across_frames() {
    let Some(g) = gpu() else { return };
    let mut p = program(&g, "feedback.fs");
    let none = HashMap::new();
    for frame in 1..=5 {
        let px = run(&g, &mut p, (4, 4), 0.0, &HashMap::new(), &none);
        let expected = 0.1 * frame as f32;
        assert!(
            (px[5][0] - expected).abs() < 5e-3,
            "frame {frame}: {:?}",
            px[5]
        );
    }
    p.reset();
    let px = run(&g, &mut p, (4, 4), 0.0, &HashMap::new(), &none);
    assert!((px[5][0] - 0.1).abs() < 5e-3, "reset clears history");
}
