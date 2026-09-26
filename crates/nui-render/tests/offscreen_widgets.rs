//! Offscreen widget tests: the controls built in FUTURE 控件扩展 批次 1-3,
//! verified at the pixel level through `render_to_view` (lavapipe/CI).
//!
//! These complement the draw-list assertions in `scene.rs`: the scene
//! tests say *what parts* a control produces, these say that the parts
//! land on screen where a user would look for them.

use std::collections::HashMap;

use nui_core::{Color, Length, Value};
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

/// A root column sized to the viewport with `element` as its only child,
/// so the child lands at the origin.
fn rooted(element: Element, viewport: (f32, f32)) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(f64::from(viewport.0)));
    root.set("height", Value::Float(f64::from(viewport.1)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let id = tree.insert(element);
    tree.append_child(root_id, id);
    return tree;
}

// ---------------------------------------------------------------- Button

#[test]
fn button_at_rest_paints_its_surface_and_center_stays_flat() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    // The interior is the palette surface (a mid-dark grey), not black.
    let center = pixel(&data, stride, 60, 18);
    assert!(
        luma(center) > 90,
        "the surface fills the box, got {center:?}"
    );
    // Corners are still background: the box is exactly the declared size.
    let corner = pixel(&data, stride, 119, 35);
    assert!(
        luma(corner) < 70,
        "outside the box stays clear, got {corner:?}"
    );
}

#[test]
fn hovering_a_button_brightens_its_surface_only() {
    let mut resting = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let (rest, rest_stride) = render_pixels(&mut resting, nui_core::Size::new(120.0, 36.0));

    let mut hovered = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    // The tracker writes `hovered` as a property; the renderer reads it.
    let root = hovered.roots.first().copied().expect("one root");
    let child = hovered.arena[root].children[0];
    hovered.arena[child].set("hovered", Value::Bool(true));
    let (hover, hover_stride) = render_pixels(&mut hovered, nui_core::Size::new(120.0, 36.0));

    let before = pixel(&rest, rest_stride, 60, 18);
    let after = pixel(&hover, hover_stride, 60, 18);
    assert!(
        luma(after) > luma(before),
        "hover raises the surface luminance: {before:?} -> {after:?}"
    );
    // The box edge does not move just because the pointer arrived.
    assert!(luma(pixel(&hover, hover_stride, 119, 35)) < 70);
}

#[test]
fn an_armed_button_reads_darker_than_a_hovered_one() {
    let mut hovered = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = hovered.arena[hovered.roots[0]].children[0];
    hovered.arena[child].set("hovered", Value::Bool(true));
    let (hover, hover_stride) = render_pixels(&mut hovered, nui_core::Size::new(120.0, 36.0));

    let mut pressed = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = pressed.arena[pressed.roots[0]].children[0];
    // `armed` (pointer down *and* inside) is what the renderer keys the
    // pressed look off, not `pressed`.
    pressed.arena[child].set("armed", Value::Bool(true));
    let (down, down_stride) = render_pixels(&mut pressed, nui_core::Size::new(120.0, 36.0));

    let hover_px = pixel(&hover, hover_stride, 60, 18);
    let down_px = pixel(&down, down_stride, 60, 18);
    assert!(
        luma(down_px) < luma(hover_px),
        "pressing darkens the surface: {hover_px:?} -> {down_px:?}"
    );
}

#[test]
fn a_dragged_out_button_drops_the_press_look() {
    // `pressed` but not `armed`: the pointer left the box mid-gesture, so
    // the highlight must go with it.
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("pressed", Value::Bool(true));
    tree.arena[child].set("armed", Value::Bool(false));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));

    let mut rest = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let (rest_data, rest_stride) = render_pixels(&mut rest, nui_core::Size::new(120.0, 36.0));
    assert_eq!(
        luma(pixel(&data, stride, 60, 18)),
        luma(pixel(&rest_data, rest_stride, 60, 18)),
        "`pressed` alone must not paint the pressed state"
    );
}

