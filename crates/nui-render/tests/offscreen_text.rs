//! Offscreen text-pipeline test (M6): shape + rasterize glyphs through the
//! full renderer into an offscreen target and assert text pixels landed
//! (plan §9 golden rendering, deterministic via the embedded font on
//! lavapipe).

use nui_core::{Color, Size, Value};
use nui_render::{Renderer, SceneBuilder};
use nui_runtime::Element;

/// Builds a tree with one sized `Text` element.
fn build_tree() -> nui_runtime::ElementTree {
    let mut tree = nui_runtime::ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Length(nui_core::Length::Dp(200.0)));
    root.set("height", Value::Length(nui_core::Length::Dp(100.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut label = Element::new("Text", None);
    label.set("content", Value::String("nui".to_string()));
    label.set("color", Value::Color(Color::from_rgb8(255, 255, 255)));
    let label_id = tree.insert(label);
    tree.append_child(root_id, label_id);
    return tree;
}

#[test]
fn text_pipeline_draws_glyph_pixels() {
    let viewport = Size::new(200.0, 100.0);
    let mut tree = build_tree();
    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, viewport, Some(&mut text));
    let scene = SceneBuilder::build_with_context(
        &tree,
        &mut text,
        nui_render::SceneContext {
            focused: None,
            image_keys: &std::collections::HashMap::new(),
        },
    );
    assert!(
        !scene.texts.is_empty(),
        "shaped glyphs must reach the scene"
    );

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
    let mut renderer = Renderer::new(&device, format);
    renderer.prepare(
        &device,
        &queue,
        viewport,
        1.0,
        &scene,
        Some(&mut text),
        &std::collections::HashMap::new(),
    );

    let width = viewport.width as u32;
    let height = viewport.height as u32;
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

    // Assertions over the readback: the glyphs ("nui", white, upper-left)
    // must light up a band of pixels while the background stays black.
    let bytes_per_row = bytes_per_row as usize;
    let mut lit = 0;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = y * bytes_per_row + x * 4;
            if data[offset] > 128 {
                lit += 1;
            }
        }
    }
    assert!(lit > 50, "expected lit glyph pixels, got {lit}");
    // Bottom-right corner (well past the ~1.2×16dp line) stays background.
    let corner = (height as usize - 5) * bytes_per_row + (width as usize - 5) * 4;
    assert_eq!(data[corner], 0, "background must stay black");
}
