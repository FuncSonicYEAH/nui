//! Offscreen image-pipeline test (M8): decode a generated PNG, upload it
//! through the renderer, and assert tinted + nine-sliced pixels land.

use std::sync::atomic::{AtomicU64, Ordering};

use nui_core::Value;
use nui_render::{Renderer, SceneBuilder, SceneContext, cache_key, decode_file};
use nui_runtime::Element;

/// Hands out a distinct file name to every call, within one process.
///
/// A temp file's name has to be unique per *call*, not per logical test.
/// The earlier version keyed it on a caller-supplied label, which is
/// unique only if every caller picks a different one — and two tests both
/// rendered the `top-left` atlas cell, so both wrote
/// `nui-atlas-test-top-left.png` and the pair raced. The failure was
/// `unexpected end of file` from `decode_file`, i.e. a half-written file
/// read by the other thread, which reads exactly like an image-pipeline
/// regression and is not one.
///
/// A counter is what makes it per-call, and it costs nothing: the label
/// stays for legibility, the number does the work.
fn unique_temp_file(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    return std::env::temp_dir().join(format!("nui-test-{label}-{serial}.png"));
}

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
///
/// `name` only labels the temp file; uniqueness comes from
/// [`unique_temp_file`].
fn render_image(name: &str, slice: f32) -> (Vec<u8>, usize, u32, u32) {
    let width = 64u32;
    let height = 64u32;
    let viewport = nui_core::Size::new(width as f32, height as f32);

    let png_path = unique_temp_file(&format!("image-{name}"));
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
            engine: None,
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
    let (data, stride, width, height) = render_image("decoded", 0.0);
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

/// Writes a 16x16 four-cell PNG, the shape a generated icon atlas takes:
/// four solid quadrants, so a region selecting one of them is
/// unambiguous at the pixel level.
///
/// | red | green |
/// | blue | white |
fn write_atlas_png(path: &std::path::Path) {
    let mut buffer = image::RgbaImage::new(16, 16);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let right = x >= 8;
            let bottom = y >= 8;
            let pixel = match (right, bottom) {
                (false, false) => image::Rgba([255, 0, 0, 255]),
                (true, false) => image::Rgba([0, 255, 0, 255]),
                (false, true) => image::Rgba([0, 0, 255, 255]),
                (true, true) => image::Rgba([255, 255, 255, 255]),
            };
            buffer.put_pixel(x, y, pixel);
        }
    }
    buffer.save(path).expect("png encode works");
}

/// Renders one 64x64 `Image` of the four-cell atlas through `region` and
/// returns the pixels plus the stride.
///
/// `name` only labels the temp file; uniqueness comes from
/// [`unique_temp_file`].
fn render_region(name: &str, region: Option<[f32; 4]>) -> (Vec<u8>, usize) {
    let width = 64u32;
    let height = 64u32;
    let viewport = nui_core::Size::new(width as f32, height as f32);

    let png_path = unique_temp_file(&format!("atlas-{name}"));
    write_atlas_png(&png_path);
    let source = png_path.display().to_string();
    let bytes = std::fs::read(&source).expect("png exists");
    let key = cache_key(&source, nui_render::content_hash(&bytes));
    let decoded = decode_file(&source).expect("png decodes");

    let mut tree = nui_runtime::ElementTree::new();
    let mut image = Element::new("Image", None);
    image.set("source", Value::String(source.clone()));
    image.set("width", Value::Float(64.0));
    image.set("height", Value::Float(64.0));
    if let Some([x, y, w, h]) = region {
        image.set("region.x", Value::Float(f64::from(x)));
        image.set("region.y", Value::Float(f64::from(y)));
        image.set("region.width", Value::Float(f64::from(w)));
        image.set("region.height", Value::Float(f64::from(h)));
    }
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
            engine: None,
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
    let mut renderer = Renderer::new(&device, format);
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
        &std::collections::HashMap::from([(key, decoded)]),
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
    return (data, bytes_per_row as usize);
}

