//! Text pipeline: one instanced, atlas-textured quad per rasterized glyph
//! (plan §6.2). Masks are 8-bit alpha in the atlas; the fragment shader
//! multiplies the instance color (premultiplied) by the sampled alpha.
//!
//! Instances are bucketed per atlas page at prepare time; each page draws
//! with its own bind group (texture) while sharing one instance buffer.

use bytemuck::{Pod, Zeroable};
use nui_core::{Color, Point, Size};

/// Per-instance GPU data: screen quad, atlas UVs, tint, and clip region.
///
/// Layout mirrors the WGSL `TextData` struct: four `vec2`s, a `vec4`, then
/// clip bounds (`vec4`), clip radius, and padding — 80 bytes total.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct TextInstance {
    /// Top-left corner in physical pixels.
    pub origin: [f32; 2],
    /// Quad size in physical pixels.
    pub size: [f32; 2],
    /// Atlas UV top-left.
    pub uv_origin: [f32; 2],
    /// Atlas UV extent.
    pub uv_size: [f32; 2],
    /// Tint color (premultiplied rgba).
    pub color: [f32; 4],
    /// Clip bounds in physical px; unused when clip_radius < 0.
    pub clip_bounds: [f32; 4],
    /// Clip corner radius; negative = no clipping.
    pub clip_radius: f32,
    /// Padding to 80 bytes.
    pub _pad: [f32; 3],
}

impl TextInstance {
    /// Builds an instance from a dp-space quad plus the atlas texel rect,
    /// applying the dpi scale and the half-texel UV inset (linear sampling
    /// must hit texel centers, not bleed across glyph boundaries).
    pub fn from_dp(
        origin_dp: Point,
        size_dp: Size,
        mask: (u32, u32, u32, u32),
        page_size: u32,
        color: Color,
        scale: f32,
        clip: Option<crate::scene::ClipDraw>,
    ) -> TextInstance {
        let (x, y, width, height) = mask;
        let page = page_size as f32;
        let (clip_bounds, clip_radius) = match clip {
            Some(clip) => (
                [
                    clip.bounds.origin.x * scale,
                    clip.bounds.origin.y * scale,
                    clip.bounds.size.width * scale,
                    clip.bounds.size.height * scale,
                ],
                clip.radius * scale,
            ),
            None => ([0.0, 0.0, 0.0, 0.0], -1.0),
        };
        // Snap glyph quads to whole physical pixels: the masks are crisp
        // rasterized bitmaps, and a fractional draw position smears them
        // through linear filtering (thin strokes lose pixels).
        let origin_x = (origin_dp.x * scale).round();
        let origin_y = (origin_dp.y * scale).round();
        return TextInstance {
            origin: [origin_x, origin_y],
            size: [size_dp.width * scale, size_dp.height * scale],
            // Exact 1:1 texel mapping: with the quad aligned to whole
            // pixels, fragment centers land on texel centers (any inset
            // here progressively skews sampling across the glyph).
            uv_origin: [x as f32 / page, y as f32 / page],
            uv_size: [width as f32 / page, height as f32 / page],
            color: crate::rect::linear_rgba(color),
            clip_bounds,
            clip_radius,
            _pad: [0.0; 3],
        };
    }
}

/// WGSL shader: instanced glyph quads sampled from the alpha atlas.
pub const TEXT_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>, // physical pixel size of the surface
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct TextData {
    origin: vec2<f32>,
    size: vec2<f32>,
    uv_origin: vec2<f32>,
    uv_size: vec2<f32>,
    color: vec4<f32>,
    clip_bounds: vec4<f32>,
    clip_radius: f32,
    _pad: array<f32, 3>,
};

@group(1) @binding(0) var<storage, read> glyphs: array<TextData>;
@group(1) @binding(1) var atlas: texture_2d<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) instance_index: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32,
           @builtin(instance_index) instance: u32) -> VertexOutput {
    // Two triangles from the corner table (0,0),(1,0),(0,1),(1,0),(1,1),(0,1).
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let data = glyphs[instance];
    let corner = corners[vertex];
    let pixel = data.origin + corner * data.size;
    let ndc = vec2<f32>(
        pixel.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / camera.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.uv = data.uv_origin + corner * data.uv_size;
    output.instance_index = instance;
    return output;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    let data = glyphs[output.instance_index];
    let alpha = textureSample(atlas, atlas_sampler, output.uv).r;
    return data.color * alpha * clip_mask(output.position.xy, data.clip_bounds, data.clip_radius);
}
"#;

/// Camera uniform: viewport size in physical pixels.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct TextCameraUniform {
    /// Surface size (physical px).
    pub viewport: [f32; 2],
    /// Padding to 16-byte uniform alignment.
    _padding: [f32; 2],
}

