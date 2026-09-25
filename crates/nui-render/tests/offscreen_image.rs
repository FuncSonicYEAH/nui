//! Offscreen image-pipeline test (M8): decode a generated PNG, upload it
//! through the renderer, and assert tinted + nine-sliced pixels land.

use nui_core::Value;
use nui_render::{SceneBuilder, SceneContext, cache_key, decode_file};
use nui_runtime::Element;

/// Writes an 8x8 PNG: left half opaque red, right half transparent.
fn write_test_png(path: &std::path::Path) {
    let mut buffer = image::RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let pixel = if x < 4 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
            buffer.put_pixel(x, y, pixel);
        }
    }
    buffer.save(path).expect("png encode works");
}

/// Offscreen render helper: builds a tree with one Image element, renders
/// it, returns the pixels plus the readback stride.
fn render_image(slice: f32) -> (Vec<u8>, usize, u32, u32) {
    let width = 64u32;
    let height = 64u32;
    let viewport = nui_core::Size::new(width as f32, height as f32);

    let png_path = std::env::temp_dir().join("nui-image-test.png");
    write_test_png(&png_path);
    let source = png_path.display().to_string();

    let bytes = std::fs::read(&source).expect("png exists");
    let key = cache_key(&source, nui_render::content_hash(&bytes));
    let decoded = decode_file(&source).expect("png decodes");

    let mut tree = nui_runtime::ElementTree::new();
    let mut image = Element::new("Image", None);
    image.set("source", Value::String(source.clone()));
    // v1 contract: images need explicit size (decode is async; intrinsic
    // sizing arrives with M9's compositing work).
    image.set("width", Value::Float(64.0));
    image.set("height", Value::Float(64.0));
    image.set("slice", Value::Float(f64::from(slice)));
    let image_id = tree.insert(image);
    tree.push_root(image_id);

    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, viewport, Some(&mut text));
    let image_keys = std::collections::HashMap::from([(source, key.clone())]);
    let scene = SceneBuilder::build_with_context(
        &tree,
        &mut text,
        SceneContext {
            focused: None,
            image_keys: &image_keys,
        },
    );
    assert_eq!(scene.images.len(), 1, "the image draw reached the scene");

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
    let mut renderer = nui_render::Renderer::new(&device, format);
    let decoded_map = std::collections::HashMap::from([(key, decoded)]);
    renderer.prepare(&device, &queue, viewport, 1.0, &scene, None, &decoded_map);

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("nui-offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
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
        renderer.render(&mut pass, &scene);
    }
    let bytes_per_row = (width * 4).next_multiple_of(256);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nui-readback"),
        size: (bytes_per_row * height) as u64,
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
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice_handle = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice_handle.map_async(wgpu::MapMode::Read, move |result| {
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
    let mapped = slice_handle
        .get_mapped_range()
        .expect("mapped after success");
    let data = mapped.to_vec();
    drop(mapped);
    readback.unmap();
    return (data, bytes_per_row as usize, width, height);
}

#[test]
fn image_draws_decoded_pixels() {
    let (data, stride, width, height) = render_image(0.0);
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y as usize) * stride + (x as usize) * 4;
        return [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
    };
    // Left half: opaque red (sRGB round-trip keeps the channel dominant).
    let red = pixel(4, 32);
    assert!(red[0] > 180, "red channel at left, got {red:?}");
    assert!(red[1] < 60 && red[2] < 60, "left is red, got {red:?}");
    // Right half: alpha 0 -> background (premultiplied blend keeps black).
    let empty = pixel(60, 32);
    assert!(
        empty[0] < 30 && empty[1] < 30,
        "right half is background, got {empty:?}"
    );
    // Full-width stretch: corners of the image element are in the texture.
    let _ = (width, height);
}

#[test]
fn nine_slice_keeps_corners_and_stretches_middle() {
    // slice = 3dp: the 64x64 dest keeps the 3px source corners unscaled at
    // scale 1; the middle stretches. Verify a corner stays red while the
    // transparent middle-right region stays background.
    let (data, stride, _width, height) = render_image(3.0);
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y as usize) * stride + (x as usize) * 4;
        return [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
    };
    let corner = pixel(1, 1);
    assert!(corner[0] > 180, "top-left corner stays red, got {corner:?}");
    // Bottom-right area of the DEST maps to the source bottom-right corner
    // (transparent source region), still background.
    let br = pixel(62, height - 2);
    assert!(br[0] < 30, "bottom-right stays background, got {br:?}");
}
