//! nui-render: the wgpu renderer.
//!
//! M3 contents: the rect pipeline ([`rect`], SDF rounded rectangles with
//! `fwidth` AA, instanced), the display-list scene graph built from the
//! element tree ([`scene`]), and the [`Renderer`] that draws a scene to a
//! wgpu surface or offscreen target. Text/image pipelines and effects are
//! M3 follow-up / M6.
//!
//! Coordinates arrive in dp and are scaled to physical pixels at submission
//! (plan §7.2: the engine is dpi-free; only submission multiplies).

pub mod image;
pub mod layer;
pub mod rect;
pub mod scene;
pub mod text;

pub use image::{
    DecodedImage, ImageDraw, ImageInstance, ImageLoadError, ImagePipeline, cache_key, content_hash,
    decode_file, nine_slice_quads,
};
pub use layer::{BlurPipeline, LayerInstance, LayerPipeline};
pub use rect::{CameraUniform, RectInstance, RectPipeline};
pub use scene::{Scene, SceneBuilder, SceneContext, TextDraw};
pub use text::{TextInstance, TextPipeline};

use std::collections::HashMap;

use nui_core::Size;

/// The wgpu renderer: owns device-independent state and draws scenes.
pub struct Renderer {
    /// The rect pipeline.
    pub rect: RectPipeline,
    /// The text pipeline (M6 glyph atlas quads).
    pub text: TextPipeline,
    /// The image pipeline (M8 textured quads).
    pub image: ImagePipeline,
    /// The layer compositing pipeline (M9 offscreen layers).
    pub layer: LayerPipeline,
    /// The blur pipeline (M9 two-pass gaussian).
    pub blur: BlurPipeline,
    /// The target format all pipelines were built for.
    format: wgpu::TextureFormat,
}

