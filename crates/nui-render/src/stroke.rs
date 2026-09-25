//! Stroke pipeline: one instanced quad per polyline segment, fragment-
//! shader capsule/box SDF with `fwidth` antialiasing (FUTURE batch 1).
//!
//! The vertex shader orients the quad along the segment (grown past the
//! endpoints so round caps fit; the butt-cap SDF culls the headroom), and
//! the fragment shader evaluates the segment distance from the framebuffer
//! position — no extra varyings beyond the instance index. Instance stride
//! is 64 bytes with only `vec2`/`vec4`/`f32` fields, so there is no hidden
//! WGSL alignment padding (the lesson from the layer pipeline fix).

use bytemuck::{Pod, Zeroable};
use nui_core::{Color, Point, Rect};

use crate::rect::{CLIP_WGSL, CameraUniform, linear_rgba};
use crate::scene::LineCap;

/// Per-instance GPU data for one segment: endpoints in physical pixels,
/// stroke geometry, inherited clip, and color.
///
/// Field layout mirrors the WGSL `StrokeData` struct: the `f32` group runs
/// to offset 32, then the two `vec4`s sit 16-byte aligned, ending at 64.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct StrokeInstance {
    /// Segment start in physical pixels.
    pub a: [f32; 2],
    /// Segment end in physical pixels.
    pub b: [f32; 2],
    /// Half the stroke width (physical px).
    pub half_width: f32,
    /// Cap style: 0 = butt, 1 = round.
    pub cap: f32,
    /// Clip corner radius (physical px); negative = no clipping.
    pub clip_radius: f32,
    /// WGSL alignment padding so `clip_bounds` lands on a 16-byte boundary.
    pub _pad0: f32,
    /// Clip bounds in physical px (x, y, w, h); unused when clip_radius < 0.
    pub clip_bounds: [f32; 4],
    /// Stroke color (linear-space premultiplied rgba).
    pub color: [f32; 4],
}

impl StrokeInstance {
    /// Builds one segment instance from dp endpoints and the dpi scale.
    pub fn from_dp(
        a: Point,
        b: Point,
        width_dp: f32,
        cap: LineCap,
        color: Color,
        scale: f32,
    ) -> StrokeInstance {
        return StrokeInstance {
            a: [a.x * scale, a.y * scale],
            b: [b.x * scale, b.y * scale],
            half_width: (width_dp * 0.5 * scale).max(0.0),
            cap: match cap {
                LineCap::Butt => 0.0,
                LineCap::Round => 1.0,
            },
            clip_radius: -1.0,
            _pad0: 0.0,
            clip_bounds: [0.0, 0.0, 0.0, 0.0],
            color: linear_rgba(color),
        };
    }

    /// Attaches a rounded clip region in dp (shader-side clipping).
    pub fn with_clip(mut self, bounds: Rect, radius_dp: f32, scale: f32) -> StrokeInstance {
        self.clip_bounds = [
            bounds.origin.x * scale,
            bounds.origin.y * scale,
            bounds.size.width * scale,
            bounds.size.height * scale,
        ];
        self.clip_radius = radius_dp * scale;
        return self;
    }
}

/// WGSL shader: segment-oriented instanced quads, capsule (round cap) or
/// oriented-box (butt cap) SDF, premultiplied alpha out.
pub const STROKE_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>, // physical pixel size of the surface
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) instance_index: u32,
};

struct StrokeData {
    a: vec2<f32>,
    b: vec2<f32>,
    half_width: f32,
    cap: f32,          // 0 = butt, 1 = round
    clip_radius: f32,
    _pad: f32,
    clip_bounds: vec4<f32>,
    color: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> strokes: array<StrokeData>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32,
           @builtin(instance_index) instance: u32) -> VertexOutput {
    // Two triangles from the corner table (0,0),(1,0),(0,1),(1,0),(1,1),(0,1).
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let data = strokes[instance];
    let segment = data.b - data.a;
    let seg_len = max(length(segment), 0.0001);
    let dir = segment / seg_len;
    let normal = vec2<f32>(-dir.y, dir.x);
    // Grow the quad past both endpoints by the half width: round caps need
    // the headroom, the butt SDF simply culls it. A degenerate zero-length
    // segment collapses toward a point (arc flattening drops the duplicate
    // closing point, so this only arises from degenerate user input).
    let half_len = seg_len * 0.5 + data.half_width;
    let center = (data.a + data.b) * 0.5;
    let corner = corners[vertex] - vec2<f32>(0.5, 0.5);
    let pixel = center
        + dir * (corner.x * 2.0 * half_len)
        + normal * (corner.y * 2.0 * data.half_width);
    let ndc = vec2<f32>(
        pixel.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / camera.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.instance_index = instance;
    return output;
}

/// Signed distance to the stroked segment at `p`: a stadium (clamped
/// closest point minus half width) for round caps, an oriented box for
/// butt caps.
fn stroke_sdf(p: vec2<f32>, data: StrokeData) -> f32 {
    let segment = data.b - data.a;
    let seg_len2 = max(dot(segment, segment), 0.000001);
    if (data.cap > 0.5) {
        let t = clamp(dot(p - data.a, segment) / seg_len2, 0.0, 1.0);
        return distance(p, data.a + segment * t) - data.half_width;
    }
    let seg_len = sqrt(seg_len2);
    let dir = segment / seg_len;
    let normal = vec2<f32>(-dir.y, dir.x);
    let q = p - (data.a + data.b) * 0.5;
    let ox = abs(dot(q, dir)) - seg_len * 0.5;
    let oy = abs(dot(q, normal)) - data.half_width;
    let outside = length(max(vec2<f32>(ox, oy), vec2<f32>(0.0)));
    let inside = min(max(ox, oy), 0.0);
    return outside + inside;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    let data = strokes[output.instance_index];
    let distance = stroke_sdf(output.position.xy, data);
    // fwidth-based AA: one pixel of soft edge.
    let aa = fwidth(distance);
    let alpha = 1.0 - smoothstep(-aa, aa, distance);
    return data.color * alpha
        * clip_mask(output.position.xy, data.clip_bounds, data.clip_radius);
}
"#;

/// The stroke pipeline: shader, bind groups, and instance buffer.
pub struct StrokePipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    stroke_layout: wgpu::BindGroupLayout,
    stroke_buffer: wgpu::Buffer,
    stroke_bind_group: wgpu::BindGroup,
    stroke_capacity: usize,
}

