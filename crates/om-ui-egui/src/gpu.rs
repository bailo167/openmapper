// SPDX-License-Identifier: Apache-2.0
//! Bridges the renderer to egui: renders the project with the UI's GPU
//! device, presents it into a texture egui can draw, and keeps media
//! uploads in sync with the document.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use om_engine::MediaLibrary;
use om_gpu::GpuContext;
use om_project::Project;
use om_render::{Compositor, FrameReport};

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
    pub media: MediaLibrary,
    pub last_frame: Option<FrameReport>,
    pub last_error: Option<String>,
}

impl std::fmt::Debug for Viewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Viewer")
            .field("compositor", &self.compositor)
            .finish_non_exhaustive()
    }
}

impl Viewer {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let gpu = GpuContext::from_parts(
            render_state.adapter.clone(),
            render_state.device.clone(),
            render_state.queue.clone(),
        );
        Self {
            render_state: render_state.clone(),
            compositor: Compositor::new(gpu),
            preview: None,
            media: MediaLibrary::new(),
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
    ) -> Option<egui::TextureId> {
        let changes = self.media.sync(project, project_dir);
        self.apply_media(&changes);
        match self.render(project) {
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

    /// Retries media that failed to load (missing files may have appeared).
    pub fn retry_media(&mut self) {
        let changes = self.media.retry_failed();
        self.apply_media(&changes);
    }

    fn apply_media(&mut self, changes: &om_engine::MediaChanges) {
        for id in &changes.unloaded {
            self.compositor.remove_image(*id);
        }
        for id in &changes.loaded {
            if let Some(img) = self.media.image(*id)
                && let Err(e) = self.compositor.set_image(*id, img)
            {
                self.last_error = Some(e.to_string());
            }
        }
    }

    fn render(&mut self, project: &Project) -> Result<egui::TextureId, String> {
        let report = self.compositor.render(project).map_err(|e| e.to_string())?;
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