impl Renderer {
    /// Creates a renderer for `device` targeting `format`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        return Renderer {
            rect: RectPipeline::new(device, format),
            text: TextPipeline::new(device, format),
            image: ImagePipeline::new(device, format),
            layer: LayerPipeline::new(device, format),
            blur: BlurPipeline::new(device, format),
            format,
        };
    }

    /// Uploads camera + instances for this frame. `text` syncs the glyph
    /// atlas and the glyph instances; `None` skips the text pipeline.
    /// `decoded_images` feeds not-yet-uploaded image textures.
    // The parameter list mirrors the per-pipeline upload sequence; bundling
    // would hide the symmetry with the render pass.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport: Size,
        scale: f32,
        scene: &Scene,
        text: Option<&mut nui_text::TextSystem>,
        decoded_images: &HashMap<String, DecodedImage>,
    ) {
        self.rect
            .set_viewport(queue, viewport.width * scale, viewport.height * scale);
        self.text
            .set_viewport(queue, viewport.width * scale, viewport.height * scale);
        self.image
            .set_viewport(queue, viewport.width * scale, viewport.height * scale);
        let mut instances = Vec::with_capacity(scene.rects.len());
        for rect in &scene.rects {
            let mut instance =
                RectInstance::from_dp(rect.geometry, rect.corner_radius, rect.fill, scale);
            if let Some(shadow) = &rect.shadow {
                instance = instance.with_shadow(
                    [shadow.offset.x, shadow.offset.y],
                    shadow.blur,
                    shadow.color,
                    scale,
                );
            }
            if let Some(clip) = &rect.clip {
                instance = instance.with_clip(clip.bounds, clip.radius, scale);
            }
            instances.push(instance);
        }
        self.rect.upload_instances(device, queue, &instances);
        // Images: upload not-yet-uploaded textures, then expand draws.
        for (key, decoded) in decoded_images {
            if !self.image.has_texture(key) {
                self.image.upload_texture(device, queue, key, decoded);
            }
        }
        self.image.prepare(device, queue, scale, &scene.images);
        // Text: sync glyph atlas and upload glyph instances.
        if let Some(text) = text {
            self.text.sync_atlas(device, queue, text.atlas());
            // Bucket per atlas page (usually one), preserving paint order.
            let page_count = self.text.page_count().max(
                scene
                    .texts
                    .iter()
                    .map(|draw| return draw.page as usize + 1)
                    .max()
                    .unwrap_or(1),
            );
            let mut bucketed: Vec<Vec<TextInstance>> = vec![Vec::new(); page_count];
            for draw in &scene.texts {
                bucketed[draw.page as usize].push(TextInstance::from_dp(
                    draw.origin,
                    draw.size,
                    draw.mask,
                    nui_text::atlas_page_size(),
                    draw.color,
                    scale,
                    draw.clip,
                ));
            }
            self.text.upload_instances(device, queue, &bucketed);
        }
    }

    /// Encodes the draw commands for the current frame: rects, then images,
    /// then glyphs (document paint order within each list). Layers must be
    /// prepared first — use [`Renderer::render_to_view`].
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, scene: &Scene) {
        self.rect.render(pass, scene.rects.len() as u32);
        self.image.render(pass);
        self.text.render(pass);
        self.layer.render(pass, scene.layers.len() as u32);
    }

    /// Renders `scene` into `view`: prepares all pipelines, renders nested
    /// offscreen layers (recursively), blurs them, then encodes the main
    /// pass and submits. `physical` is the target size in physical pixels.
    // Mirrors the frame pipeline sequence (prepare -> layers -> pass);
    // bundling would hide the ordering that makes instance uploads safe.
    #[allow(clippy::too_many_arguments)]
    pub fn render_to_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        physical: Size,
        scale: f32,
        scene: &Scene,
        mut text: Option<&mut nui_text::TextSystem>,
        decoded_images: &HashMap<String, DecodedImage>,
        clear: wgpu::Color,
    ) {
        // Nested layers render into their own textures FIRST; each level
        // submits its own encoder, so per-scene instance uploads stay
        // ordered with their passes. The top-level prepare runs last —
        // nested prepares overwrite the shared instance buffers.
        self.render_scene_layers(device, queue, scale, scene, &mut text);
        let logical = Size::new(physical.width / scale, physical.height / scale);
        self.prepare(device, queue, logical, scale, scene, text, decoded_images);
        self.layer
            .set_viewport(queue, physical.width, physical.height);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nui-frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nui-main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.render(&mut pass, scene);
        }
        queue.submit([encoder.finish()]);
    }

    /// Renders every layer of `scene` into a texture (blur included) and
    /// prepares the layer pipeline for the main pass. Recurses for nested
    /// layers through [`Renderer::render_offscreen`].
    fn render_scene_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scale: f32,
        scene: &Scene,
        text: &mut Option<&mut nui_text::TextSystem>,
    ) {
        let mut instances: Vec<LayerInstance> = Vec::new();
        let mut views: Vec<wgpu::TextureView> = Vec::new();
        for layer in &scene.layers {
            let width = (layer.size.width * scale).ceil().max(1.0) as u32;
            let height = (layer.size.height * scale).ceil().max(1.0) as u32;
            let texture =
                self.render_offscreen(device, queue, width, height, scale, &layer.scene, text);
            // Sync before the next sibling's work: on lavapipe, the staged
            // `write_buffer` copies of a later layer's `prepare` were
            // observed landing while the previous layer's pass was still
            // pending, corrupting its result (plan §M9 已知问题). One
            // blocking poll per layer — layers are rare, correctness first.
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("layer sync completes");

            let view = if layer.blur > 0.0 {
                let blurred =
                    self.blur_texture(device, queue, texture, width, height, layer.blur * scale);
                blurred.create_view(&wgpu::TextureViewDescriptor::default())
            } else {
                texture.create_view(&wgpu::TextureViewDescriptor::default())
            };
            views.push(view);
            instances.push(LayerInstance {
                origin: [layer.origin.x * scale, layer.origin.y * scale, 0.0, 0.0],
                size_opacity: [
                    layer.size.width * scale,
                    layer.size.height * scale,
                    layer.opacity,
                    0.0,
                ],
                _pad: [0.0; 4],
            });
        }
        self.layer.prepare(device, queue, &instances, &views);
        // The pass that draws the layer quads belongs to the CALLER's
        // encoder; its viewport matches the caller's target and is set
        // there.
    }

    /// Renders a sub-scene into a fresh offscreen texture (transparent
    /// background), handling its nested layers.
    #[allow(clippy::too_many_arguments)]
    fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        scale: f32,
        scene: &Scene,
        text: &mut Option<&mut nui_text::TextSystem>,
    ) -> wgpu::Texture {
        self.render_scene_layers(device, queue, scale, scene, text);
        let texture = self.blur.create_target(device, self.format, width, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let logical = Size::new(width as f32 / scale, height as f32 / scale);
        self.prepare(
            device,
            queue,
            logical,
            scale,
            scene,
            text.as_deref_mut(),
            &HashMap::new(),
        );
        self.layer.set_viewport(queue, width as f32, height as f32);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nui-layer-frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nui-layer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.render(&mut pass, scene);
        }
        queue.submit([encoder.finish()]);
        return texture;
    }

    /// Two-pass gaussian blur: horizontal then vertical over ping-pong
    /// targets; returns the blurred texture (plan §6.4).
    fn blur_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: wgpu::Texture,
        width: u32,
        height: u32,
        blur_px: f32,
    ) -> wgpu::Texture {
        let ping = self.blur.create_target(device, self.format, width, height);
        let pong = self.blur.create_target(device, self.format, width, height);
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let ping_view = ping.create_view(&wgpu::TextureViewDescriptor::default());
        let pong_view = pong.create_view(&wgpu::TextureViewDescriptor::default());
        // One submit per pass: both passes share the uniform buffer, so a
        // batched submit would run both with the second step value. The
        // step is in normalized UV units (tap offsets in the shader are
        // UV deltas), hence the division by the texture size.
        let step_x = blur_px / 3.0 / width as f32;
        let step_y = blur_px / 3.0 / height as f32;
        {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nui-blur-horizontal"),
            });
            self.blur.encode_pass(
                device,
                queue,
                &mut encoder,
                &source_view,
                &ping_view,
                [step_x, 0.0],
            );
            queue.submit([encoder.finish()]);
        }
        {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nui-blur-vertical"),
            });
            self.blur.encode_pass(
                device,
                queue,
                &mut encoder,
                &ping_view,
                &pong_view,
                [0.0, step_y],
            );
            queue.submit([encoder.finish()]);
        }
        return pong;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_round_trips_rect_count() {
        let mut builder = SceneBuilder::new();
        builder.push_rect(
            nui_core::Rect::new(nui_core::Point::ZERO, nui_core::Size::new(10.0, 10.0)),
            2.0,
            nui_core::Color::from_rgb8(1, 2, 3),
        );
        let scene = builder.build();
        assert_eq!(scene.rects.len(), 1);
    }
}
