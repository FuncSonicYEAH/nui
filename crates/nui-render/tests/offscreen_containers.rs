//! Offscreen container tests: the layout containers added in FUTURE
//! 批次 5 (`Panel`, `Card`, `Stack`, `Grid`, `Separator`) verified at the
//! pixel level through `render_to_view`.
//!
//! The scene tests already pin the *parts*; these pin that the parts land
//! where a user would look, which is the half a draw-list assertion cannot
//! see (the shadow halo outside a card, an overlapping stack's z order, a
//! separator's hairline surviving rasterization).

use std::collections::HashMap;

use nui_core::{Color, Length, Value};
use nui_render::{Renderer, SceneBuilder, SceneContext};
use nui_runtime::{Element, ElementTree};

/// Renders a tree via `render_to_view` into an offscreen target (black
/// background) and returns the pixels plus the stride.
fn render_pixels(tree: &mut ElementTree, viewport: nui_core::Size) -> (Vec<u8>, usize) {
    return render_pixels_on(tree, viewport, wgpu::Color::BLACK);
}

/// The same render, on a chosen clear colour. A shadow cannot be observed
/// over black: darkening black leaves black. Tests that probe a shadow
/// paint onto a mid grey instead.
fn render_pixels_on(
    tree: &mut ElementTree,
    viewport: nui_core::Size,
    clear: wgpu::Color,
) -> (Vec<u8>, usize) {
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
        clear,
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

/// Luminance, for "is anything drawn here at all" checks.
fn luma(px: [u8; 4]) -> u32 {
    return px[0] as u32 + px[1] as u32 + px[2] as u32;
}

fn widget(ty: &str, width: f32, height: f32) -> Element {
    let mut element = Element::new(ty, None);
    element.set("width", Value::Length(Length::Dp(width)));
    element.set("height", Value::Length(Length::Dp(height)));
    return element;
}

// ----------------------------------------------------------------- Panel

/// Paints `element` alone in a viewport exactly its size, then returns a
/// viewport with a `margin` dp of clear space around it so "outside the
/// box" pixels exist to probe.
fn padded(element: Element, width: f32, height: f32, margin: f32) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("padding", Value::Length(Length::Dp(margin)));
    root.set("width", Value::Float(f64::from(width + margin * 2.0)));
    root.set("height", Value::Float(f64::from(height + margin * 2.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let id = tree.insert(element);
    tree.append_child(root_id, id);
    return tree;
}

fn padded_viewport(width: f32, height: f32, margin: f32) -> nui_core::Size {
    return nui_core::Size::new(width + margin * 2.0, height + margin * 2.0);
}

#[test]
fn a_panel_paints_its_surface_over_its_own_box_only() {
    let margin = 20.0;
    let mut tree = padded(widget("Panel", 200.0, 120.0), 200.0, 120.0, margin);
    let viewport = padded_viewport(200.0, 120.0, margin);
    let (data, stride) = render_pixels(&mut tree, viewport);
    let inside = pixel(&data, stride, 120, 80);
    assert!(
        luma(inside) > 60,
        "the panel surface fills the box, got {inside:?}"
    );
    // 5dp above the top edge is still background.
    let above = pixel(&data, stride, 120, 10);
    assert!(
        luma(above) < 40,
        "outside the box stays clear, got {above:?}"
    );
}

#[test]
fn a_card_casts_a_shadow_outside_its_box() {
    let margin = 24.0;
    let mut card = widget("Card", 200.0, 120.0);
    let mut plain = widget("Panel", 200.0, 120.0);
    card.set("fill", Value::Color(Color::from_rgb8(0x2a, 0x30, 0x3a)));
    plain.set("fill", Value::Color(Color::from_rgb8(0x2a, 0x30, 0x3a)));

    let viewport = padded_viewport(200.0, 120.0, margin);
    // The card must be observed against a mid grey: a shadow can only
    // darken, and it cannot darken the black default clear colour.
    let grey = wgpu::Color {
        r: 0.5,
        g: 0.5,
        b: 0.5,
        a: 1.0,
    };
    let mut card_tree = padded(card, 200.0, 120.0, margin);
    let (card_pixels, card_stride) = render_pixels_on(&mut card_tree, viewport, grey);
    let mut plain_tree = padded(plain, 200.0, 120.0, margin);
    let (plain_pixels, plain_stride) = render_pixels_on(&mut plain_tree, viewport, grey);

    // Sample the band just under the bottom edge (the card's box ends at
    // y = 24 + 120 = 144) and keep the darkest row: the shadow peaks near
    // the box and fades with the 10dp blur.
    let x = 124;
    let row = |data: &[u8], stride: usize| -> u32 {
        return (145..154)
            .map(|y| return luma(pixel(data, stride, x, y)))
            .min()
            .expect("a non-empty band");
    };
    let under_card = row(&card_pixels, card_stride);
    let under_plain = row(&plain_pixels, plain_stride);
    assert!(
        under_card + 8 < under_plain,
        "the card's shadow darkens the space under it: {under_plain} vs {under_card}"
    );
}

#[test]
fn a_titled_panel_draws_a_rule_across_it() {
    let margin = 20.0;
    let mut panel = widget("Panel", 200.0, 120.0);
    panel.set("title", Value::String("Details".to_string()));
    let viewport = padded_viewport(200.0, 120.0, margin);
    let mut tree = padded(panel, 200.0, 120.0, margin);
    let (data, stride) = render_pixels(&mut tree, viewport);

    // The rule sits at padding (16) + title (16) + gap (8) = 40dp below
    // the panel's top edge, which is at `margin`. It is one dp thick and
    // centred on a pixel boundary, so both rows it touches come out at
    // partial coverage; the brightest of the two is the line.
    let rule_y = (margin + 40.0) as u32;
    let x = margin as u32 + 100;
    let off_rule = pixel(&data, stride, x, rule_y + 8);
    let on_rule = [rule_y - 1, rule_y]
        .into_iter()
        .map(|y| return pixel(&data, stride, x, y))
        .max_by_key(|px| return luma(*px))
        .expect("two candidate rows");
    assert!(
        luma(on_rule) > luma(off_rule) + 20,
        "the title rule is a visible line: {off_rule:?} -> {on_rule:?}"
    );
}

// ----------------------------------------------------------------- Stack

#[test]
fn a_stack_puts_the_later_child_on_top() {
    let mut tree = ElementTree::new();
    let mut stack = Element::new("Stack", None);
    stack.set("width", Value::Length(Length::Dp(60.0)));
    stack.set("height", Value::Length(Length::Dp(40.0)));
    let stack_id = tree.insert(stack);
    tree.push_root(stack_id);
    for fill in [
        Color::from_rgb8(0xee, 0x22, 0x22),
        Color::from_rgb8(0x22, 0x44, 0xee),
    ] {
        let mut child = Element::new("Rectangle", None);
        child.set("fill", Value::Color(fill));
        let child_id = tree.insert(child);
        tree.append_child(stack_id, child_id);
    }
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(60.0, 40.0));
    let center = pixel(&data, stride, 30, 20);
    assert!(
        center[2] > center[0],
        "the second child (blue) covers the first (red): {center:?}"
    );
}

// ----------------------------------------------------------------- Grid

#[test]
fn a_grid_paints_a_gap_between_its_cells() {
    let mut tree = ElementTree::new();
    let mut grid = Element::new("Grid", None);
    grid.set("width", Value::Length(Length::Dp(100.0)));
    grid.set("height", Value::Length(Length::Dp(40.0)));
    grid.set("columns", Value::Int(2));
    grid.set("column_spacing", Value::Length(Length::Dp(20.0)));
    let grid_id = tree.insert(grid);
    tree.push_root(grid_id);
    for _ in 0..2 {
        let mut cell = Element::new("Rectangle", None);
        cell.set("height", Value::Length(Length::Dp(20.0)));
        cell.set("fill", Value::Color(Color::from_rgb8(0xdd, 0xdd, 0xdd)));
        let cell_id = tree.insert(cell);
        tree.append_child(grid_id, cell_id);
    }
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(100.0, 40.0));
    // Tracks are (100 - 20) / 2 = 40 wide, so the gap spans x = 40..60.
    let in_cell = pixel(&data, stride, 20, 10);
    let in_gap = pixel(&data, stride, 50, 10);
    assert!(
        luma(in_cell) > luma(in_gap) + 100,
        "the column gap stays clear: cell {in_cell:?} vs gap {in_gap:?}"
    );
}

// ------------------------------------------------------------- Separator

#[test]
fn a_separator_paints_a_visible_hairline() {
    let mut tree = ElementTree::new();
    let mut column = Element::new("Column", None);
    column.set("width", Value::Length(Length::Dp(80.0)));
    column.set("height", Value::Length(Length::Dp(40.0)));
    let column_id = tree.insert(column);
    tree.push_root(column_id);
    let mut top = Element::new("Rectangle", None);
    top.set("height", Value::Length(Length::Dp(19.0)));
    let top_id = tree.insert(top);
    let mut separator = Element::new("Separator", None);
    separator.set("color", Value::Color(Color::from_rgb8(0xff, 0x00, 0x00)));
    let separator_id = tree.insert(separator);
    tree.append_child(column_id, top_id);
    tree.append_child(column_id, separator_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(80.0, 40.0));
    let on_line = pixel(&data, stride, 40, 19);
    assert!(
        on_line[0] > 100 && on_line[0] > on_line[2],
        "the separator's colour lands on its own dp row: {on_line:?}"
    );
}
