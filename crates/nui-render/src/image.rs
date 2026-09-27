//! Image pipeline (M8, plan §6.3): decoded-RGBA textures keyed by
//! "path:content-hash", instanced textured quads with tint and nine-slice
//! support. Decoding itself is CPU work done off-thread by the host; the
//! pipeline only uploads and samples.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use nui_core::{Color, Point, Rect, Size};

/// A decoded image, straight (non-premultiplied) RGBA, row-major.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA bytes, `width * height * 4` entries.
    pub rgba: Vec<u8>,
}

/// Image decode/load error (typed, plan §9).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageLoadError {
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ImageLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return write!(formatter, "{}", self.message);
    }
}

impl std::error::Error for ImageLoadError {}

/// Synchronous decode of one image file (run on a worker thread; the host
/// owns the async dispatch, plan §5 图片解码等后台线程).
pub fn decode_file(path: &str) -> Result<DecodedImage, ImageLoadError> {
    let reader = image::ImageReader::open(path).map_err(|error| {
        return ImageLoadError {
            message: format!("cannot open `{path}`: {error}"),
        };
    })?;
    let decoded = reader.decode().map_err(|error| {
        return ImageLoadError {
            message: format!("cannot decode `{path}`: {error}"),
        };
    })?;
    let rgba = decoded.to_rgba8();
    return Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    });
}

/// The texture cache key: path plus content hash, so a changed file with
/// the same path invalidates (plan §6.3 「路径+内容哈希」).
pub fn cache_key(path: &str, content_hash: u64) -> String {
    return format!("{path}:{content_hash:016x}");
}

/// FNV-1a content hash (stable across runs; no crypto needs here).
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    return hash;
}

/// A sub-rectangle of a texture, in **texture pixels**.
///
/// The reason this exists: a generated icon set. One atlas holding every
/// glyph is one decode and one texture upload instead of forty, and each
/// glyph is addressed by its own cell. Pixels rather than fractions,
/// because the atlas is generated at a known size and a generated
/// name-to-cell table naturally carries pixel numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRegion {
    /// Left edge, in texture pixels.
    pub left: f32,
    /// Top edge, in texture pixels.
    pub top: f32,
    /// Width, in texture pixels.
    pub width: f32,
    /// Height, in texture pixels.
    pub height: f32,
}

impl ImageRegion {
    /// Builds a region from its top-left corner and size.
    pub fn new(left: f32, top: f32, width: f32, height: f32) -> ImageRegion {
        return ImageRegion {
            left,
            top,
            width,
            height,
        };
    }

    /// The region's pixel size, which is what a nine-slice inset is
    /// measured against in its place of the whole texture.
    pub fn size(&self) -> (f32, f32) {
        return (self.width, self.height);
    }
}

/// One drawable image region (already expanded to quads by the renderer's
/// nine-slice split; the scene carries the logical draw).
#[derive(Debug, Clone)]
pub struct ImageDraw {
    /// Destination rect in dp.
    pub geometry: Rect,
    /// Tint multiplied over the sampled color (white = untouched).
    pub tint: Color,
    /// Texture cache key.
    pub key: String,
    /// Nine-slice inset on all sides (dp); 0 = stretch the whole texture.
    pub slice: f32,
    /// The part of the texture to draw; `None` is the whole thing, which is
    /// what every image did before regions existed.
    pub region: Option<ImageRegion>,
    /// Inherited clip region, if any.
    pub clip: Option<crate::scene::ClipDraw>,
}

/// Per-instance GPU data: destination quad, texture UVs, tint.
///
/// Layout mirrors WGSL `ImageData`: three `vec2`s then a `vec4` at offset
/// 32 — 48 bytes total (same shape as the text pipeline's instances).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct ImageInstance {
    /// Top-left corner in physical pixels.
    pub origin: [f32; 2],
    /// Quad size in physical pixels.
    pub size: [f32; 2],
    /// Texture UV top-left.
    pub uv_origin: [f32; 2],
    /// Texture UV extent.
    pub uv_size: [f32; 2],
    /// Tint color (premultiplied rgba).
    pub tint: [f32; 4],
    /// Clip bounds in physical px; unused when clip_radius < 0.
    pub clip_bounds: [f32; 4],
    /// Clip corner radius; negative = no clipping.
    pub clip_radius: f32,
    /// Padding to 80 bytes.
    pub _pad: [f32; 3],
}

