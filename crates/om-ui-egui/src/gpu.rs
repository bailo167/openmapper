// SPDX-License-Identifier: Apache-2.0
//! Bridges the renderer to egui: renders the project with the UI's GPU
//! device, presents it into a texture egui can draw, and keeps media
//! uploads in sync with the document.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

use om_engine::{Adapters, AudioSetup, MediaRuntime, PublishRuntime, Transport, audio_clock};
use om_gpu::GpuContext;
use om_project::Project;
use om_render::{Compositor, FrameReader, FrameReport};
use om_time::RationalTime;

/// Format egui expects for user textures (sRGB-encoded values, see egui-wgpu).
const PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Preview {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    id: egui::TextureId,
}

/// Renderer state owned by the UI.
pub struct Viewer {
    render_state: egui_wgpu::RenderState,
    compositor: Compositor,
    preview: Option<Preview>,
    /// Mapped output images (outputs with a non-identity mapping).
    outputs: std::collections::HashMap<om_types::OutputId, Preview>,
    /// 3-D models loaded for projected outputs, or why loading failed.
    pub models: std::collections::HashMap<String, Result<usize, String>>,
    pub media: MediaRuntime,
    /// Syphon/Spout/NDI/stream publishing of the output frame.
    pub publish: PublishRuntime,
    /// Art-Net/sACN output sampled from the canvas (LED pixel mapping).
    pub dmx: om_dmx::DmxRuntime,
    /// WebAssembly plugin filters on media.
    pub plugins: om_engine::plugins::PluginStage,
    reader: FrameReader,
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
        adapters: &Adapters,
    ) -> Self {
        let audio_opener = adapters.audio.clone();
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
            outputs: std::collections::HashMap::new(),
            models: std::collections::HashMap::new(),
            media: MediaRuntime::with_audio(adapters.video.clone(), audio)
                .with_live(adapters.live.clone()),
            publish: PublishRuntime::new(adapters.sinks.clone()),
            dmx: om_dmx::DmxRuntime::new(),
            plugins: om_engine::plugins::PluginStage::new(),
            reader: FrameReader::new(&render_state.device, &render_state.queue),
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
        self.plugins.sync(project, project_dir);
        self.apply_media(&changes, show.as_seconds_f64());
        let inputs = om_render::FrameInputs {
            show_seconds: show.as_seconds_f64(),
            media_seconds: changes.shader_times.iter().copied().collect(),
        };
        self.sync_models(project, project_dir);
        if let Some(m) = &self.mixer {
            // For ISF audio inputs; a 1024-point FFT per channel is cheap.
            let scope = m.scope();
            self.compositor.set_audio(om_render::audio::AudioFrame {
                wave: &scope.wave,
                fft: &scope.fft,
            });
        }
        let rendered = self.render(project, &inputs);
        self.publish.sync(project);
        self.dmx.sync(project);
        if self.publish.is_active() || self.dmx.is_active() {
            if let (Ok(_), Some(p)) = (&rendered, &self.preview) {
                self.reader.capture(&p.texture);
            }
            if let Some(frame) = self.reader.poll() {
                if self.dmx.is_active() {
                    self.dmx.submit(&om_dmx::mapping::FrameView {
                        width: frame.width(),
                        height: frame.height(),
                        rgba8: frame.rgba8(),
                    });
                }
                if self.publish.is_active() {
                    self.publish.submit_all(&Arc::new(frame));
                }
            }
        }
        match rendered {
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

    fn apply_media(&mut self, changes: &om_engine::MediaChanges, time: f64) {
        for id in &changes.unload {
            self.compositor.remove_image(*id);
            self.thumbs.remove(id);
        }
        for (id, img) in &changes.upload {
            if self.plugins.has_chain(*id) {
                self.plugins.submit(*id, img, time);
            }
            // The original shows until plugins produce output (or if they
            // cannot run).
            if self.plugins.shows_original(*id)
                && let Err(e) = self.compositor.set_image(*id, img)
            {
                self.last_error = Some(e.to_string());
            }
            self.update_thumbnail(*id, img);
        }
        for (id, img) in self.plugins.poll() {
            if let Err(e) = self.compositor.set_image(id, &img) {
                self.last_error = Some(e.to_string());
            }
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
        let mut slot = self.preview.take();
        ensure_target(&self.render_state, &mut slot, size, "om preview");
        self.preview = slot;
    }

    /// Loads the OBJ models projected outputs use (once per path) and
    /// unloads models no output uses any more.
    fn sync_models(&mut self, project: &Project, project_dir: Option<&std::path::Path>) {
        let wanted: std::collections::HashSet<&str> = project
            .outputs
            .iter()
            .filter_map(|o| o.projection.as_ref().map(|p| p.model.as_str()))
            .collect();
        let stale: Vec<String> = self
            .models
            .keys()
            .filter(|k| !wanted.contains(k.as_str()))
            .cloned()
            .collect();
        for key in stale {
            self.compositor.remove_model(&key);
            self.models.remove(&key);
        }
        for key in wanted {
            if self.models.contains_key(key) {
                continue;
            }
            let path = std::path::Path::new(key);
            let full = match project_dir {
                Some(dir) if path.is_relative() => dir.join(path),
                _ => path.to_path_buf(),
            };
            let loaded = std::fs::read_to_string(&full)
                .map_err(|e| format!("{}: {e}", full.display()))
                .and_then(|text| om_geom::obj::parse(&text).map_err(|e| e.to_string()));
            let entry = match loaded {
                Ok(mesh) => {
                    self.compositor.set_model(key, &mesh);
                    Ok(mesh.triangles())
                }
                Err(e) => Err(e),
            };
            self.models.insert(key.to_owned(), entry);
        }
    }

    /// Forgets a model so it is read again from disk on the next frame.
    pub fn reload_model(&mut self, key: &str) {
        self.compositor.remove_model(key);
        self.models.remove(key);
    }

    /// The image `output` should show at `size`, rendered from the latest
    /// frame: its 3-D projection, or the canvas through its mapping.
    /// Unmapped outputs share the preview texture.
    pub fn output_texture(
        &mut self,
        output: &om_project::Output,
        size: (u32, u32),
    ) -> Option<egui::TextureId> {
        let id = output.id;
        if output.projection.is_none() && output.mapping.is_identity() {
            self.outputs.remove(&id);
            return self.preview_id();
        }
        let size = (size.0.clamp(1, 8192), size.1.clamp(1, 8192));
        let mut slot = self.outputs.remove(&id);
        ensure_target(&self.render_state, &mut slot, size, "om output");
        let target = slot?;
        let mut encoder =
            self.render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("om output"),
                });
        let ok = self
            .compositor
            .present_output(&mut encoder, &target.view, PREVIEW_FORMAT, size, output)
            .is_ok();
        self.render_state.queue.submit([encoder.finish()]);
        let tex = target.id;
        self.outputs.insert(id, target);
        ok.then_some(tex)
    }
}

/// Creates or resizes a render target registered with egui.
fn ensure_target(
    render_state: &egui_wgpu::RenderState,
    slot: &mut Option<Preview>,
    size: (u32, u32),
    label: &str,
) {
    if slot.as_ref().is_some_and(|p| p.size == size) {
        return;
    }
    let device = &render_state.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: PREVIEW_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut renderer = render_state.renderer.write();
    let id = match slot.take() {
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
    *slot = Some(Preview {
        size,
        texture,
        view,
        id,
    });
}