#[test]
fn a_disabled_button_paints_no_hover_look_when_the_pointer_fakes_one() {
    // `disabled` outranks `hovered` in the visual-state ladder.
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("enabled", Value::Bool(false));
    tree.arena[child].set("hovered", Value::Bool(true));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));

    let mut hovered = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = hovered.arena[hovered.roots[0]].children[0];
    hovered.arena[child].set("hovered", Value::Bool(true));
    let (hover, hover_stride) = render_pixels(&mut hovered, nui_core::Size::new(120.0, 36.0));

    assert!(
        luma(pixel(&data, stride, 60, 18)) < luma(pixel(&hover, hover_stride, 60, 18)),
        "a disabled button stays muted even with hovered = true"
    );
}

#[test]
fn a_primary_variant_fills_with_the_accent_colour() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("variant", Value::Enum("primary".to_string()));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    let center = pixel(&data, stride, 60, 18);
    // The default accent is blue: the blue channel leads.
    assert!(
        center[2] as i32 > 150 && center[2] as i32 > center[0] as i32 + 60,
        "a primary button is accent blue, got {center:?}"
    );
}

#[test]
fn a_danger_variant_fills_with_red() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("variant", Value::String("danger".to_string()));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    let center = pixel(&data, stride, 60, 18);
    assert!(
        center[0] as i32 > 150 && center[0] as i32 > center[2] as i32 + 60,
        "a danger button is red, got {center:?}"
    );
}

#[test]
fn a_ghost_variant_stays_transparent_inside() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("variant", Value::String("ghost".to_string()));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    let center = pixel(&data, stride, 60, 18);
    assert!(
        luma(center) < 70,
        "a ghost button has no fill, got {center:?}"
    );
    // But its hairline outline is there.
    let edge = pixel(&data, stride, 0, 18);
    assert!(
        luma(edge) > luma(center),
        "the ghost hairline outlines the box: {center:?} vs {edge:?}"
    );
}

#[test]
fn a_button_label_paints_glyphs_inside_the_box() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("label", Value::String("Save".to_string()));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    // Scan the middle band for a pixel that is brighter than the surface:
    // glyph coverage. (The exact glyph shape is the text system's
    // business; that *something* is drawn is ours.)
    let surface = pixel(&data, stride, 4, 18);
    let mut brightest = 0;
    for x in 0..120 {
        let l = luma(pixel(&data, stride, x, 18));
        brightest = brightest.max(l);
    }
    assert!(
        brightest > luma(surface) + 60,
        "the label draws glyphs brighter than the surface ({surface:?}), peak {brightest}"
    );
}

// -------------------------------------------------------------- CheckBox

#[test]
fn a_checked_checkbox_shows_an_accent_box() {
    let mut unchecked = rooted(widget("CheckBox", 140.0, 20.0), (140.0, 20.0));
    let (off, off_stride) = render_pixels(&mut unchecked, nui_core::Size::new(140.0, 20.0));

    let mut checked = rooted(widget("CheckBox", 140.0, 20.0), (140.0, 20.0));
    let child = checked.arena[checked.roots[0]].children[0];
    checked.arena[child].set("checked", Value::Bool(true));
    let (on, on_stride) = render_pixels(&mut checked, nui_core::Size::new(140.0, 20.0));

    // The indicator is the left 20dp square; probe a corner of it, away
    // from the tick stroke.
    let off_px = pixel(&off, off_stride, 3, 4);
    let on_px = pixel(&on, on_stride, 3, 4);
    assert!(
        on_px[2] as i32 > 140 && on_px[2] as i32 > off_px[2] as i32 + 60,
        "checking fills the indicator with accent: {off_px:?} -> {on_px:?}"
    );
}

