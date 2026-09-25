//! Offscreen path tests: earcut fills and fill+stroke overlays, verified
//! at the pixel level through `render_to_view` (lavapipe/CI).
//! FUTURE batch 2.

use std::collections::HashMap;

use nui_core::{Color, Value};
use nui_render::{Renderer, SceneBuilder, SceneContext};
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

/// A root column with one red-filled path child (absolute coordinates,
/// since the child lands at the column origin).
fn path_tree(d: &str, canvas: (f32, f32)) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(f64::from(canvas.0)));
    root.set("height", Value::Float(f64::from(canvas.1)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut path = Element::new("Path", None);
    path.set("d", Value::String(d.to_string()));
    path.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
    let path_id = tree.insert(path);
    tree.append_child(root_id, path_id);
    return tree;
}

#[test]
fn triangle_fill_covers_interior_not_outside() {
    let mut tree = path_tree("M 10 5 L 90 5 L 50 55 Z", (100.0, 60.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let top = pixel(&data, stride, 50, 10);
    assert!(top[0] > 180, "triangle interior near the top edge is red, got {top:?}");
    let middle = pixel(&data, stride, 50, 30);
    assert!(middle[0] > 180, "triangle center is red, got {middle:?}");
    let outside_left = pixel(&data, stride, 4, 30);
    assert!(outside_left[0] < 60, "left of the triangle stays black, got {outside_left:?}");
    let outside_bottom = pixel(&data, stride, 50, 58);
    assert!(outside_bottom[0] < 60, "below the apex stays black, got {outside_bottom:?}");
}

#[test]
fn concave_star_fills_the_arms_but_not_the_notches() {
    // A 5-pointed star centered at (50,50), outer radius 40, inner radius
    // 16, top arm pointing up. Ear clipping (vs naive fan) is what makes
    // the notches stay empty.
    let mut d = String::new();
    for step in 0..10 {
        let angle = std::f32::consts::TAU * step as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
        let radius = if step % 2 == 0 { 40.0 } else { 16.0 };
        let x = 50.0 + radius * angle.cos();
        let y = 50.0 + radius * angle.sin();
        if step == 0 {
            d.push_str(&format!("M {x:.1} {y:.1} "));
        } else {
            d.push_str(&format!("L {x:.1} {y:.1} "));
        }
    }
    d.push('Z');
    let mut tree = path_tree(&d, (100.0, 100.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    // Center and top arm are inside.
    let center = pixel(&data, stride, 50, 50);
    assert!(center[0] > 180, "star center is red, got {center:?}");
    let top_arm = pixel(&data, stride, 50, 15);
    assert!(top_arm[0] > 180, "top arm is red, got {top_arm:?}");
    // The notch between the top arm and the upper-right arm (36 degrees
    // off vertical, radius 30 > the 16dp valley) is outside.
    let notch = pixel(&data, stride, 68, 26);
    assert!(notch[0] < 60, "the notch between arms stays black, got {notch:?}");
    // Outside the bounding ring entirely.
    let outside = pixel(&data, stride, 95, 95);
    assert!(outside[0] < 60, "beyond the star stays black, got {outside:?}");
}

#[test]
fn stroke_draws_on_top_of_the_fill() {
    // fill red + 4dp white stroke: the outline is white and the interior
    // stays red (fills render beneath strokes).
    let mut tree = {
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", Value::Float(100.0));
        root.set("height", Value::Float(60.0));
        let root_id = tree.insert(root);
        tree.push_root(root_id);
        let mut path = Element::new("Path", None);
        path.set("d", Value::String("M 10 5 L 90 5 L 50 55 Z".to_string()));
        path.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        path.set("stroke.width", Value::Float(4.0));
        path.set("stroke.color", Value::Color(Color::from_rgb8(255, 255, 255)));
        let path_id = tree.insert(path);
        tree.append_child(root_id, path_id);
        tree
    };
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let interior = pixel(&data, stride, 50, 20);
    assert!(
        interior[0] > 180 && interior[1] < 60,
        "interior stays red, got {interior:?}"
    );
    // On the top edge (y = 5, stroke spans 3..7): white wins over red.
    let outline = pixel(&data, stride, 50, 5);
    assert!(
        outline[0] > 180 && outline[1] > 180 && outline[2] > 180,
        "the outline stroke is white, got {outline:?}"
    );
}

#[test]
fn collinear_degenerate_path_paints_nothing() {
    let mut tree = path_tree("M 0 0 L 50 50 L 100 100", (100.0, 60.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let probe = pixel(&data, stride, 50, 50);
    assert!(probe[0] < 60, "a zero-area ring paints nothing, got {probe:?}");
}
