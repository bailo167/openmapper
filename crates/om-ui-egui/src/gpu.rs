// SPDX-License-Identifier: Apache-2.0
//! Bridges the renderer to egui: renders the project with the UI's GPU
//! device, presents it into a texture egui can draw, and keeps media
//! uploads in sync with the document.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

use om_engine::{AudioSetup, MediaRuntime, Transport, audio_clock};
use om_gpu::GpuContext;
use om_media_core::{AudioOpener, VideoOpener};
use om_project::Project;
use om_render::{Compositor, FrameReport};
use om_time::RationalTime;

/// Format egui expects for user textures (sRGB-encoded values, see egui-wgpu).
const PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Preview {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    id: egui::TextureId,
}

/// Renderer state owned by the UI.
pub struct Viewer {
    render_state: egui_wgpu::RenderState,
    compositor: Compositor,
    preview: Option<Preview>,
    pub media: MediaRuntime,
    mixer: Option<om_audio::Mixer>,
    _audio_out: Option<om_audio::Output>,
    /// Audio device in use, or why audio is unavailable.
    pub audio_status: String,
    pub last_frame: Option<FrameReport>,
    pub last_error: Option<String>,
    ctx: egui::Context,
    thumbs: std::collections::HashMap<om_types::MediaId, (egui::TextureHandle, std::time::Instant)>,
}

impl std::fmt::Debug for Viewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Viewer")
            .field("compositor", &self.compositor)
            .finish_non_exhaustive()
    }
}

impl Viewer {
    pub fn new(
        ctx: &egui::Context,
        render_state: &egui_wgpu::RenderState,
        opener: Option<Arc<dyn VideoOpener>>,
        audio_opener: Option<Arc<dyn AudioOpener>>,
    ) -> Self {
        // Audio is optional: a machine without an output device still maps.
        let (mixer, audio_out, audio_status) =
            match audio_opener.as_ref().map(|_| om_audio::default_rate()) {
                None => (None, None, "audio disabled".to_owned()),
                Some(Err(e)) => (None, None, format!("no audio: {e}")),
                Some(Ok(rate)) => {
                    let mixer = om_audio::Mixer::new(rate);
                    match om_audio::Output::open(mixer.clone()) {
                        Ok(out) => {
                            let status = format!("audio: {} @ {} Hz", out.device_name, out.rate);
                            (Some(mixer), Some(out), status)
                        }
                        Err(e) => (None, None, format!("no audio: {e}")),
                    }
                }
            };
        let audio = match (&mixer, audio_opener) {
            (Some(m), Some(o)) => Some(AudioSetup {
                opener: o,
                mixer: m.clone(),
            }),
            _ => None,
        };
        let gpu = GpuContext::from_parts(
            render_state.adapter.clone(),
            render_state.device.clone(),
            render_state.queue.clone(),
        );
        Self {
            render_state: render_state.clone(),
            ctx: ctx.clone(),
            thumbs: std::collections::HashMap::new(),
            compositor: Compositor::new(gpu),
            preview: None,
            media: MediaRuntime::with_audio(opener, audio),
            mixer,
            _audio_out: audio_out,
            audio_status,
            last_frame: None,
            last_error: None,
        }
    }

    /// Texture id of the last presented frame.
    pub fn preview_id(&self) -> Option<egui::TextureId> {
        self.preview.as_ref().map(|p| p.id)
    }

    pub fn gpu_summary(&self) -> String {
        self.compositor.gpu().capabilities().to_string()
    }