/// WGSL shader: instanced textured quads, straight-alpha texture sampled
/// and premultiplied on output (the surface blend is premultiplied).
pub const IMAGE_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>, // physical pixel size of the surface
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct ImageData {
    origin: vec2<f32>,
    size: vec2<f32>,
    uv_origin: vec2<f32>,
    uv_size: vec2<f32>,
    tint: vec4<f32>,
    clip_bounds: vec4<f32>,
    clip_radius: f32,
    _pad: array<f32, 3>,
};

@group(1) @binding(0) var<storage, read> images: array<ImageData>;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var tex_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) instance_index: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32,
           @builtin(instance_index) instance: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let data = images[instance];
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
    let data = images[output.instance_index];
    let sampled = textureSample(tex, tex_sampler, output.uv);
    let color = vec4<f32>(data.tint.rgb * sampled.rgb, data.tint.a) * sampled.a;
    // Straight-alpha texture -> premultiplied output.
    return color * clip_mask(output.position.xy, data.clip_bounds, data.clip_radius);
}
"#;

/// Camera uniform (physical pixel viewport).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct ImageCameraUniform {
    /// Surface size (physical px).
    pub viewport: [f32; 2],
    /// Padding to 16-byte uniform alignment.
    _padding: [f32; 2],
}

/// One GPU texture with its bind group (view retained for rebuilds).
struct GpuImage {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

/// The image pipeline: texture cache + instanced quads.
pub struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    sampler: wgpu::Sampler,
    /// Uploaded textures by cache key.
    textures: HashMap<String, GpuImage>,
    /// This frame's draws, grouped by texture key (draw order preserved).
    frame: Vec<(String, Vec<ImageInstance>)>,
}

/// Instance capacity growth step.
const IMAGE_CAPACITY_STEP: usize = 64;

