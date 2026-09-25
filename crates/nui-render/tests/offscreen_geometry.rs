//! Offscreen geometry tests: Polyline and Arc stroking, verified at the
//! pixel level through `render_to_view` (lavapipe/CI). FUTURE batch 1.

use std::collections::HashMap;

use nui_core::{Color, Value};
use nui_render::{LineCap, Renderer, SceneBuilder, SceneContext};
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

/// A root column with one polyline child (absolute points, since the child
/// lands at the column origin).
fn polyline_tree(points: &str, width: f32, cap: LineCap, canvas: (f32, f32)) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(f64::from(canvas.0)));
    root.set("height", Value::Float(f64::from(canvas.1)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut polyline = Element::new("Polyline", None);
    polyline.set("points", Value::String(points.to_string()));
    polyline.set("stroke.width", Value::Float(f64::from(width)));
    polyline.set("color", Value::Color(Color::from_rgb8(255, 0, 0)));
    if cap != LineCap::Round {
        polyline.set("stroke.cap", Value::String("butt".to_string()));
    }
    let polyline_id = tree.insert(polyline);
    tree.append_child(root_id, polyline_id);
    return tree;
}

#[test]
fn polyline_strokes_its_segments() {
    // A 6dp red line from (10,30) to (90,30): half width 3 covers y 27..33.
    let mut tree = polyline_tree("10,30 90,30", 6.0, LineCap::Round, (100.0, 60.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let on_line = pixel(&data, stride, 50, 30);
    assert!(on_line[0] > 180, "line center is red, got {on_line:?}");
    let above = pixel(&data, stride, 50, 10);
    assert!(above[0] < 60, "far above the line stays black, got {above:?}");
    let below = pixel(&data, stride, 50, 55);
    assert!(below[0] < 60, "far below the line stays black, got {below:?}");
}

#[test]
fn round_caps_extend_past_endpoints_butt_does_not() {
    // Round caps bulge a semicircle (radius = half width = 3) past the
    // endpoints at x=10/90; butt caps stop exactly at them.
    let mut rounded = polyline_tree("10,30 90,30", 6.0, LineCap::Round, (100.0, 60.0));
    let (data, stride) = render_pixels(&mut rounded, nui_core::Size::new(100.0, 60.0));
    let inside_cap = pixel(&data, stride, 8, 30);
    assert!(
        inside_cap[0] > 180,
        "round cap covers 2dp past the start, got {inside_cap:?}"
    );
    let past_cap = pixel(&data, stride, 5, 30);
    assert!(
        past_cap[0] < 60,
        "round cap ends 3dp past the start, got {past_cap:?}"
    );

    let mut butt = polyline_tree("10,30 90,30", 6.0, LineCap::Butt, (100.0, 60.0));
    let (data, stride) = render_pixels(&mut butt, nui_core::Size::new(100.0, 60.0));
    let outside_butt = pixel(&data, stride, 8, 30);
    assert!(
        outside_butt[0] < 60,
        "butt cap stops at the endpoint, got {outside_butt:?}"
    );
    let on_butt = pixel(&data, stride, 15, 30);
    assert!(on_butt[0] > 180, "butt line still strokes, got {on_butt:?}");
}

#[test]
fn arc_draws_a_ring() {
    // Full circle (default end = 360): radius 30 around (50,50), width 4
    // (ring spans radius 28..32).
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut arc = Element::new("Arc", None);
    arc.set("cx", Value::Float(50.0));
    arc.set("cy", Value::Float(50.0));
    arc.set("radius", Value::Float(30.0));
    arc.set("stroke.width", Value::Float(4.0));
    arc.set("color", Value::Color(Color::from_rgb8(0, 255, 0)));
    let arc_id = tree.insert(arc);
    tree.append_child(root_id, arc_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    let top = pixel(&data, stride, 50, 20);
    assert!(top[1] > 180, "ring top is green, got {top:?}");
    let right = pixel(&data, stride, 80, 50);
    assert!(right[1] > 180, "ring right vertex is green, got {right:?}");
    let center = pixel(&data, stride, 50, 50);
    assert!(center[1] < 60, "ring interior stays black, got {center:?}");
    let outside = pixel(&data, stride, 50, 2);
    assert!(outside[1] < 60, "beyond the ring stays black, got {outside:?}");
}

#[test]
fn arc_quarter_sweep_stays_within_its_angles() {
    // start = 0, end = 90 (screen coordinates, y down): the sweep runs
    // clockwise from (80,50) to (50,80) through the lower-right quadrant.
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut arc = Element::new("Arc", None);
    arc.set("cx", Value::Float(50.0));
    arc.set("cy", Value::Float(50.0));
    arc.set("radius", Value::Float(30.0));
    arc.set("start", Value::Float(0.0));
    arc.set("end", Value::Float(90.0));
    arc.set("stroke.width", Value::Float(4.0));
    arc.set("color", Value::Color(Color::from_rgb8(0, 255, 0)));
    let arc_id = tree.insert(arc);
    tree.append_child(root_id, arc_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    let on_arc = pixel(&data, stride, 71, 71);
    assert!(
        on_arc[1] > 180,
        "the 45-degree midpoint is stroked, got {on_arc:?}"
    );
    let not_in_sweep = pixel(&data, stride, 50, 20);
    assert!(
        not_in_sweep[1] < 60,
        "the top of the circle is outside the sweep, got {not_in_sweep:?}"
    );
    let opposite = pixel(&data, stride, 29, 29);
    assert!(
        opposite[1] < 60,
        "the 225-degree point is outside the sweep, got {opposite:?}"
    );
}

#[test]
fn clip_cuts_the_polyline() {
    // `clip = true` on the 60-wide root clips the inherited clip onto the
    // child: the line stops at x = 60 even though it extends to 90.
    let mut tree = polyline_tree("10,30 90,30", 6.0, LineCap::Round, (100.0, 60.0));
    let roots = tree.roots.clone();
    let root = &mut tree.arena[roots[0]];
    root.set("width", Value::Float(60.0));
    root.set("clip", Value::Bool(true));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let inside = pixel(&data, stride, 30, 30);
    assert!(inside[0] > 180, "inside the clip the line is red, got {inside:?}");
    let clipped = pixel(&data, stride, 70, 30);
    assert!(clipped[0] < 60, "past the clip the line is cut, got {clipped:?}");
}
