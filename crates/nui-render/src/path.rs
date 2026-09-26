//! Path pipeline: CPU-triangulated fills drawn as plain colored triangles
//! (FUTURE batch 2).
//!
//! The scene builder flattens path data into rings and the earclipper in
//! nui-core produces indices; this pipeline uploads one big vertex+index
//! pair per frame and issues a single indexed draw. v1 is hard-edged —
//! no SDF, no MSAA — matching the plan's "correct first, AA review later"
//! stance. Buffers rebuild per frame (paths are low-frequency UI shapes);
//! capacity growth replaces the buffers, which is safe here because the
//! pipeline holds no bind group pointing at them.

use bytemuck::{Pod, Zeroable};
use nui_core::Color;

use crate::rect::CameraUniform;

/// One fill vertex: position in physical pixels + premultiplied linear
/// color. 24 bytes, both attributes naturally aligned.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct PathVertex {
    /// Position in physical pixels.
    pub position: [f32; 2],
    /// Fill color (linear-space premultiplied rgba).
    pub color: [f32; 4],
}

impl PathVertex {
    /// Builds one vertex from a dp point, dpi scale, and fill color.
    pub fn from_dp(x: f32, y: f32, color: Color, scale: f32) -> PathVertex {
        return PathVertex {
            position: [x * scale, y * scale],
            color: crate::rect::linear_rgba(color),
        };
    }
}

/// WGSL shader: colored triangles straight to the framebuffer,
/// premultiplied alpha out (hard edges; AA is a later review).
pub const PATH_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>, // physical pixel size of the surface
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct PathData {
    position: vec2<f32>,
    color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>,
           @location(1) color: vec4<f32>) -> VertexOutput {
    let ndc = vec2<f32>(
        position.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - position.y / camera.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.color = color;
    return output;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    return output.color;
}
"#;

/// The path fill pipeline: shader, camera uniform, vertex + index buffers.
pub struct PathPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    index_capacity: usize,
    /// Index count of the last upload; `render` draws exactly this many
    /// (unlike the stroke pipeline, the count is not a pure function of
    /// the scene — it depends on the ear clipping result).
    index_count: u32,
}

/// Buffer capacity growth step (vertices and indices grow independently).
const PATH_CAPACITY_STEP: usize = 4096;

impl PathPipeline {
    /// Creates the pipeline on `device` with the surface's format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> PathPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-path-shader"),
            source: wgpu::ShaderSource::Wgsl(PATH_SHADER.into()),
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-camera-layout"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nui-path-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-path-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PathVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                })],
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
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capacity = PATH_CAPACITY_STEP;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-path-vertices"),
            size: (capacity * std::mem::size_of::<PathVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-path-indices"),
            size: (capacity * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-camera-bind"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        return PathPipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            vertex_buffer,
            index_buffer,
            vertex_capacity: capacity,
            index_capacity: capacity,
            index_count: 0,
        };
    }

    /// Uploads the camera uniform (physical pixel viewport).
    pub fn set_viewport(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let camera = CameraUniform {
            viewport: [width, height],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
    }

    /// Uploads this frame's fill geometry, regrowing either buffer when the
    /// frame outgrows the current capacity (the buffers carry no bind
    /// group, so replacement is safe).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[PathVertex],
        indices: &[u32],
    ) {
        self.index_count = indices.len() as u32;
        if vertices.len() > self.vertex_capacity {
            self.vertex_capacity = vertices.len().next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-path-vertices"),
                size: (self.vertex_capacity * std::mem::size_of::<PathVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if indices.len() > self.index_capacity {
            self.index_capacity = indices.len().next_power_of_two();
            self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-path-indices"),
                size: (self.index_capacity * std::mem::size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        }
        if !indices.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(indices));
        }
    }

    /// The index count uploaded for this frame.
    pub fn index_count(&self) -> u32 {
        return self.index_count;
    }

    /// Draws the uploaded fill geometry into `pass`.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn vertex_is_24_bytes_with_color_at_offset_8() {
        assert_eq!(std::mem::size_of::<PathVertex>(), 24);
        let probe = PathVertex::from_dp(1.0, 2.0, Color::from_rgb8(255, 0, 0), 1.0);
        let bytes = bytemuck::bytes_of(&probe);
        assert_eq!(&bytes[8..12], &1.0f32.to_ne_bytes(), "color.r at 8");
        assert_eq!(&bytes[12..16], &0.0f32.to_ne_bytes(), "color.g at 12");
    }

    #[test]
    fn from_dp_scales_position() {
        let probe = PathVertex::from_dp(3.0, 4.0, Color::BLACK, 2.0);
        assert_eq!(probe.position, [6.0, 8.0]);
    }

    #[test]
    fn upload_tracks_index_count_and_grows_capacity() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nui-path-test-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .expect("device");
        let mut pipeline = PathPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        let vertices: Vec<PathVertex> = (0..PATH_CAPACITY_STEP + 1)
            .map(|index| return PathVertex::from_dp(index as f32, 0.0, Color::BLACK, 1.0))
            .collect();
        let indices: Vec<u32> = (0..PATH_CAPACITY_STEP as u32 + 3).collect();
        pipeline.upload(&device, &queue, &vertices, &indices);
        assert_eq!(pipeline.index_count(), PATH_CAPACITY_STEP as u32 + 3);
        assert!(pipeline.vertex_capacity >= vertices.len());
        assert!(pipeline.index_capacity >= indices.len());
    }
}
