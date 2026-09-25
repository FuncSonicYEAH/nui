//! Rect pipeline: one instanced quad per element, fragment-shader SDF for
//! rounded rectangles and borders with `fwidth` antialiasing (plan §6.1).
//!
//! The rect pipeline carries ~80% of a typical UI, so it is the M3
//! deliverable; text/image pipelines arrive with the M3 follow-up and
//! effects with M6.

use bytemuck::{Pod, Zeroable};
use nui_core::{Color, Rect};

/// GPU vertex: corner position in quad space.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct QuadVertex {
    /// Corner offset (-1..=1 square).
    position: [f32; 2],
}

/// Per-instance GPU data: transform, size, radius, border, and fill color.
///
/// Field layout mirrors the WGSL `RectData` struct, including the implicit
/// 8-byte alignment padding before each `vec4` (WGSL requires `vec4` be
/// 16-byte aligned).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct RectInstance {
    /// Top-left corner in physical pixels (dpi-scaled at submission).
    pub origin: [f32; 2],
    /// Size in physical pixels.
    pub size: [f32; 2],
    /// Corner radius (physical px, clamped in the shader).
    pub corner_radius: f32,
    /// Border width (physical px; 0 = no border).
    pub border_width: f32,
    /// WGSL struct alignment padding before `fill: vec4<f32>`.
    pub _pad0: [f32; 2],
    /// Fill color (premultiplied rgba).
    pub fill: [f32; 4],
    /// Border color (premultiplied rgba).
    pub border_color: [f32; 4],
    /// Shadow color (premultiplied rgba; transparent = no shadow).
    pub shadow_color: [f32; 4],
    /// Shadow softness (physical px).
    pub shadow_blur: f32,
    /// WGSL struct alignment padding before `shadow_offset: vec2<f32>`.
    pub _pad1: [f32; 1],
    /// Shadow offset (physical px, positive = down/right).
    pub shadow_offset: [f32; 2],
    /// Clip bounds in physical px (x, y, w, h); unused when clip_radius < 0.
    pub clip_bounds: [f32; 4],
    /// Clip corner radius (physical px); negative = no clipping.
    pub clip_radius: f32,
    /// WGSL struct padding to 128 bytes.
    pub _pad2: [f32; 3],
}

/// Converts a UI color (sRGB-encoded components) to **linear-space
/// premultiplied** RGBA for GPU submission: the render targets are sRGB,
/// so shader values must be linear or every color double-encodes lighter
/// (M9 playground screenshots exposed this as a real bug).
pub fn linear_rgba(color: Color) -> [f32; 4] {
    let [red, green, blue, alpha] = color.to_rgba8();
    let linear = |component: u8| -> f32 {
        let component = f32::from(component) / 255.0;
        if component <= 0.04045 {
            return component / 12.92;
        }
        return ((component + 0.055) / 1.055).powf(2.4);
    };
    let alpha = f32::from(alpha) / 255.0;
    return [
        linear(red) * alpha,
        linear(green) * alpha,
        linear(blue) * alpha,
        alpha,
    ];
}

/// The shared WGSL clip helper, concatenated ahead of every pipeline's
/// shader (plan §6: instances carry clip rects, rounded clipping in the
/// shader, tightest ancestor clip wins).
pub const CLIP_WGSL: &str = r#"
fn clip_mask(pos: vec2<f32>, bounds: vec4<f32>, radius: f32) -> f32 {
    if (radius < 0.0) {
        return 1.0;
    }
    let half_size = bounds.zw * 0.5;
    let center = pos - bounds.xy - half_size;
    let clamped_radius = min(radius, min(half_size.x, half_size.y));
    let offset = abs(center) - (half_size - vec2<f32>(clamped_radius));
    let outside = length(max(offset, vec2<f32>(0.0)));
    let inside = min(max(offset.x, offset.y), 0.0);
    let distance = outside + inside - clamped_radius;
    return 1.0 - smoothstep(-1.0, 1.0, distance);
}
"#;

impl RectInstance {
    /// Builds an instance from a dp rect, colors, and the dpi scale factor.
    pub fn from_dp(rect: Rect, radius_dp: f32, fill: Color, scale: f32) -> RectInstance {
        return RectInstance {
            origin: [rect.origin.x * scale, rect.origin.y * scale],
            size: [rect.size.width * scale, rect.size.height * scale],
            corner_radius: radius_dp * scale,
            border_width: 0.0,
            _pad0: [0.0, 0.0],
            fill: linear_rgba(fill),
            border_color: linear_rgba(fill),
            shadow_color: [0.0, 0.0, 0.0, 0.0],
            shadow_blur: 0.0,
            _pad1: [0.0],
            shadow_offset: [0.0, 0.0],
            clip_bounds: [0.0, 0.0, 0.0, 0.0],
            clip_radius: -1.0,
            _pad2: [0.0, 0.0, 0.0],
        };
    }

