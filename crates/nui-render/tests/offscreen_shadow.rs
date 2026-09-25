//! Offscreen shadow test (M8): the rect pipeline's SDF soft shadow must
//! darken pixels outside the rect within the blur reach.

use nui_core::{Color, Point, Rect, Size};
use nui_render::{RectInstance, RectPipeline};

/// Renders `instances` at `viewport` (physical px) over a white clear and
/// returns RGBA pixels plus the stride.
fn render_pixels(instances: &[RectInstance], viewport: Size) -> (Vec<u8>, usize) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("offscreen adapter (CI provides lavapipe/llvmpipe)");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nui-test-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("test device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut pipeline = RectPipeline::new(&device, format);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("nui-offscreen"),
        size: wgpu::Extent3d {
            width: viewport.width as u32,
            height: viewport.height as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        format,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    pipeline.set_viewport(&queue, viewport.width, viewport.height);
    pipeline.upload_instances(&device, &queue, instances);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("nui-test-encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nui-test-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pipeline.render(&mut pass, instances.len() as u32);
    }
    let bytes_per_row = (viewport.width as u32 * 4).next_multiple_of(256);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nui-readback"),
        size: (bytes_per_row * viewport.height as u32) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: viewport.width as u32,
            height: viewport.height as u32,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll completes");
    receiver
        .recv()
        .expect("map callback runs")
        .expect("map succeeds");
    let mapped = slice.get_mapped_range().expect("mapped after success");
    let data = mapped.to_vec();
    drop(mapped);
    readback.unmap();
    return (data, bytes_per_row as usize);
}

#[test]
fn shadow_darkens_outside_the_rect() {
    // Rect 40x40 at (10,10), shadow offset (0, 8), blur 6, black @ 60%.
    let rect = Rect::new(Point::new(10.0, 10.0), Size::new(40.0, 40.0));
    let instance = RectInstance::from_dp(rect, 4.0, Color::from_rgb8(30, 40, 60), 1.0).with_shadow(
        [0.0, 8.0],
        6.0,
        Color::from_rgba8(0, 0, 0, 153),
        1.0,
    );
    let viewport = Size::new(80.0, 80.0);
    eprintln!(
        "DBG inst shadow_color={:?} blur={} offset={:?}",
        instance.shadow_color, instance.shadow_blur, instance.shadow_offset
    );
    let (data, stride) = render_pixels(&[instance], viewport);
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y as usize) * stride + (x as usize) * 4;
        return [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
    };

    // Inside the rect: the panel color dominates.
    // Below the rect (y 52..58): inside the blur reach but outside the
    // panel — must be darker than the white background.
    let shadow_zone = pixel(30, 55);
    eprintln!("DBG zone={shadow_zone:?}");
    assert!(
        shadow_zone[0] < 240,
        "shadow must darken below the rect, got {shadow_zone:?}"
    );
    // Far corner: untouched white background.
    let far = pixel(76, 76);
    assert!(far[0] > 250, "far corner stays background, got {far:?}");
}
