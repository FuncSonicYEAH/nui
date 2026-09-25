//! Offscreen rendering tests: draw the rect pipeline into an offscreen
//! target, read the pixels back, and assert the geometry landed. CI runs
//! these on lavapipe (software Vulkan) for pixel determinism (plan §9).

use nui_core::{Color, Point, Rect, Size};
use nui_render::{RectInstance, RectPipeline};

/// Renders `instances` at `viewport` (physical px) and returns RGBA pixels.
fn render_pixels(instances: &[RectInstance], viewport: Size) -> Vec<u8> {
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
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
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
    let view = slice.get_mapped_range().expect("mapped after success");
    let data = view.to_vec();
    drop(view);
    readback.unmap();
    return data;
}

/// Reads the pixel at (x, y) physical px from the padded readback.
fn pixel_at(data: &[u8], viewport: Size, x: u32, y: u32) -> [u8; 4] {
    let bytes_per_row = (viewport.width as u32 * 4).next_multiple_of(256) as usize;
    let offset = y as usize * bytes_per_row + x as usize * 4;
    return [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
}

#[test]
fn rect_covers_its_area_only() {
    let viewport = Size::new(64.0, 64.0);
    let rect = RectInstance::from_dp(
        Rect::new(Point::new(16.0, 16.0), Size::new(32.0, 32.0)),
        0.0,
        Color::from_rgb8(255, 0, 0),
        1.0,
    );
    let data = render_pixels(&[rect], viewport);
    // Inside the rect: red.
    let inside = pixel_at(&data, viewport, 32, 32);
    assert!(inside[0] > 200, "inside should be red, got {inside:?}");
    // Outside the rect: background black.
    let outside = pixel_at(&data, viewport, 4, 4);
    assert!(
        outside[0] < 20,
        "outside should stay black, got {outside:?}"
    );
}

#[test]
fn rounded_corners_leave_corners_transparent() {
    let viewport = Size::new(64.0, 64.0);
    let radius = 16.0;
    let rect = RectInstance::from_dp(
        Rect::new(Point::new(0.0, 0.0), Size::new(64.0, 64.0)),
        radius,
        Color::from_rgb8(0, 255, 0),
        1.0,
    );
    let data = render_pixels(&[rect], viewport);
    // Exact corner (0,0) is outside the rounded radius: not green.
    let corner = pixel_at(&data, viewport, 0, 0);
    assert!(
        corner[1] < 100,
        "rounded corner pixel must be mostly background, got {corner:?}"
    );
    // Center stays filled.
    let center = pixel_at(&data, viewport, 32, 32);
    assert!(center[1] > 200, "center must be filled, got {center:?}");
}

#[test]
fn later_instances_paint_on_top() {
    let viewport = Size::new(64.0, 64.0);
    let bottom = RectInstance::from_dp(
        Rect::new(Point::ZERO, Size::new(64.0, 64.0)),
        0.0,
        Color::from_rgb8(255, 0, 0),
        1.0,
    );
    let top = RectInstance::from_dp(
        Rect::new(Point::new(32.0, 32.0), Size::new(32.0, 32.0)),
        0.0,
        Color::from_rgb8(0, 0, 255),
        1.0,
    );
    let data = render_pixels(&[bottom, top], viewport);
    let overlap = pixel_at(&data, viewport, 48, 48);
    assert!(overlap[2] > 200, "later instance wins, got {overlap:?}");
    let uncovered = pixel_at(&data, viewport, 8, 8);
    assert!(
        uncovered[0] > 200,
        "bottom stays visible, got {uncovered:?}"
    );
}
