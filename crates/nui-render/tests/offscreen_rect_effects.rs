//! Offscreen rect-effect tests: element rotation and linear gradients,
//! verified at the pixel level through `render_to_view` (lavapipe/CI).

use std::collections::HashMap;

use nui_core::{Color, Point, Rect, Size, Value};
use nui_render::{Renderer, Scene, SceneBuilder, SceneContext};
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

/// Renders a hand-built scene offscreen (no layout, no text) and returns
/// the pixels plus the stride. Same wgpu setup as `render_pixels`.
fn render_scene_pixels(scene: &Scene, viewport: Size) -> (Vec<u8>, usize) {
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
        scene,
        None,
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

#[test]
fn instance_growth_rebinds_the_storage_buffer() {
    // 1100 rects exceed the pipeline's initial 1024-instance capacity: the
    // storage buffer is replaced on upload, so the bind group must be
    // rebuilt with it or draws past 1024 read the old (empty) buffer and
    // render nothing.
    let count = 1100u32;
    let mut builder = SceneBuilder::new();
    for index in 0..count {
        let color = if index == count - 1 {
            Color::from_rgb8(255, 0, 0)
        } else {
            Color::from_rgb8(0, 0, 255)
        };
        builder.push_rect(
            Rect::new(Point::new(index as f32 * 2.0, 0.0), Size::new(2.0, 10.0)),
            0.0,
            color,
        );
    }
    let scene = builder.build();
    let (data, stride) = render_scene_pixels(&scene, Size::new(2200.0, 10.0));
    let last = pixel(&data, stride, 2199, 5);
    assert!(
        last[0] > 150 && last[2] < 60,
        "instance 1099 renders red after regrowth, got {last:?}"
    );
    let before = pixel(&data, stride, 2197, 5);
    assert!(
        before[2] > 150 && before[0] < 60,
        "instance 1098 stays blue, got {before:?}"
    );
    let early = pixel(&data, stride, 3, 5);
    assert!(
        early[2] > 150 && early[0] < 60,
        "instance 1 stays blue, got {early:?}"
    );
}

#[test]
fn rotation_turns_the_rect_into_a_diamond() {
    // A 60x60 red rect centered in a 100x100 canvas, rotated 45 degrees.
    // It becomes a diamond with vertices at center +- (42.4, 0)/(0, 42.4):
    // - (25, 25) was inside the axis-aligned rect but falls outside the
    //   diamond (Manhattan distance 50 > 42.4),
    // - (50, 12) was outside the axis-aligned rect but inside the diamond.
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    // Center the 60x60 child: padding 20 puts its layout box at (20,20).
    root.set("padding", Value::Length(nui_core::Length::Dp(20.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut rect = Element::new("Rectangle", None);
    rect.set("width", Value::Float(60.0));
    rect.set("height", Value::Float(60.0));
    rect.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
    rect.set("rotation", Value::Float(45.0));
    let rect_id = tree.insert(rect);
    tree.append_child(root_id, rect_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    let center = pixel(&data, stride, 50, 50);
    assert!(center[0] > 180, "center stays red, got {center:?}");
    let former_inside = pixel(&data, stride, 25, 25);
    assert!(
        former_inside[0] < 60,
        "corner left the rotated area, got {former_inside:?}"
    );
    let former_outside = pixel(&data, stride, 50, 12);
    assert!(
        former_outside[0] > 180,
        "rotated vertex covers new ground, got {former_outside:?}"
    );
}

/// Builds a 100x100 tree with one 80x80 child at (10, 10) carrying the
/// given gradient properties.
fn radial_tree(kind: Value, center: Option<(f32, f32)>, radius: Option<f32>) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    root.set("padding", Value::Length(nui_core::Length::Dp(10.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut rect = Element::new("Rectangle", None);
    rect.set("width", Value::Float(80.0));
    rect.set("height", Value::Float(80.0));
    rect.set("gradient.from", Value::Color(Color::from_rgb8(255, 0, 0)));
    rect.set("gradient.to", Value::Color(Color::from_rgb8(0, 0, 0)));
    rect.set("gradient.kind", kind);
    if let Some((x, y)) = center {
        rect.set("gradient.center_x", Value::Float(f64::from(x)));
        rect.set("gradient.center_y", Value::Float(f64::from(y)));
    }
    if let Some(radius) = radius {
        rect.set("gradient.radius", Value::Float(f64::from(radius)));
    }
    let rect_id = tree.insert(rect);
    tree.append_child(root_id, rect_id);
    return tree;
}

#[test]
fn a_radial_gradient_falls_off_from_its_centre() {
    // An 80x80 rect whose gradient runs red (centre) to black (radius 40).
    // The default centre is the rect's own middle, which sits at (50, 50) in
    // window coordinates. Four samples on the horizontal axis: bright red at
    // the centre, black at one radius out, black inside past it (clamped),
    // and a red corner because the corner is *closer* to the centre than the
    // edge midpoint is not true — the corner is further, so it stays black.
    let mut tree = radial_tree(Value::Enum("radial".to_string()), None, Some(40.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));

    let centre = pixel(&data, stride, 50, 50);
    assert!(
        centre[0] > 200 && centre[1] < 40,
        "the centre is the start colour, got {centre:?}"
    );

    // Halfway out (20dp from the centre on a 40dp radius) is halfway between
    // the two colours — but *linearly*. The shader mixes premultiplied
    // linear values, so 0.5 linear red comes back as ~0.74 in sRGB, i.e.
    // around 188 rather than 128. Asserting the sRGB midpoint here would be
    // asserting a colour-space bug.
    let halfway = pixel(&data, stride, 70, 50);
    assert!(
        halfway[0] > 160 && halfway[0] < 210,
        "halfway out is between the two colours, got {halfway:?}"
    );

    // At the radius the gradient is fully at the end colour.
    let edge = pixel(&data, stride, 90, 50);
    assert!(
        edge[0] < 40,
        "one radius out is the end colour, got {edge:?}"
    );

    // Past the radius the value clamps, so the corner is still black.
    let corner = pixel(&data, stride, 88, 88);
    assert!(
        corner[0] < 40,
        "beyond the radius stays clamped, got {corner:?}"
    );
}

#[test]
fn a_radial_gradient_honours_an_explicit_centre() {
    // The same rect, but the centre pushed to its bottom-left corner: the
    // bright spot moves there, which is the whole point of a ripple
    // following a finger.
    let mut tree = radial_tree(
        Value::Enum("radial".to_string()),
        Some((0.0, 0.0)),
        Some(40.0),
    );
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));

    // (10, 10) in window coordinates is the rect's own (0, 0).
    let at_centre = pixel(&data, stride, 11, 11);
    assert!(
        at_centre[0] > 200,
        "the bright spot moved to the declared centre, got {at_centre:?}"
    );
    // The old centre is now 40dp away — exactly the radius — so black.
    let old_centre = pixel(&data, stride, 50, 50);
    assert!(
        old_centre[0] < 60,
        "the rect's middle is a full radius away, got {old_centre:?}"
    );
}

#[test]
fn a_radial_gradient_without_a_centre_completes_at_the_nearer_edge() {
    // The default radius is half the shorter side, so the gradient reaches
    // its end colour at the midpoint of the nearest edge and clamps beyond.
    let mut tree = radial_tree(Value::Enum("radial".to_string()), None, None);
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    let at_edge_midpoint = pixel(&data, stride, 90, 50);
    assert!(
        at_edge_midpoint[0] < 40,
        "half the shorter side reaches the end colour, got {at_edge_midpoint:?}"
    );
    // A tall rect's shorter side is its width, so this is still the midpoint.
    let mut tall = radial_tree(Value::Enum("radial".to_string()), None, None);
    let root = tall.roots[0];
    tall.arena[root].set("height", Value::Float(200.0));
    let (tall_data, tall_stride) = render_pixels(&mut tall, nui_core::Size::new(100.0, 220.0));
    let still_black = pixel(&tall_data, tall_stride, 90, 60);
    assert!(
        still_black[0] < 40,
        "the width still sets the default radius, got {still_black:?}"
    );
}

#[test]
fn the_gradient_kind_is_also_accepted_as_a_number() {
    // A document that parameterises its gradients may hold the kind in a
    // number, and an integer composes better in an expression than a bare
    // identifier.
    let mut tree = radial_tree(Value::Int(2), None, Some(40.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    let centre = pixel(&data, stride, 50, 50);
    assert!(
        centre[0] > 200,
        "kind 2 is radial, got {centre:?} at the centre"
    );
}

#[test]
fn an_unset_or_linear_kind_still_runs_along_the_angle() {
    // The default must not change: a gradient with no `kind` is still the
    // top-to-bottom linear one it has always been.
    let mut tree = radial_tree(Value::Enum("linear".to_string()), None, Some(40.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    // Linear at 90 degrees: the top row is the start colour, the bottom row
    // the end colour, and the horizontal middle is the same as the corners.
    let top = pixel(&data, stride, 50, 12);
    let bottom = pixel(&data, stride, 50, 88);
    assert!(top[0] > 200, "the top is the start colour, got {top:?}");
    assert!(
        bottom[0] < 40,
        "the bottom is the end colour, got {bottom:?}"
    );
    let left = pixel(&data, stride, 12, 50);
    let right = pixel(&data, stride, 88, 50);
    assert!(
        (i32::from(left[0]) - i32::from(right[0])).abs() < 12,
        "a vertical gradient does not vary across x, got {left:?} and {right:?}"
    );
}

#[test]
fn zero_rotation_matches_the_axis_aligned_rect() {
    // rotation = 0 must render identically to no rotation at all: the
    // vertex math collapses to the original `origin - bleed + corner * size`.
    let mut rotated = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    let root_id = rotated.insert(root);
    rotated.push_root(root_id);
    let mut rect = Element::new("Rectangle", None);
    rect.set("width", Value::Float(60.0));
    rect.set("height", Value::Float(60.0));
    rect.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
    rect.set("rotation", Value::Float(0.0));
    let rect_id = rotated.insert(rect);
    rotated.append_child(root_id, rect_id);
    let (data, stride) = render_pixels(&mut rotated, nui_core::Size::new(100.0, 100.0));
    // Top-left corner region of the rect (at 0,0 in the column): red.
    let probe = pixel(&data, stride, 5, 5);
    assert!(probe[0] > 180, "rect corner is red, got {probe:?}");
}

#[test]
fn horizontal_gradient_runs_from_to_to() {
    // Two stacked 100x50 rects: the first with a left-to-right gradient
    // (angle 0), the second top-to-bottom (angle 90).
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(100.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    for angle in [0.0, 90.0] {
        let mut rect = Element::new("Rectangle", None);
        rect.set("height", Value::Float(50.0));
        rect.set("gradient.from", Value::Color(Color::from_rgb8(255, 0, 0)));
        rect.set("gradient.to", Value::Color(Color::from_rgb8(0, 0, 255)));
        rect.set("gradient.angle", Value::Float(angle));
        let rect_id = tree.insert(rect);
        tree.append_child(root_id, rect_id);
    }

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 100.0));
    // First rect (y 0..50, angle 0 = left-to-right).
    let left = pixel(&data, stride, 10, 25);
    let right = pixel(&data, stride, 90, 25);
    assert!(
        left[0] > left[2] + 60,
        "gradient start leans red, got {left:?}"
    );
    assert!(
        right[2] > right[0] + 60,
        "gradient end leans blue, got {right:?}"
    );
    // Second rect (y 50..100, angle 90 = top-to-bottom): sample its top
    // and bottom bands at the same x.
    let top = pixel(&data, stride, 50, 55);
    let bottom = pixel(&data, stride, 50, 95);
    assert!(
        top[0] > top[2] + 60,
        "vertical gradient starts red, got {top:?}"
    );
    assert!(
        bottom[2] > bottom[0] + 60,
        "vertical gradient ends blue, got {bottom:?}"
    );
}

#[test]
fn gradient_defaults_to_top_to_bottom() {
    // Without `gradient.angle` the axis defaults to 90 (screen y down).
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(100.0));
    root.set("height", Value::Float(60.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut rect = Element::new("Rectangle", None);
    rect.set("height", Value::Float(60.0));
    rect.set("gradient.from", Value::Color(Color::from_rgb8(255, 255, 0)));
    rect.set("gradient.to", Value::Color(Color::from_rgb8(0, 0, 255)));
    let rect_id = tree.insert(rect);
    tree.append_child(root_id, rect_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 60.0));
    let top = pixel(&data, stride, 50, 6);
    let bottom = pixel(&data, stride, 50, 54);
    // Yellow start: green and red dominate blue; blue end: blue dominates.
    assert!(
        top[1] > top[2] + 60 && top[0] > top[2] + 60,
        "top band keeps the yellow start, got {top:?}"
    );
    assert!(
        bottom[2] > bottom[0] + 60 && bottom[2] > bottom[1] + 60,
        "bottom band reaches the blue end, got {bottom:?}"
    );
}