impl ImagePipeline {
    /// Creates the pipeline on `device` with the surface's format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> ImagePipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-image-shader"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}{}", crate::rect::CLIP_WGSL, IMAGE_SHADER).into(),
            ),
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-image-camera"),
            size: std::mem::size_of::<ImageCameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-image-camera-layout"),
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
                label: Some("nui-image-instance-layout"),
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
            label: Some("nui-image-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&instance_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-image-pipeline"),
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
        let instance_capacity = IMAGE_CAPACITY_STEP;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-image-instances"),
            size: (instance_capacity * std::mem::size_of::<ImageInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-image-camera-bind"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nui-image-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        return ImagePipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            instance_bind_group_layout,
            instance_buffer,
            instance_capacity,
            sampler,
            textures: HashMap::new(),
            frame: Vec::new(),
        };
    }

    /// Whether `key` has a GPU texture yet.
    pub fn has_texture(&self, key: &str) -> bool {
        return self.textures.contains_key(key);
    }

    /// Uploads a decoded image as `key` (host calls this when a decode job
    /// lands; re-uploads replace the texture, e.g. changed file content).
    pub fn upload_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        decoded: &DecodedImage,
    ) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nui-image-texture"),
            size: wgpu::Extent3d {
                width: decoded.width.max(1),
                height: decoded.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB texture: decoded bytes are sRGB-encoded; sampling
            // linearizes and the sRGB surface converts back.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &decoded.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(decoded.width * 4),
                rows_per_image: Some(decoded.height),
            },
            wgpu::Extent3d {
                width: decoded.width.max(1),
                height: decoded.height.max(1),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-image-texture-bind"),
            layout: &self.instance_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.instance_buffer.as_entire_binding(),
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
        self.textures.insert(
            key.to_string(),
            GpuImage {
                view,
                bind_group,
                width: decoded.width,
                height: decoded.height,
            },
        );
    }

    /// Uploads the camera uniform (physical pixel viewport).
    pub fn set_viewport(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let camera = ImageCameraUniform {
            viewport: [width, height],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
    }

    /// Expands the frame's image draws into instances (nine-slice aware)
    /// and uploads them. Draws without a GPU texture are skipped this
    /// frame (decode still in flight).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scale: f32,
        draws: &[ImageDraw],
    ) {
        // Group by texture key, preserving paint order.
        let mut grouped: Vec<(String, Vec<ImageInstance>)> = Vec::new();
        let mut index_of: HashMap<String, usize> = HashMap::new();
        for draw in draws {
            let Some(texture) = self.textures.get(&draw.key) else {
                continue;
            };
            // A region replaces the texture's own extent as the space the
            // nine-slice split works in, so `slice` stays in dp and is
            // measured against the region rather than the whole atlas.
            let (split_width, split_height) = match &draw.region {
                Some(region) => {
                    let (width, height) = region.size();
                    // `nine_slice_quads` clamps its inset against the pixel
                    // extent, so a fractional region is rounded here rather
                    // than producing a sub-pixel split it cannot express.
                    (width.round() as u32, height.round() as u32)
                }
                None => (texture.width, texture.height),
            };
            let quads =
                nine_slice_quads(draw.geometry, draw.slice, split_width, split_height, scale);
            let index = match index_of.get(&draw.key) {
                Some(index) => *index,
                None => {
                    grouped.push((draw.key.clone(), Vec::new()));
                    let index = grouped.len() - 1;
                    index_of.insert(draw.key.clone(), index);
                    index
                }
            };
            for (dest, uv) in quads {
                let (uv_origin, uv_size) = remap_uv(
                    draw.region,
                    (texture.width as f32, texture.height as f32),
                    uv[0],
                    uv[1],
                );
                grouped[index].1.push(
                    ImageInstance::from_dp(dest.origin, dest.size, draw.tint, scale, draw.clip)
                        .with_uv(uv_origin, uv_size),
                );
            }
        }
        let total: usize = grouped
            .iter()
            .map(|(_, instances)| return instances.len())
            .sum();
        if total > self.instance_capacity {
            self.instance_capacity = total.max(IMAGE_CAPACITY_STEP).next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-image-instances"),
                size: (self.instance_capacity * std::mem::size_of::<ImageInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // Bind groups embed the instance buffer; rebuild all.
            for gpu in self.textures.values_mut() {
                gpu.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("nui-image-texture-bind"),
                    layout: &self.instance_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&gpu.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            }
        }
        // One flat upload; per-texture draws slice the range.
        let flat: Vec<ImageInstance> = grouped
            .iter()
            .flat_map(|(_, instances)| return instances.iter().copied())
            .collect();
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&flat));
        self.frame = grouped;
    }

    /// Renders this frame's images (one draw per texture, in upload order).
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        let mut start = 0u32;
        for (key, instances) in &self.frame {
            let Some(texture) = self.textures.get(key) else {
                continue;
            };
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &texture.bind_group, &[]);
            let count = instances.len() as u32;
            pass.draw(0..6, start..start + count);
            start += count;
        }
    }
}

impl ImageInstance {
    /// Builds an instance from a dp-space quad plus the atlas texel rect,
    /// applying the dpi scale, tint, and clip region.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dp(
        origin_dp: Point,
        size_dp: Size,
        tint: Color,
        scale: f32,
        clip: Option<crate::scene::ClipDraw>,
    ) -> ImageInstance {
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
        return ImageInstance {
            origin: [origin_dp.x * scale, origin_dp.y * scale],
            size: [size_dp.width * scale, size_dp.height * scale],
            uv_origin: [0.0, 0.0],
            uv_size: [1.0, 1.0],
            tint: crate::rect::linear_rgba(tint),
            clip_bounds,
            clip_radius,
            _pad: [0.0; 3],
        };
    }

    /// Sets the texture UV rect (normalized).
    pub fn with_uv(mut self, uv_origin: [f32; 2], uv_size: [f32; 2]) -> ImageInstance {
        self.uv_origin = uv_origin;
        self.uv_size = uv_size;
        return self;
    }
}

