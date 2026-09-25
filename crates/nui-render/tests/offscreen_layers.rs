//! Offscreen layer tests (M9): shader-side clipping, group opacity layers,
//! and the two-pass gaussian blur, rendered through `render_to_view`.

use std::collections::HashMap;

use nui_core::{Color, Value};
use nui_render::{Renderer, SceneBuilder, SceneContext};
use nui_runtime::{Element, ElementTree};

/// Renders a tree via `render_to_view` into an offscreen target (black
/// background) and returns the pixels plus the stride.
fn render_pixels(tree: &ElementTree, viewport: nui_core::Size) -> (Vec<u8>, usize) {
    let mut text = nui_text::TextSystem::with_embedded_font();
    let scene = SceneBuilder::build_with_context(
        tree,
        &mut text,
        SceneContext {
            focused: None,
            image_keys: &HashMap::new(),
        },
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

    renderer.render_to_view(
        &device,
        &queue,
        &view,
        viewport,
        1.0,
        &scene,
        Some(&mut text),
        &HashMap::new(),
        wgpu::Color::BLACK,
    );

    let bytes_per_row = (width * 4).next_multiple_of(256);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nui-readback"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("nui-readback-encoder"),
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
    return (data, bytes_per_row as usize);
}

/// A red 100x100 rect child inside a clip/layer root of `size`.
fn tree_with_red_child(size: (f64, f64)) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(size.0));
    root.set("height", Value::Float(size.1));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut child = Element::new("Rectangle", None);
    child.set("width", Value::Float(100.0));
    child.set("height", Value::Float(100.0));
    child.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
    let child_id = tree.insert(child);
    tree.append_child(root_id, child_id);
    return tree;
}

fn pixel(data: &[u8], stride: usize, x: u32, y: u32) -> [u8; 4] {
    let offset = y as usize * stride + x as usize * 4;
    return [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
}

#[test]
fn clip_property_crops_overflowing_children() {
    // Column 50x50 with `clip = true`; the child rect overflows to 100x100.
    let mut tree = tree_with_red_child((50.0, 50.0));
    let root = tree.roots[0];
    tree.arena[root].set("clip", Value::Bool(true));
    let (data, stride) = render_pixels(&tree, nui_core::Size::new(100.0, 100.0));

    let inside = pixel(&data, stride, 25, 25);
    assert!(
        inside[0] > 180,
        "clipped interior stays red, got {inside:?}"
    );
    let outside = pixel(&data, stride, 80, 80);
    assert!(outside[0] < 30, "overflow is cropped, got {outside:?}");
}

#[test]
fn without_clip_children_overflow() {
    let tree = tree_with_red_child((50.0, 50.0));
    let (data, stride) = render_pixels(&tree, nui_core::Size::new(100.0, 100.0));
    let outside = pixel(&data, stride, 80, 80);
    assert!(outside[0] > 180, "overflow stays visible, got {outside:?}");
}

#[test]
fn layer_opacity_dims_the_whole_subtree() {
    let mut tree = tree_with_red_child((50.0, 50.0));
    let root = tree.roots[0];
    tree.arena[root].set("layer.opacity", Value::Float(0.5));
    let (data, stride) = render_pixels(&tree, nui_core::Size::new(100.0, 100.0));
    let inside = pixel(&data, stride, 25, 25);
    eprintln!("DBG inside={inside:?}");
    // Group opacity: red at 50% alpha composited over black. The layer
    // texture is sRGB, so the blend happens in linear space: 0.5 alpha on
    // the premultiplied red encodes back to ~187.
    assert!(
        inside[0] > 150 && inside[0] < 220,
        "group opacity halves the color, got {inside:?}"
    );
}

#[test]
fn layer_blur_spreads_bright_pixels() {
    // A 10x10 bright rect inside a blurred layer: pixels well outside the
    // rect must receive blurred light.
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(60.0));
    root.set("height", Value::Float(60.0));
    root.set("layer.blur", Value::Float(6.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut child = Element::new("Rectangle", None);
    child.set("width", Value::Float(10.0));
    child.set("height", Value::Float(10.0));
    child.set("fill", Value::Color(Color::WHITE));
    let child_id = tree.insert(child);
    tree.append_child(root_id, child_id);
    let (data, stride) = render_pixels(&tree, nui_core::Size::new(60.0, 60.0));

    let inside = pixel(&data, stride, 5, 5);
    assert!(
        inside[0] > 150,
        "blur keeps the center bright, got {inside:?}"
    );
    // 4px right of the 10px rect: only the blur reaches there (the
    // 9-tap kernel at step=2 reaches 8px).
    let spread = pixel(&data, stride, 14, 5);
    assert!(spread[0] > 8, "blur spreads light outward, got {spread:?}");
    let far = pixel(&data, stride, 55, 55);
    assert!(far[0] < 8, "far corner stays dark, got {far:?}");
}