#[test]
fn a_checked_checkbox_draws_a_tick_inside_the_indicator() {
    let mut tree = rooted(widget("CheckBox", 140.0, 20.0), (140.0, 20.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("checked", Value::Bool(true));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(140.0, 20.0));
    // The tick's elbow sits near (8.4, 15) for a 20dp box: the low point
    // of the polyline. It is foreground (near-white) over accent blue.
    let elbow = pixel(&data, stride, 8, 15);
    assert!(
        elbow[0] > 150 && elbow[1] > 150,
        "the tick is drawn over the indicator, got {elbow:?}"
    );
}

#[test]
fn an_unchecked_checkbox_has_no_tick() {
    let mut tree = rooted(widget("CheckBox", 140.0, 20.0), (140.0, 20.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(140.0, 20.0));
    let elbow = pixel(&data, stride, 8, 15);
    // The indicator's own surface is [42, 48, 58]; only a tick would
    // brighten it.
    assert!(
        luma(elbow) < 200,
        "nothing is drawn inside an empty indicator, got {elbow:?}"
    );
}

// ------------------------------------------------------------ RadioButton

#[test]
fn a_selected_radio_shows_an_accent_ring_and_a_light_dot() {
    let mut tree = rooted(widget("RadioButton", 140.0, 20.0), (140.0, 20.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("selected", Value::Bool(true));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(140.0, 20.0));
    // The indicator is a full 20dp accent circle with a 42% inner dot in
    // the foreground colour: the centre is light, the ring band is blue.
    let centre = pixel(&data, stride, 10, 10);
    assert!(
        centre[0] > 150 && centre[1] > 150,
        "the inner dot is the foreground colour, got {centre:?}"
    );
    // The inner dot is 8.4dp across, centred at x = 10, so it spans
    // 5.8..14.2. A probe at x = 3 is inside the 20dp accent circle but
    // clear of the dot.
    let band = pixel(&data, stride, 3, 10);
    assert!(
        band[2] as i32 > 140 && band[2] as i32 > band[0] as i32 + 40,
        "the ring band is accent blue, got {band:?}"
    );
}

#[test]
fn an_unselected_radio_has_a_hollow_centre() {
    let mut tree = rooted(widget("RadioButton", 140.0, 20.0), (140.0, 20.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(140.0, 20.0));
    // The middle of an empty indicator is its own dark surface: the ring
    // is a hairline at the very edge, not a fill.
    let centre = pixel(&data, stride, 10, 10);
    assert!(
        centre[2] as i32 <= centre[0] as i32 + 30,
        "an empty radio shows the dark surface, got {centre:?}"
    );
    // Contrast with the selected case above: no accent anywhere inside.
    assert!(
        centre[2] < 120,
        "an empty radio has no accent fill, got {centre:?}"
    );
    // The hairline ring is on the boundary, at x = 0.
    let ring = pixel(&data, stride, 0, 10);
    assert!(
        luma(ring) > luma(centre),
        "the ring hairline is visible against the surface: {centre:?} vs {ring:?}"
    );
}

// ----------------------------------------------------------------- Switch

#[test]
fn a_switch_thumb_sits_left_off_and_right_on() {
    let mut off = rooted(widget("Switch", 160.0, 22.0), (160.0, 22.0));
    let (off_data, off_stride) = render_pixels(&mut off, nui_core::Size::new(160.0, 22.0));

    let mut on = rooted(widget("Switch", 160.0, 22.0), (160.0, 22.0));
    let child = on.arena[on.roots[0]].children[0];
    on.arena[child].set("checked", Value::Bool(true));
    let (on_data, on_stride) = render_pixels(&mut on, nui_core::Size::new(160.0, 22.0));

    // Track: height 22, width 22 * 1.9 = 41.8, thumb 18 inset 2.
    // Off the thumb spans x 2..20, on it spans x 21.8..39.8. The lit
    // track is accent blue, so "is the thumb here" is a *hue* question on
    // the checked switch and a *brightness* one on the unchecked one.
    // Both probes sit on the track's vertical centre (y = 11), where the
    // thumb fills the whole height.
    let left_off = pixel(&off_data, off_stride, 11, 11);
    let left_on = pixel(&on_data, on_stride, 11, 11);
    assert!(
        left_off[2] as i32 <= left_off[0] as i32 + 30,
        "with the switch off, the left half is the grey thumb, got {left_off:?}"
    );
    assert!(
        left_on[2] as i32 > left_on[0] as i32 + 60,
        "with the switch on, the left half is bare accent track, got {left_on:?}"
    );
    // The right half is the mirror image: bare track off, thumb on.
    let right_off = pixel(&off_data, off_stride, 31, 11);
    let right_on = pixel(&on_data, on_stride, 31, 11);
    assert!(
        right_off[2] as i32 <= right_off[0] as i32 + 30,
        "with the switch off, the right half is bare grey track, got {right_off:?}"
    );
    assert!(
        right_on[2] as i32 <= right_on[0] as i32 + 30,
        "with the switch on, the right half is the grey thumb, got {right_on:?}"
    );
    // The thumb is far brighter than the grey track it sits on, so the
    // two ends really did swap.
    assert!(
        luma(left_off) > luma(right_off),
        "the thumb starts on the left: {left_off:?} vs {right_off:?}"
    );
    assert!(
        luma(right_on) > luma(left_on),
        "the thumb ends on the right: {left_on:?} vs {right_on:?}"
    );
}

#[test]
fn a_checked_switch_track_turns_accent() {
    let mut tree = rooted(widget("Switch", 160.0, 22.0), (160.0, 22.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("checked", Value::Bool(true));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(160.0, 22.0));
    // Behind the thumb, near the right end of the track: accent blue.
    let track = pixel(&data, stride, 39, 4);
    assert!(
        track[2] as i32 > 120 && track[2] as i32 > track[0] as i32 + 30,
        "the lit track is accent blue, got {track:?}"
    );
}

// ----------------------------------------------------------------- Slider

#[test]
fn a_half_filled_slider_splits_accent_and_track() {
    let mut slider = widget("Slider", 200.0, 20.0);
    slider.set("min", Value::Float(0.0));
    slider.set("max", Value::Float(100.0));
    slider.set("value", Value::Float(50.0));
    let mut tree = rooted(slider, (200.0, 20.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 20.0));

    // Track is 4dp tall, centred: y = 8..12. Thumb travel is [8, 192], so
    // value 50 puts the thumb centre at x = 100.
    let filled = pixel(&data, stride, 40, 10);
    let empty = pixel(&data, stride, 180, 10);
    assert!(
        filled[2] as i32 > 120 && filled[2] as i32 > filled[0] as i32 + 30,
        "the filled lead-in is accent blue, got {filled:?}"
    );
    assert!(
        luma(empty) < luma(filled),
        "the unfilled tail is the dark track: {filled:?} vs {empty:?}"
    );
}

#[test]
fn a_slider_thumb_marks_the_value_position() {
    let mut slider = widget("Slider", 200.0, 20.0);
    slider.set("min", Value::Float(0.0));
    slider.set("max", Value::Float(100.0));
    slider.set("value", Value::Float(0.0));
    let mut tree = rooted(slider, (200.0, 20.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 20.0));

    // At value 0 the 16dp thumb spans x 0..16, vertically 2..18. Its
    // centre column is bright foreground; further right the track is
    // thinner (only y 8..12 lit).
    let thumb_above_track = pixel(&data, stride, 8, 4);
    let track_only = pixel(&data, stride, 120, 4);
    assert!(
        luma(thumb_above_track) > luma(track_only),
        "the thumb is taller than the track: {thumb_above_track:?} vs {track_only:?}"
    );
}

#[test]
fn a_slider_at_max_puts_its_thumb_at_the_far_end() {
    let mut slider = widget("Slider", 200.0, 20.0);
    slider.set("min", Value::Float(0.0));
    slider.set("max", Value::Float(100.0));
    slider.set("value", Value::Float(100.0));
    let mut tree = rooted(slider, (200.0, 20.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 20.0));
    // The thumb spans x 184..200 at max; probe its centre column.
    let thumb = pixel(&data, stride, 192, 4);
    let beyond = pixel(&data, stride, 8, 4);
    assert!(
        luma(thumb) > luma(beyond),
        "the thumb sits at the far end: {beyond:?} vs {thumb:?}"
    );
}

#[test]
fn a_zero_size_widget_paints_nothing() {
    // Layout gives it no box, so there is nothing to draw and nothing to
    // click — the control must not fall back to a full-canvas rect.
    let mut tree = rooted(widget("Button", 0.0, 0.0), (60.0, 60.0));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(60.0, 60.0));
    let probe = pixel(&data, stride, 30, 30);
    assert!(
        luma(probe) < 40,
        "an empty button draws nothing, got {probe:?}"
    );
}

#[test]
fn opacity_scales_a_control_surface() {
    let mut opaque = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let (solid, solid_stride) = render_pixels(&mut opaque, nui_core::Size::new(120.0, 36.0));

    let mut faded = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = faded.arena[faded.roots[0]].children[0];
    faded.arena[child].set("opacity", Value::Float(0.3));
    let (dim, dim_stride) = render_pixels(&mut faded, nui_core::Size::new(120.0, 36.0));

    let full = pixel(&solid, solid_stride, 60, 18);
    let scaled = pixel(&dim, dim_stride, 60, 18);
    assert!(
        luma(scaled) < luma(full),
        "opacity 0.3 dims the surface: {full:?} -> {scaled:?}"
    );
}

#[test]
fn a_custom_fill_overrides_the_default_surface() {
    let mut tree = rooted(widget("Button", 120.0, 36.0), (120.0, 36.0));
    let child = tree.arena[tree.roots[0]].children[0];
    tree.arena[child].set("fill", Value::Color(Color::from_rgb8(0, 255, 0)));
    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(120.0, 36.0));
    let center = pixel(&data, stride, 60, 18);
    assert!(
        center[1] as i32 > 150 && center[1] as i32 > center[0] as i32 + 60,
        "a custom fill wins over the palette, got {center:?}"
    );
}

// ----------------------------------------------------------------- Dialog

#[test]
fn an_open_dialog_dims_what_is_behind_it() {
    // A full-window green rect with a dialog on top: the scrim must mute
    // the green where the panel is not.
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(200.0));
    root.set("height", Value::Float(200.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut background = Element::new("Rectangle", None);
    background.set("width", Value::Float(200.0));
    background.set("height", Value::Float(200.0));
    background.set("fill", Value::Color(Color::from_rgb8(0, 255, 0)));
    let background_id = tree.insert(background);
    tree.append_child(root_id, background_id);
    let mut dialog = widget("Dialog", 100.0, 60.0);
    dialog.set("open", Value::Bool(true));
    let dialog_id = tree.insert(dialog);
    tree.append_child(root_id, dialog_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 200.0));
    // Top-left corner is bare background under the scrim: dimmed green.
    let scrimmed = pixel(&data, stride, 5, 5);
    assert!(
        scrimmed[1] > scrimmed[0] && scrimmed[1] < 200,
        "the scrim dims the green behind it, got {scrimmed:?}"
    );
    // The dialog's own panel is the palette surface (near-neutral grey),
    // so wherever the panel sits the green is gone entirely.
    let panel = pixel(&data, stride, 100, 100);
    assert!(
        panel[1] as i32 <= panel[0] as i32 + 20 && panel[1] < 120,
        "the panel is opaque and neutral, got {panel:?}"
    );
}

#[test]
fn a_closed_dialog_leaves_the_window_untouched() {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(200.0));
    root.set("height", Value::Float(200.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut background = Element::new("Rectangle", None);
    background.set("width", Value::Float(200.0));
    background.set("height", Value::Float(200.0));
    background.set("fill", Value::Color(Color::from_rgb8(0, 255, 0)));
    let background_id = tree.insert(background);
    tree.append_child(root_id, background_id);
    let mut dialog = widget("Dialog", 100.0, 60.0);
    dialog.set("open", Value::Bool(false));
    let dialog_id = tree.insert(dialog);
    tree.append_child(root_id, dialog_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 200.0));
    let full_green = pixel(&data, stride, 100, 100);
    assert!(
        full_green[1] > 200 && full_green[0] < 60,
        "a closed dialog paints nothing over the content, got {full_green:?}"
    );
}

#[test]
fn a_dialog_outranks_a_plain_rect_declared_after_it() {
    // The hoist is what makes this true: document order alone would put
    // the red rect on top.
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(200.0));
    root.set("height", Value::Float(200.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut dialog = widget("Dialog", 200.0, 200.0);
    dialog.set("open", Value::Bool(true));
    let dialog_id = tree.insert(dialog);
    tree.append_child(root_id, dialog_id);
    let mut later = Element::new("Rectangle", None);
    later.set("width", Value::Float(200.0));
    later.set("height", Value::Float(200.0));
    later.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
    let later_id = tree.insert(later);
    tree.append_child(root_id, later_id);

    let (data, stride) = render_pixels(&mut tree, nui_core::Size::new(200.0, 200.0));
    let center = pixel(&data, stride, 100, 100);
    assert!(
        center[0] as i32 <= center[1] as i32 + 20,
        "the dialog panel covers the later red rect, got {center:?}"
    );
}