/// Splits `rect` into up to nine destination/UV quad pairs for a
/// Maps UVs expressed in a region's own `0..1` space into texture space.
///
/// `None` is the identity — the draw already covers the whole texture, which
/// is every image written before regions existed. With a region, the `0..1`
/// span of the region is scaled by its share of the texture and offset to
/// where the region starts.
///
/// Pure, so the arithmetic is checkable without a GPU: the offscreen tests
/// confirm the *result* on screen, and this is where the off-by-one lives.
pub fn remap_uv(
    region: Option<ImageRegion>,
    texture_size: (f32, f32),
    uv_origin: [f32; 2],
    uv_size: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let Some(region) = region else {
        return (uv_origin, uv_size);
    };
    let (texture_width, texture_height) = texture_size;
    // A zero-sized texture cannot be divided by; the draw is degenerate
    // anyway, so leave the UVs where they were rather than emit NaNs.
    if texture_width <= 0.0 || texture_height <= 0.0 {
        return (uv_origin, uv_size);
    }
    let scale_x = region.width / texture_width;
    let scale_y = region.height / texture_height;
    let origin = [
        region.left / texture_width + uv_origin[0] * scale_x,
        region.top / texture_height + uv_origin[1] * scale_y,
    ];
    return (origin, [uv_size[0] * scale_x, uv_size[1] * scale_y]);
}

