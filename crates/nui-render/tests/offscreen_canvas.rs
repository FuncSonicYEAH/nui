//! Offscreen canvas tests: the behavior's command buffer interpreted into
//! fills and strokes, composited through the layer pipeline. Pixel probes
//! via `render_to_view` (lavapipe/CI). FUTURE batch 3.

use std::collections::HashMap;

use nui_core::{Color, Value};
use nui_render::{Renderer, SceneBuilder, SceneContext};
use nui_runtime::canvas::CanvasPainter;
use nui_runtime::{Element, ElementTree};

/// Renders a tree via `render_to_view` into an offscreen target (black
/// background) and returns the pixels plus the stride.
fn render_pixels(tree: &mut ElementTree, viewport: nui_core::Size) -> (Vec<u8>, usize) {
    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(tree, viewport, Some(&mut text));
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

fn pixel(data: &[u8], stride: usize, x: u32, y: u32) -> [u8; 4] {
    let offset = y as usize * stride + x as usize * 4;
    return [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
}

/// Builds a Canvas element with a painter that draws a red triangle
/// (filled) and a quarter-circle arc (stroked white, kappa cubic form:
/// center (60,60), radius 40, top to right vertex).
fn canvas_tree() -> ElementTree {
    let painter = CanvasPainter::new();
    painter.move_to(10.0, 10.0);
    painter.line_to(90.0, 10.0);
    painter.line_to(50.0, 80.0);
    painter.close();
    painter.fill(Color::from_rgb8(255, 0, 0));
    // Canvas semantics: fill/stroke do not consume the path — Clear drops
    // it so the arc below strokes alone.
    painter.clear();
    painter.move_to(60.0, 20.0);
    // Quarter circle, kappa cubic form: center (60,60), radius 40, from
    // the top vertex to the right vertex. Control points sit on the
    // tangent lines, kappa*R = 22.08 away.
    painter.cubic_to(82.08, 20.0, 100.0, 37.92, 100.0, 60.0);
    painter.stroke(4.0, nui_runtime::canvas::CanvasCap::Round, Color::WHITE);

    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(120.0));
    root.set("height", Value::Float(100.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut canvas = Element::new("Canvas", None);
    canvas.set("width", Value::Float(120.0));
    canvas.set("height", Value::Float(100.0));
    canvas.canvas = Some(painter);
    let canvas_id = tree.insert(canvas);
    tree.append_child(root_id, canvas_id);
    return tree;
}

#[test]
fn canvas_interprets_fills_and_strokes_through_the_layer_pipeline() {
    let mut tree = canvas_tree();
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 100.0));
    // Triangle interior is red (canvas-local coordinates == screen
    // coordinates here, since the canvas sits at the column origin).
    let triangle = pixel(&data, stride, 50, 40);
    assert!(triangle[0] > 180, "triangle fill is red, got {triangle:?}");
    // The 45-degree point of the arc (60 + 28.3, 60 - 28.3) carries the
    // white stroke.
    let arc = pixel(&data, stride, 88, 32);
    assert!(
        arc[0] > 180 && arc[1] > 180 && arc[2] > 180,
        "the arc stroke is white, got {arc:?}"
    );
    // Inside the arc's circle but off both shapes stays black (no fill on
    // the stroke path): (75,45) is 21dp from the arc's center (60,60) —
    // inside the sector — and outside the triangle.
    let arc_interior = pixel(&data, stride, 75, 45);
    assert!(
        arc_interior[0] < 60 && arc_interior[1] < 60,
        "the stroked arc does not fill, got {arc_interior:?}"
    );
    // Outside the canvas content.
    let outside = pixel(&data, stride, 115, 95);
    assert!(outside[0] < 60, "empty canvas area stays black, got {outside:?}");
}

#[test]
fn canvas_without_a_painter_paints_nothing() {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(60.0));
    root.set("height", Value::Float(60.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut canvas = Element::new("Canvas", None);
    canvas.set("width", Value::Float(60.0));
    canvas.set("height", Value::Float(60.0));
    let canvas_id = tree.insert(canvas);
    tree.append_child(root_id, canvas_id);
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(60.0, 60.0));
    let probe = pixel(&data, stride, 30, 30);
    assert!(probe[0] < 60, "a shell without a painter paints nothing, got {probe:?}");
}