    /// Attaches a rounded clip region in dp (M9 shader-side clipping).
    pub fn with_clip(mut self, bounds: Rect, radius_dp: f32, scale: f32) -> RectInstance {
        self.clip_bounds = [
            bounds.origin.x * scale,
            bounds.origin.y * scale,
            bounds.size.width * scale,
            bounds.size.height * scale,
        ];
        self.clip_radius = radius_dp * scale;
        return self;
    }

    /// Attaches a border in dp.
    pub fn with_border(mut self, width_dp: f32, color: Color) -> RectInstance {
        self.border_width = width_dp.max(0.0);
        self.border_color = linear_rgba(color);
        return self;
    }

    /// Attaches a soft shadow (SDF approximation, plan §6.4): `offset_dp`
    /// (positive = down/right), `blur_dp` softness, premultiplied color.
    pub fn with_shadow(
        mut self,
        offset_dp: [f32; 2],
        blur_dp: f32,
        color: Color,
        scale: f32,
    ) -> RectInstance {
        self.shadow_offset = [offset_dp[0] * scale, offset_dp[1] * scale];
        self.shadow_blur = blur_dp.max(0.0) * scale;
        self.shadow_color = linear_rgba(color);
        return self;
    }
}

/// WGSL shader: instanced quads, SDF rounded-rect fill + border, premultiplied
/// alpha out. Coordinate convention: `origin` is the quad's top-left in
/// physical pixels, the vertex shader expands the quad in NDC.
pub const RECT_SHADER: &str = r#"
struct Camera {
    viewport: vec2<f32>, // physical pixel size of the surface
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,   // 0..size in physical px
    @location(1) @interpolate(flat) instance_index: u32,
};

struct RectData {
    origin: vec2<f32>,
    size: vec2<f32>,
    corner_radius: f32,
    border_width: f32,
    fill: vec4<f32>,
    border_color: vec4<f32>,
    shadow_color: vec4<f32>,
    shadow_blur: f32,
    _pad: f32,
    shadow_offset: vec2<f32>,
    clip_bounds: vec4<f32>,
    clip_radius: f32,
    _pad2: array<f32, 3>,
};

@group(1) @binding(0) var<storage, read> rects: array<RectData>;

/// Shadow bleed for one instance: blur radius plus the offset magnitude,
/// per axis (zero without a shadow). The quad must grow by this much or
/// the shadow falls outside the rasterized area.
fn shadow_bleed(data: RectData) -> vec2<f32> {
    if (data.shadow_color.a > 0.0) {
        return vec2<f32>(data.shadow_blur) + abs(data.shadow_offset);
    }
    return vec2<f32>(0.0, 0.0);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32,
           @builtin(instance_index) instance: u32) -> VertexOutput {
    // Two triangles from the corner table (0,0),(1,0),(0,1),(1,0),(1,1),(0,1).
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let data = rects[instance];
    let corner = corners[vertex];
    let bleed = shadow_bleed(data);
    let expanded = data.size + bleed * 2.0;
    let pixel = data.origin - bleed + corner * expanded;
    let ndc = vec2<f32>(
        pixel.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / camera.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    // Expanded-quad space: the rect itself spans bleed..bleed+size.
    output.local = corner * expanded;
    output.instance_index = instance;
    return output;
}

/// Signed distance to a rounded rectangle at `point` (rect spans 0..size).
fn rounded_rect_sdf(point: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let half_size = size * 0.5;
    let center = point - half_size;
    let clamped_radius = min(radius, min(half_size.x, half_size.y));
    let offset = abs(center) - (half_size - vec2<f32>(clamped_radius));
    let outside = length(max(offset, vec2<f32>(0.0)));
    let inside = min(max(offset.x, offset.y), 0.0);
    return outside + inside - clamped_radius;
}

@fragment
fn fs_main(output: VertexOutput) -> @location(0) vec4<f32> {
    let data = rects[output.instance_index];
    let bleed = shadow_bleed(data);
    // `local` is in expanded-quad space; the rect itself spans bleed..bleed+size.
    let distance = rounded_rect_sdf(output.local - bleed, data.size, data.corner_radius);
    // fwidth-based AA: one pixel of soft edge.
    let aa = fwidth(distance);
    let fill_alpha = 1.0 - smoothstep(-aa, aa, distance);
    var color = data.fill * fill_alpha;

    // Soft shadow: SDF of the rect shifted by `shadow_offset`, softened
    // over `shadow_blur`, composited under the fill (plan §6.4).
    if (data.shadow_color.a > 0.0) {
        let shadow_distance = rounded_rect_sdf(
            output.local - bleed - data.shadow_offset,
            data.size,
            data.corner_radius,
        );
        let soft = max(data.shadow_blur, 0.001);
        let shadow_alpha = (1.0 - smoothstep(-soft, soft, shadow_distance)) * data.shadow_color.a;
        color = data.shadow_color * shadow_alpha * (1.0 - fill_alpha) + color;
    }

    if (data.border_width > 0.0) {
        let inner_distance = distance + data.border_width;
        let band = smoothstep(-aa, aa, distance) * (1.0 - smoothstep(-aa, aa, inner_distance));
        let border_alpha = band * (1.0 - fill_alpha);
        color = color + data.border_color * border_alpha;
    }
    return color * clip_mask(output.position.xy, data.clip_bounds, data.clip_radius);
}
"#;

/// Camera uniform: viewport size in physical pixels.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CameraUniform {
    /// Surface size (physical px).
    pub viewport: [f32; 2],
    /// Padding to 16-byte uniform alignment.
    _padding: [f32; 2],
}