/// One GPU-side atlas page: R8 texture plus the frame's bind group.
struct AtlasPageGpu {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// One page's draw range in the shared instance buffer.
struct PageDraw {
    page: usize,
    start: u32,
    count: u32,
}

/// The text pipeline: shader, camera, instance storage, and atlas pages.
pub struct TextPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    glyph_buffer: wgpu::Buffer,
    glyph_capacity: usize,
    /// GPU pages, index-aligned with the CPU atlas page list.
    pages: Vec<AtlasPageGpu>,
    /// Per-page draw ranges of the last upload.
    page_draws: Vec<PageDraw>,
    /// The frame's sampler (shared by all pages).
    sampler: wgpu::Sampler,
}

/// Instance capacity growth step.
const TEXT_CAPACITY_STEP: usize = 4096;
/// Atlas page edge length (must match nui-text's atlas page size).
const ATLAS_PAGE_SIZE: u32 = 1024;

impl TextPipeline {
    /// Creates the pipeline on `device` with the surface's format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> TextPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-text-shader"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}{}", crate::rect::CLIP_WGSL, TEXT_SHADER).into(),
            ),
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-text-camera"),
            size: std::mem::size_of::<TextCameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-text-camera-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("nui-text-instance-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nui-text-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&instance_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-text-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let glyph_capacity = TEXT_CAPACITY_STEP;
        let glyph_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-text-glyphs"),
            size: (glyph_capacity * std::mem::size_of::<TextInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-text-camera-bind"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nui-text-atlas-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        return TextPipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            instance_bind_group_layout,
            glyph_buffer,
            glyph_capacity,
            pages: Vec::new(),
            page_draws: Vec::new(),
            sampler,
        };
    }

    /// Uploads the camera uniform (physical pixel viewport).
    pub fn set_viewport(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let camera = TextCameraUniform {
            viewport: [width, height],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
    }

    /// Syncs GPU atlas pages: creates textures for new pages and uploads
    /// dirtied ones.
    pub fn sync_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut nui_text::GlyphAtlas,
    ) {
        while self.pages.len() < atlas.page_count() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("nui-text-atlas"),
                size: wgpu::Extent3d {
                    width: ATLAS_PAGE_SIZE,
                    height: ATLAS_PAGE_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nui-text-atlas-bind"),
                layout: &self.instance_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.glyph_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.pages.push(AtlasPageGpu {
                texture,
                view,
                bind_group,
            });
        }
        for dirty in atlas.take_dirty_pages() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pages[dirty.index as usize].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &dirty.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dirty.size),
                    rows_per_image: Some(dirty.size),
                },
                wgpu::Extent3d {
                    width: dirty.size,
                    height: dirty.size,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Uploads this frame's glyph instances. `bucketed` maps page index →
    /// instances (in draw order); the flat storage keeps page grouping.
    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bucketed: &[Vec<TextInstance>],
    ) {
        let mut flat: Vec<TextInstance> = Vec::new();
        let mut draws: Vec<PageDraw> = Vec::new();
        for (page, instances) in bucketed.iter().enumerate() {
            if instances.is_empty() {
                continue;
            }
            draws.push(PageDraw {
                page,
                start: flat.len() as u32,
                count: instances.len() as u32,
            });
            flat.extend_from_slice(instances);
        }
        if flat.len() > self.glyph_capacity {
            self.glyph_capacity = flat.len().next_power_of_two();
            self.glyph_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-text-glyphs"),
                size: (self.glyph_capacity * std::mem::size_of::<TextInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // The page bind groups embed the instance buffer; rebuild them
            // after the rare capacity change (past ~4096 visible glyphs).
            for page in &mut self.pages {
                page.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("nui-text-atlas-bind"),
                    layout: &self.instance_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.glyph_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&page.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            }
        }
        queue.write_buffer(&self.glyph_buffer, 0, bytemuck::cast_slice(&flat));
        self.page_draws = draws;
    }

    /// The GPU page count (renderer-side page bucketing).
    pub fn page_count(&self) -> usize {
        return self.pages.len();
    }

    /// Renders the glyph instances into `pass` (one draw per atlas page).
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        for draw in &self.page_draws {
            let Some(page) = self.pages.get(draw.page) else {
                continue;
            };
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &page.bind_group, &[]);
            pass.draw(0..6, draw.start..draw.start + draw.count);
        }
    }
}