/// `slice`-dp nine-patch (`slice <= 0` yields one stretched quad).
/// Returns `((dest_rect, [uv_origin, uv_size]), ...)`.
pub fn nine_slice_quads(
    rect: Rect,
    slice_dp: f32,
    texture_width: u32,
    texture_height: u32,
    scale: f32,
) -> Vec<(Rect, [[f32; 2]; 2])> {
    if slice_dp <= 0.0 || texture_width == 0 || texture_height == 0 {
        return vec![(rect, [[0.0, 0.0], [1.0, 1.0]])];
    }
    let slice_px = slice_dp * scale;
    let texture_w = texture_width as f32;
    let texture_h = texture_height as f32;
    let left = slice_px.min(texture_w / 2.0);
    let right = left;
    let top = slice_px.min(texture_h / 2.0);
    let bottom = top;
    let dest = |x: f32, y: f32, w: f32, h: f32| -> Rect {
        return Rect::new(
            Point::new(
                rect.origin.x + x * rect.size.width,
                rect.origin.y + y * rect.size.height,
            ),
            Size::new(w * rect.size.width, h * rect.size.height),
        );
    };
    let uv = |x0: f32, y0: f32, x1: f32, y1: f32| -> [[f32; 2]; 2] {
        return [
            [x0 / texture_w, y0 / texture_h],
            [(x1 - x0) / texture_w, (y1 - y0) / texture_h],
        ];
    };
    let column = |x: f32| -> f32 {
        return x / rect.size.width;
    };
    let row = |y: f32| -> f32 {
        return y / rect.size.height;
    };
    let lx = column(left / scale);
    let rx = column((rect.size.width - right / scale).max(lx));
    let ty = row(top / scale);
    let by = row((rect.size.height - bottom / scale).max(ty));
    let mut quads = Vec::with_capacity(9);
    // Rows top / middle / bottom × columns left / middle / right; UVs are
    // in texture pixels so corners keep their exact size regardless of the
    // destination stretch.
    let uv_left = left;
    let uv_right = texture_w - right;
    let uv_top = top;
    let uv_bottom = texture_h - bottom;
    let spans_x: [(f32, f32, f32, f32); 3] = [
        (0.0, lx, 0.0, uv_left),
        (lx, rx, uv_left, uv_right),
        (rx, 1.0, uv_right, texture_w),
    ];
    let spans_y: [(f32, f32, f32, f32); 3] = [
        (0.0, ty, 0.0, uv_top),
        (ty, by, uv_top, uv_bottom),
        (by, 1.0, uv_bottom, texture_h),
    ];
    for (y0, y1, v0, v1) in spans_y {
        for (x0, x1, u0, u1) in spans_x {
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            quads.push((dest(x0, y0, x1 - x0, y1 - y0), uv(u0, v0, u1, v1)));
        }
    }
    return quads;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn no_slice_yields_one_full_quad() {
        let rect = Rect::new(Point::ZERO, Size::new(50.0, 50.0));
        let quads = nine_slice_quads(rect, 0.0, 32, 32, 1.0);
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].1, [[0.0, 0.0], [1.0, 1.0]]);
    }

    #[test]
    fn slice_splits_into_nine_with_pixel_uv_corners() {
        let rect = Rect::new(Point::ZERO, Size::new(90.0, 90.0));
        let quads = nine_slice_quads(rect, 10.0, 30, 30, 1.0);
        assert_eq!(quads.len(), 9, "3x3 patch");
        // Corner UVs keep exact texel sizes: top-left corner is 10x10 px of
        // a 30x30 texture.
        let (dest, uv) = quads[0];
        assert!((dest.size.width - 10.0).abs() < 1e-4);
        assert!((uv[0][0] - 0.0).abs() < 1e-6);
        assert!((uv[1][0] - 10.0 / 30.0).abs() < 1e-4, "corner uv width");
        // Middle piece stretches over the remaining destination width.
        let (middle, uv_middle) = quads[4];
        assert!((middle.size.width - 70.0).abs() < 1e-4, "90 - 2*10");
        assert!((uv_middle[1][0] - 10.0 / 30.0).abs() < 1e-4);
    }

    #[test]
    fn no_region_leaves_the_uvs_alone() {
        // Every image written before regions existed takes this path, and
        // the whole texture is `0..1` on both axes.
        let (origin, size) = remap_uv(None, (64.0, 32.0), [0.0, 0.0], [1.0, 1.0]);
        assert_eq!(origin, [0.0, 0.0]);
        assert_eq!(size, [1.0, 1.0]);
    }

    #[test]
    fn a_region_scales_and_offsets_the_uvs() {
        // A 16x16 cell at (32, 48) of a 128x64 atlas: the region's share of
        // the texture is 1/8 across and 1/4 down, starting a quarter and
        // three quarters of the way in.
        let region = ImageRegion::new(32.0, 48.0, 16.0, 16.0);
        let (origin, size) = remap_uv(Some(region), (128.0, 64.0), [0.0, 0.0], [1.0, 1.0]);
        assert!((origin[0] - 0.25).abs() < 1e-6, "u starts at 0.25");
        assert!((origin[1] - 0.75).abs() < 1e-6, "v starts at 0.75");
        assert!((size[0] - 0.125).abs() < 1e-6, "u spans an eighth");
        assert!((size[1] - 0.25).abs() < 1e-6, "v spans a quarter");
        // The far corner lands exactly on the region's bottom-right, which
        // is the off-by-one this function exists to get right.
        let end = [origin[0] + size[0], origin[1] + size[1]];
        assert!((end[0] - 0.375).abs() < 1e-6, "u ends at 0.375");
        assert!((end[1] - 1.0).abs() < 1e-6, "v ends at 1.0");
    }

    #[test]
    fn a_nine_sliced_region_maps_each_band_into_the_cell() {
        // A nine-slice inside a region: the split happens in the region's
        // own space, then every band's UVs are remapped. A 4px slice of a
        // 16px cell is a quarter of the cell, not a quarter of the atlas.
        let region = ImageRegion::new(32.0, 0.0, 16.0, 16.0);
        let quads = nine_slice_quads(
            Rect::new(Point::ZERO, Size::new(160.0, 16.0)),
            4.0,
            16,
            16,
            1.0,
        );
        assert_eq!(quads.len(), 9, "the split happens in region space");
        for (_, uv) in &quads {
            let (origin, size) = remap_uv(Some(region), (128.0, 16.0), uv[0], uv[1]);
            assert!(
                origin[0] >= 0.25 - 1e-6 && origin[0] + size[0] <= 0.375 + 1e-6,
                "every band stays inside the cell's u range, got {origin:?} {size:?}"
            );
        }
    }

    #[test]
    fn a_degenerate_texture_leaves_the_uvs_alone() {
        // Division by a zero-sized texture would be NaN, and a NaN UV makes
        // the whole draw vanish rather than fail visibly.
        let region = ImageRegion::new(0.0, 0.0, 8.0, 8.0);
        let (origin, size) = remap_uv(Some(region), (0.0, 0.0), [0.0, 0.0], [1.0, 1.0]);
        assert_eq!(origin, [0.0, 0.0]);
        assert_eq!(size, [1.0, 1.0]);
    }

    #[test]
    fn oversized_slice_clamps_to_half_texture() {
        let rect = Rect::new(Point::ZERO, Size::new(90.0, 90.0));
        let quads = nine_slice_quads(rect, 100.0, 30, 30, 1.0);
        assert_eq!(quads.len(), 9, "clamped slice still splits");
        let (corner, _) = quads[0];
        assert!(
            (corner.size.width - 15.0).abs() < 1e-4,
            "clamped to texture half"
        );
    }
}