    /// Loads changed media, renders the project and presents it into the
    /// preview texture. Returns the texture to draw, if rendering succeeded.
    pub fn update(
        &mut self,
        project: &Project,
        project_dir: Option<&std::path::Path>,
        show: RationalTime,
        transport: &Transport,
    ) -> Option<egui::TextureId> {
        if let Some(m) = &self.mixer {
            m.set_clock(audio_clock(transport, m.rate()));
        }
        let changes = self.media.update(project, project_dir, show);
        self.apply_media(&changes);
        let inputs = om_render::FrameInputs {
            show_seconds: show.as_seconds_f64(),
            media_seconds: changes.shader_times.iter().copied().collect(),
        };
        match self.render(project, &inputs) {
            Ok(id) => {
                self.last_error = None;
                Some(id)
            }
            Err(e) => {
                self.last_error = Some(e);
                None
            }
        }
    }

    fn apply_media(&mut self, changes: &om_engine::MediaChanges) {
        for id in &changes.unload {
            self.compositor.remove_image(*id);
            self.thumbs.remove(id);
        }
        for (id, img) in &changes.upload {
            if let Err(e) = self.compositor.set_image(*id, img) {
                self.last_error = Some(e.to_string());
            }
            self.update_thumbnail(*id, img);
        }
        for path in &changes.shaders_removed {
            self.compositor.remove_shader(path);
        }
        for (path, compiled) in &changes.shaders {
            self.compositor.set_shader(path, compiled);
        }
    }

    /// Audio analysis of what is playing (zeros without an audio device).
    pub fn audio_levels(&self) -> om_audio::analysis::Levels {
        self.mixer
            .as_ref()
            .map(om_audio::Mixer::levels)
            .unwrap_or_default()
    }

    /// Shader compile/pipeline error for a stored shader path.
    pub fn shader_error(&self, path: &str) -> Option<String> {
        self.media
            .shader_error(path)
            .or_else(|| self.compositor.shader_error(path))
            .map(str::to_owned)
    }

    /// Live thumbnail, refreshed at most twice a second.
    fn update_thumbnail(&mut self, id: om_types::MediaId, img: &om_media_core::StillImage) {
        const REFRESH: std::time::Duration = std::time::Duration::from_millis(500);
        if self
            .thumbs
            .get(&id)
            .is_some_and(|(_, t)| t.elapsed() < REFRESH)
        {
            return;
        }
        let t = img.thumbnail(96);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [t.width() as usize, t.height() as usize],
            t.rgba8(),
        );
        let now = std::time::Instant::now();
        match self.thumbs.get_mut(&id) {
            Some((handle, at)) => {
                handle.set(color, egui::TextureOptions::LINEAR);
                *at = now;
            }
            None => {
                let handle = self.ctx.load_texture(
                    format!("thumb-{id}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbs.insert(id, (handle, now));
            }
        }
    }

    /// Thumbnail texture for a media item, once it has pixels.
    pub fn thumbnail(&self, id: om_types::MediaId) -> Option<&egui::TextureHandle> {
        self.thumbs.get(&id).map(|(h, _)| h)
    }

    fn render(
        &mut self,
        project: &Project,
        inputs: &om_render::FrameInputs,
    ) -> Result<egui::TextureId, String> {
        let report = self
            .compositor
            .render_with(project, inputs)
            .map_err(|e| e.to_string())?;
        let size = report.canvas;
        self.last_frame = Some(report);
        self.ensure_preview(size);
        let preview = self.preview.as_ref().ok_or("preview unavailable")?;
        let mut encoder =
            self.render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("om preview"),
                });
        self.compositor
            .present(&mut encoder, &preview.view, PREVIEW_FORMAT)
            .map_err(|e| e.to_string())?;
        self.render_state.queue.submit([encoder.finish()]);
        Ok(preview.id)
    }

    fn ensure_preview(&mut self, size: (u32, u32)) {
        if self.preview.as_ref().is_some_and(|p| p.size == size) {
            return;
        }
        let device = &self.render_state.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("om preview"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PREVIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = self.render_state.renderer.write();
        let id = match self.preview.take() {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &view,
                    wgpu::FilterMode::Linear,
                    old.id,
                );
                old.id
            }
            None => renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear),
        };
        self.preview = Some(Preview {
            size,
            _texture: texture,
            view,
            id,
        });
    }
}