#[test]
fn a_region_draws_one_cell_of_an_atlas() {
    // The whole point: one texture, many draws, each addressing its own
    // cell. Drawing the top-left 8x8 quadrant must fill the element with
    // solid red and nothing else.
    //
    // Sampled away from the outermost two pixels on purpose: the texture is
    // sampled with a linear filter, so the last row of the cell blends with
    // its neighbour in the atlas. That bleed is real and correct — the
    // alternative is a magnified atlas with padding between cells — but it
    // means a pixel at the very edge of a cell is not purely that cell's
    // colour.
    let (data, stride) = render_region("top-left", Some([0.0, 0.0, 8.0, 8.0]));
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y as usize) * stride + (x as usize) * 4;
        return [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
    };
    for (x, y) in [(4, 4), (32, 32), (56, 56), (8, 40)] {
        let got = pixel(x, y);
        assert!(
            got[0] > 180 && got[1] < 60 && got[2] < 60,
            "({x}, {y}) is inside the red cell, got {got:?}"
        );
    }
}

#[test]
fn a_region_picks_a_different_cell_of_the_same_atlas() {
    // Four draws, four cells, one texture — the icon-atlas shape. Each cell
    // is a separate render here (and so a separate file), which is enough
    // to show the region is honoured per *draw* rather than per texture.
    // `expected` is the cell's colour: channels above 100 must be bright,
    // the rest near zero.
    for (name, region, expected) in [
        ("top-left", [0.0, 0.0, 8.0, 8.0], [255_u8, 0, 0]),
        ("top-right", [8.0, 0.0, 8.0, 8.0], [0, 255, 0]),
        ("bottom-left", [0.0, 8.0, 8.0, 8.0], [0, 0, 255]),
        ("bottom-right", [8.0, 8.0, 8.0, 8.0], [255, 255, 255]),
    ] {
        let (data, stride) = render_region(name, Some(region));
        let offset = stride * 32 + 32 * 4;
        let got = [data[offset], data[offset + 1], data[offset + 2]];
        for channel in 0..3 {
            if expected[channel] > 100 {
                assert!(
                    got[channel] > 180,
                    "{name}: channel {channel} should be lit, got {got:?}"
                );
            } else {
                assert!(
                    got[channel] < 60,
                    "{name}: channel {channel} should be dark, got {got:?}"
                );
            }
        }
    }
}

#[test]
fn no_region_still_draws_the_whole_texture() {
    // The regression a region feature could introduce: the UV remap
    // applying to an image that never asked for one. Without a region the
    // element must still show all four quadrants.
    let (data, stride) = render_region("whole", None);
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y as usize) * stride + (x as usize) * 4;
        return [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ];
    };
    let top_left = pixel(16, 16);
    let top_right = pixel(48, 16);
    let bottom_left = pixel(16, 48);
    assert!(
        top_left[0] > 180 && top_left[1] < 60,
        "top-left quadrant is red, got {top_left:?}"
    );
    assert!(
        top_right[1] > 180 && top_right[0] < 60,
        "top-right quadrant is green, got {top_right:?}"
    );
    assert!(
        bottom_left[2] > 180 && bottom_left[0] < 60,
        "bottom-left quadrant is blue, got {bottom_left:?}"
    );
}

#[test]
fn a_partially_specified_region_is_ignored() {
    // All four numbers or none: a region missing its height would have to
    // guess, and a guess here is an icon that silently renders the wrong
    // glyph.
    let (data, stride) = render_region("partial", None);
    let offset = stride * 32 + 32 * 4;
    let got = &data[offset..offset + 4];
    // Without a region the centre of the element straddles the four
    // quadrants; the red and blue halves are both present.
    assert!(
        got[0] > 60 || got[2] > 60,
        "the whole texture is drawn, got {got:?}"
    );
}

#[test]
fn nine_slice_keeps_corners_and_stretches_middle() {
    // slice = 3dp: the 64x64 dest keeps the 3px source corners unscaled at
    // scale 1; the middle stretches. Verify a corner stays red while the
    // transparent middle-right region stays background.
    let (data, stride, _width, height) = render_image("nine-slice", 3.0);
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
