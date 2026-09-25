//! Layer pipeline (M9, plan §6.4): offscreen-composited subtrees. A layer
//! renders its subtree into a texture, optionally blurs it (two-pass
//! gaussian ping-pong), and composites it with a group opacity.
//!
//! Content is rendered premultiplied (same blend as the main pass), so a
//! plain alpha scale composites correctly.

use bytemuck::{Pod, Zeroable};

/// Per-instance GPU data for one composited layer quad.
///
/// Layout mirrors WGSL `LayerData`: three `vec4`s — 48 bytes. Every member
/// is a `vec4` so the WGSL stride is unambiguous (a `vec3` padding field
/// once grew the stride to 48 while Rust wrote 32, and every layer after
/// the first read its neighbor's padding).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct LayerInstance {
    /// Top-left corner in physical pixels; `.zw` unused.
    pub origin: [f32; 4],
    /// Quad size in physical pixels in `.xy`; `.z` = group opacity (0..=1);
    /// `.w` unused.
    pub size_opacity: [f32; 4],
    /// Padding to 48 bytes.
    pub _pad: [f32; 4],
}

/// WGSL shader: textured quad scaled by the group opacity.
pub const LAYER_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct LayerData {
    origin: vec4<f32>,
    size_opacity: vec4<f32>,
    _pad: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> layers: array<LayerData>;
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
    let data = layers[instance];
    let corner = corners[vertex];
    let pixel = data.origin.xy + corner * data.size_opacity.xy;
    let ndc = vec2<f32>(
        pixel.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / camera.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.uv = corner;
    output.instance_index = instance;
    return output;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    let data = layers[output.instance_index];
    let sampled = textureSample(tex, tex_sampler, output.uv);
    return sampled * data.size_opacity.z;
}
"#;

/// Camera uniform (physical pixel viewport).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct LayerCameraUniform {
    /// Surface size (physical px).
    pub viewport: [f32; 2],
    /// Padding to 16-byte uniform alignment.
    _padding: [f32; 2],
}

/// The layer compositing pipeline.
pub struct LayerPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    sampler: wgpu::Sampler,
    /// Per-frame bind groups, index-aligned with the frame's layer quads.
    bind_groups: Vec<wgpu::BindGroup>,
}

/// Instance capacity growth step.
const LAYER_CAPACITY_STEP: usize = 16;

impl LayerPipeline {
    /// Creates the pipeline on `device` with the target format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> LayerPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-layer-shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_SHADER.into()),
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-layer-camera"),
            size: std::mem::size_of::<LayerCameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-layer-camera-layout"),
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
                label: Some("nui-layer-instance-layout"),
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
            label: Some("nui-layer-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&instance_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-layer-pipeline"),
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
        let instance_capacity = LAYER_CAPACITY_STEP;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-layer-instances"),
            size: (instance_capacity * std::mem::size_of::<LayerInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-layer-camera-bind"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nui-layer-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        return LayerPipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            instance_bind_group_layout,
            instance_buffer,
            instance_capacity,
            sampler,
            bind_groups: Vec::new(),
        };
    }

    /// Uploads the camera uniform (physical pixel viewport).
    pub fn set_viewport(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let camera = LayerCameraUniform {
            viewport: [width, height],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
    }

    /// Uploads one frame's layer quads and builds per-layer bind groups.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[LayerInstance],
        textures: &[wgpu::TextureView],
    ) {
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().max(LAYER_CAPACITY_STEP).next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-layer-instances"),
                size: (self.instance_capacity * std::mem::size_of::<LayerInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        self.bind_groups = textures
            .iter()
            .map(|view| {
                return device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("nui-layer-texture-bind"),
                    layout: &self.instance_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.instance_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            })
            .collect();
    }

    /// Renders the layer quads into `pass`.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>, count: u32) {
        if count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        for (index, bind_group) in self.bind_groups.iter().enumerate() {
            let instance = index as u32;
            pass.set_bind_group(1, bind_group, &[]);
            pass.draw(0..6, instance..instance + 1);
        }
    }
}

/// Blur pass uniform: texel step (direction * radius factor).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct BlurUniform {
    /// Sample step in texels (direction * spacing).
    pub step: [f32; 2],
    /// Padding to 16-byte uniform alignment.
    _padding: [f32; 2],
}

/// Two-pass gaussian blur (plan §6.4): horizontal then vertical, 9-tap
/// kernel with the classic binomial weights (premultiplied content).
pub const BLUR_SHADER: &str = r#"
struct Blur {
    step: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> blur: Blur;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    var output: VertexOutput;
    let corner = corners[vertex];
    output.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    output.position.y = -output.position.y;
    output.uv = corner;
    return output;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    // 9-tap binomial kernel over premultiplied content.
    var weights = array<f32, 5>(0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162);
    var color = textureSample(tex, tex_sampler, output.uv) * weights[0];
    for (var tap: i32 = 1; tap < 5; tap = tap + 1) {
        let weight = weights[tap];
        let offset = blur.step * f32(tap);
        color = color + textureSample(tex, tex_sampler, output.uv + offset) * weight;
        color = color + textureSample(tex, tex_sampler, output.uv - offset) * weight;
    }
    return color;
}
"#;

/// The blur pipeline: full-quad passes between ping-pong targets.
pub struct BlurPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl BlurPipeline {
    /// Creates the blur pipeline on `device` with the target format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> BlurPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-blur-shader"),
            source: wgpu::ShaderSource::Wgsl(BLUR_SHADER.into()),
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-blur-uniform"),
            size: std::mem::size_of::<BlurUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-blur-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
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
            label: Some("nui-blur-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-blur-pipeline"),
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
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nui-blur-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        return BlurPipeline {
            pipeline,
            uniform_buffer,
            bind_group_layout,
            sampler,
        };
    }

    /// Creates an offscreen texture usable as layer/blur target. The
    /// format must match the pipelines' target format.
    pub fn create_target(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        return device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nui-layer-target"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
    }

    /// Encodes one blur pass from `source` into `target`.
    pub fn encode_pass(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        step: [f32; 2],
    ) {
        let uniform = BlurUniform {
            step,
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-blur-bind"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nui-blur-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}