/// Instance capacity growth step.
const STROKE_CAPACITY_STEP: usize = 1024;

impl StrokePipeline {
    /// Creates the pipeline on `device` with the surface's format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> StrokePipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-stroke-shader"),
            source: wgpu::ShaderSource::Wgsl(format!("{}{}", CLIP_WGSL, STROKE_SHADER).into()),
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
        let stroke_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-stroke-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nui-stroke-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&stroke_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-stroke-pipeline"),
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
        let capacity = STROKE_CAPACITY_STEP;
        let stroke_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-stroke-instances"),
            size: (capacity * std::mem::size_of::<StrokeInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
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
        let stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-stroke-bind"),
            layout: &stroke_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: stroke_buffer.as_entire_binding(),
            }],
        });
        return StrokePipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            stroke_layout,
            stroke_buffer,
            stroke_bind_group,
            stroke_capacity: capacity,
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

    /// Uploads the instance list, regrowing the storage buffer (and its
    /// bind group, which references the buffer) when needed.
    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[StrokeInstance],
    ) {
        if instances.len() > self.stroke_capacity {
            self.stroke_capacity = instances.len().next_power_of_two();
            self.stroke_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-stroke-instances"),
                size: (self.stroke_capacity * std::mem::size_of::<StrokeInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // The old bind group still points at the replaced buffer; rebuild
            // it so rendering reads the new storage.
            self.stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nui-stroke-bind"),
                layout: &self.stroke_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.stroke_buffer.as_entire_binding(),
                }],
            });
        }
        let bytes = bytemuck::cast_slice(instances);
        queue.write_buffer(&self.stroke_buffer, 0, bytes);
    }

    /// Renders `instance_count` segment quads into `pass`.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, instance_count: u32) {
        if instance_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.stroke_bind_group, &[]);
        pass.draw(0..6, 0..instance_count);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn instance_is_64_bytes_with_vec4_aligned_color() {
        // a(8) + b(8) + half_width(4) + cap(4) + clip_radius(4) + pad(4)
        // + clip_bounds(16) + color(16) = 64 bytes; `color` must sit at the
        // WGSL vec4 alignment (offset 48).
        assert_eq!(std::mem::size_of::<StrokeInstance>(), 64);
        let probe = StrokeInstance::from_dp(
            Point::new(1.0, 2.0),
            Point::new(3.0, 4.0),
            2.0,
            LineCap::Round,
            Color::from_rgb8(255, 0, 0),
            1.0,
        );
        let bytes = bytemuck::bytes_of(&probe);
        assert_eq!(&bytes[48..52], &1.0f32.to_ne_bytes(), "color.r at 48");
        assert_eq!(&bytes[52..56], &0.0f32.to_ne_bytes(), "color.g at 52");
    }

    #[test]
    fn from_dp_scales_geometry_and_encodes_caps() {
        let butt = StrokeInstance::from_dp(
            Point::ZERO,
            Point::new(10.0, 0.0),
            4.0,
            LineCap::Butt,
            Color::BLACK,
            2.0,
        );
        assert_eq!(butt.a, [0.0, 0.0]);
        assert_eq!(butt.b, [20.0, 0.0]);
        assert_eq!(butt.half_width, 4.0, "half the width, dpi-scaled");
        assert_eq!(butt.cap, 0.0);
        assert_eq!(butt.clip_radius, -1.0, "unclipped by default");
        let round = StrokeInstance::from_dp(
            Point::ZERO,
            Point::ZERO,
            4.0,
            LineCap::Round,
            Color::BLACK,
            1.0,
        );
        assert_eq!(round.cap, 1.0);
    }

    #[test]
    fn clip_attaches_bounds_and_radius() {
        let mut instance = StrokeInstance::from_dp(
            Point::ZERO,
            Point::new(10.0, 0.0),
            2.0,
            LineCap::Butt,
            Color::BLACK,
            1.0,
        );
        instance = instance.with_clip(
            Rect::new(Point::new(1.0, 2.0), nui_core::Size::new(30.0, 40.0)),
            6.0,
            2.0,
        );
        assert_eq!(instance.clip_bounds, [2.0, 4.0, 60.0, 80.0]);
        assert_eq!(instance.clip_radius, 12.0);
    }
}