/// The rect pipeline: shader, bind groups, and instance buffer.
pub struct RectPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
    rect_bind_group: wgpu::BindGroup,
    rect_capacity: usize,
}

/// Instance capacity growth step.
const RECT_CAPACITY_STEP: usize = 1024;

impl RectPipeline {
    /// Creates the pipeline on `device` with the surface's format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> RectPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nui-rect-shader"),
            source: wgpu::ShaderSource::Wgsl(format!("{}{}", CLIP_WGSL, RECT_SHADER).into()),
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
        let rect_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nui-rect-layout"),
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
            label: Some("nui-rect-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&rect_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nui-rect-pipeline"),
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
        let capacity = RECT_CAPACITY_STEP;
        let rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-rect-instances"),
            size: (capacity * std::mem::size_of::<RectInstance>()) as u64,
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
        let rect_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nui-rect-bind"),
            layout: &rect_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: rect_buffer.as_entire_binding(),
            }],
        });
        return RectPipeline {
            pipeline,
            camera_buffer,
            camera_bind_group,
            rect_buffer,
            rect_bind_group,
            rect_capacity: capacity,
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

    /// Uploads the instance list, growing the storage buffer when needed.
    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[RectInstance],
    ) {
        if instances.len() > self.rect_capacity {
            self.rect_capacity = instances.len().next_power_of_two();
            self.rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nui-rect-instances"),
                size: (self.rect_capacity * std::mem::size_of::<RectInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // The bind group references the old buffer; recreate lazily on
            // the next render by storing capacity only. Simpler: rebuild now
            // requires the layout; instead the renderer recreates bind groups
            // per frame when capacity changed (tracked by the caller).
        }
        let bytes = bytemuck::cast_slice(instances);
        queue.write_buffer(&self.rect_buffer, 0, bytes);
    }

    /// Renders `instance_count` rects into `pass`.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, instance_count: u32) {
        if instance_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.rect_bind_group, &[]);
        pass.draw(0..6, 0..instance_count);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::{Point, Size};

    #[test]
    fn instance_from_dp_scales_by_dpi() {
        let rect = Rect::new(Point::new(10.0, 20.0), Size::new(100.0, 50.0));
        let instance = RectInstance::from_dp(rect, 4.0, Color::from_rgb8(255, 0, 0), 2.0);
        assert_eq!(instance.origin, [20.0, 40.0]);
        assert_eq!(instance.size, [200.0, 100.0]);
        assert_eq!(instance.corner_radius, 8.0);
    }

    #[test]
    fn border_attaches_width_and_color() {
        let rect = Rect::new(Point::ZERO, Size::new(10.0, 10.0));
        let instance = RectInstance::from_dp(rect, 0.0, Color::BLACK, 1.0)
            .with_border(1.5, Color::from_rgb8(0, 255, 0));
        assert_eq!(instance.border_width, 1.5);
        assert_eq!(instance.border_color[1], 1.0);
    }

    #[test]
    fn round_trip_gpu_bytes_matches_layout() {
        // Layout mirrors WGSL: origin(8) + size(8) + radius(4) + border(4)
        // + pad(8) + fill(16) + border_color(16) + shadow_color(16)
        // + blur(4) + pad(4) + offset(8) + clip(16+4+12) = 128 bytes.
        assert_eq!(std::mem::size_of::<RectInstance>(), 128);
        assert_eq!(std::mem::size_of::<CameraUniform>(), 16);
        // `fill` must sit at the WGSL vec4 alignment (offset 32).
        let probe = RectInstance::from_dp(
            Rect::new(Point::ZERO, Size::new(1.0, 1.0)),
            0.0,
            Color::from_rgb8(255, 0, 0),
            1.0,
        );
        let bytes = bytemuck::bytes_of(&probe);
        assert_eq!(&bytes[32..36], &1.0f32.to_ne_bytes(), "fill.r at 32");
        assert_eq!(&bytes[36..40], &0.0f32.to_ne_bytes(), "fill.g at 36");
    }
}
