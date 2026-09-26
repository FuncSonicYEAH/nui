//! The nui gallery: every feature the framework ships, in one window.
//!
//! # How it is put together
//!
//! Each page is a `.nui` **fragment** under `pages/` — a single top-level
//! element, not a whole document. The gallery wraps them in its own
//! window, draws a sidebar from the same `PAGES` table it reads the
//! fragments from, and drops them all into one scrolling content area.
//! Switching pages is `visible`:
//!
//! - the selected page is drawn;
//! - the others carry `visible <- root.page == <slot>`, which
//!   (`nui_runtime::widget::is_visible`) removes them from layout
//!   entirely — no box, so nothing to paint and nothing to hit. A page
//!   you are not looking at costs one binding, not a hidden element tree.
//!
//! Two conventions make the fragments composable, and both are load
//! bearing:
//!
//! 1. **A page keeps its state on its own root element.** `Counter`'s
//!    count is `counter_page.count`, not a `Gallery` property. The
//!    alternative — hoisting every page's state into the gallery
//!    component — would force a name prefix per page (`widgets_volume`,
//!    `text_email`, …) and a rename every time two pages happened to want
//!    the same word. Reading it through an element id keeps a fragment
//!    readable on its own and copy-pasteable back out into a standalone
//!    example.
//! 2. **Element ids are prefixed with the page.** They share one
//!    document, so `id = content` in two fragments is one element.
//!
//! The one thing a fragment does *not* declare is its own `visible` line:
//! the assembly in [`host`] injects it, so a page's order in
//! [`host::PAGES`] stays the single source of truth for what the sidebar
//! shows and what index selects it.
//!
//! The document text, the page table, and the host-side models and
//! behaviors live in [`host`] — a module `tests/gallery.rs` includes too,
//! by path, so the test drives the very same gallery rather than a
//! hand-maintained copy of it.
//!
//! # Running
//!
//! ```text
//! cargo run -p nui --example gallery              # windowed
//! cargo run -p nui --example gallery -- --snap    # one PNG per page
//! ```
//!
//! The offscreen mode needs no display, which is what makes the gallery
//! checkable in CI; the layout-side checks live in `tests/gallery.rs`.
#![allow(clippy::unwrap_used)]

mod host;

fn main() {
    let mut args = std::env::args().skip(1);
    if matches!(args.next().as_deref(), Some("--snap")) {
        snapshots();
        return;
    }
    let config = nui::AppConfig::new(host::document(), "nui — gallery", host::WINDOW);
    nui::Application::new(config)
        .with_registry(host::build_registry())
        .run();
}

/// Offscreen mode: one PNG per page, into `./snapshots/`.
///
/// Needs no display and no window, so it is how the gallery is checked in
/// CI — and how a page's rendering can be diffed after a change to the
/// render layer.
fn snapshots() {
    use nui_core::Duration;

    let source = host::document();
    let registry = host::build_registry();
    let outcome = nui_compiler::compile_with_functions(&source, &registry.function_names());
    assert!(
        outcome.diagnostics.is_empty(),
        "gallery document does not compile:\n{}",
        outcome
            .diagnostics
            .iter()
            .map(|diagnostic| return nui_syntax::render_diagnostic(
                &source,
                "gallery.nui",
                diagnostic
            ))
            .collect::<Vec<_>>()
            .join("")
    );
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let mut text = nui_text::TextSystem::with_embedded_font();

    // The offscreen twin of the host's `pump_images`: decode every `Image`
    // source synchronously, so image pages draw in snapshots too.
    let mut image_keys: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut decoded_images: std::collections::HashMap<String, nui_render::DecodedImage> =
        std::collections::HashMap::new();
    tree.visit_pre_order(|_id, element| {
        if element.ty != "Image" {
            return;
        }
        let Some(source) = element
            .get("source")
            .and_then(|value| return value.as_str().ok())
        else {
            return;
        };
        if image_keys.contains_key(source) {
            return;
        }
        let Ok(bytes) = std::fs::read(source) else {
            return;
        };
        let key = nui_render::cache_key(source, nui_render::content_hash(&bytes));
        let Ok(decoded) = nui_render::decode_file(source) else {
            return;
        };
        decoded_images.insert(key.clone(), decoded);
        image_keys.insert(source.to_string(), key);
    });

    // The same pipeline order as `WindowHost::run_frame_pipeline`.
    let frame = |engine: &mut nui_runtime::Engine,
                 tree: &mut nui_runtime::ElementTree,
                 text: &mut nui_text::TextSystem| {
        let mut elapsed = std::collections::HashMap::new();
        let _ = engine.tick_timers(tree, Duration::from_millis(16.0), &mut elapsed);
        engine.tick_animations(tree, Duration::from_millis(16.0));
        let _ = engine.propagate(tree);
        let rebuilt = engine.sync_for_nodes(tree);
        if rebuilt > 0 {
            let _ = engine.propagate(tree);
        }
        let _ = engine.apply_when_blocks(tree);
        nui_layout::layout_with_text(tree, host::WINDOW, Some(text));
        let _ = engine.take_changes();
    };
    frame(&mut engine, &mut tree, &mut text);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("offscreen adapter (lavapipe/WARP/Metal)");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nui-gallery-snap"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("snapshot device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = nui_render::Renderer::new(&device, format);

    std::fs::create_dir_all("snapshots").expect("snapshots dir");
    let page_id = tree.lookup_id("root").expect("the gallery has a root");
    let width = host::WINDOW.width as u32;
    let height = host::WINDOW.height as u32;
    let bytes_per_row = (width * 4).next_multiple_of(256);

    for (slot, page) in host::PAGES.iter().enumerate() {
        engine.set_direct(
            &mut tree,
            page_id,
            "page",
            nui_core::Value::Int(slot as i64),
        );
        frame(&mut engine, &mut tree, &mut text);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nui-gallery-target"),
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
        let scene = nui_render::SceneBuilder::build_with_context(
            &tree,
            &mut text,
            nui_render::SceneContext {
                focused: engine.focused(),
                image_keys: &image_keys,
            },
        );
        renderer.render_to_view(
            &device,
            &queue,
            &view,
            host::WINDOW,
            1.0,
            &scene,
            Some(&mut text),
            &decoded_images,
            nui::host::CLEAR_COLOR,
        );

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-gallery-readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nui-gallery-encoder"),
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
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for row in 0..height as usize {
            let source = row * bytes_per_row as usize;
            let destination = row * width as usize * 4;
            rgba[destination..destination + width as usize * 4]
                .copy_from_slice(&mapped[source..source + width as usize * 4]);
        }
        drop(mapped);
        readback.unmap();
        let path = format!("snapshots/{slot:02}-{}.png", page.key);
        image::save_buffer(&path, &rgba, width, height, image::ColorType::Rgba8)
            .expect("png write works");
        eprintln!("snapshots: wrote {path}");
    }
}
