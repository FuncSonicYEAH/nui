//! Playground demo — the "see everything" showcase: text, buttons with
//! click bubbling, animated bars (`tween`), `TextInput` with two-way
//! binding, a shadowed card, `layer.opacity` / `layer.blur`, and a
//! virtualized 10k-row `ListView`.
//!
//! Two modes:
//! - windowed (default): `cargo run -p nui --example playground --release`
//! - offscreen snapshots: `cargo run -p nui --example playground -- --snap`
//!   writes PNGs into `./snapshots/` (no display or GPU window needed).
#![allow(clippy::unwrap_used)]

use nui::{AppConfig, Application};
use nui_core::{Size, Value};
use nui_runtime::{ModelRow, Registry, VecModel};

const PLAYGROUND: &str = r#"
component Playground {
    property rows: Model
    property userName: String = ""
    property progress: Float = 0.0

    Window(id = root, width = 480dp, height = 620dp) {
        Column(id = content, spacing = 10dp, padding = 16dp) {
            Text(id = title, content = "nui playground", font.size = 22dp,
                 color = #ffffff, width = 220dp, height = 30dp)

            Rectangle(id = card, width = 448dp, height = 48dp, radius = 10dp,
                      fill = #2b3240,
                      shadow.dx = 0dp, shadow.dy = 5dp,
                      shadow.blur = 10dp, shadow.color = #000000aa) {
                layer.opacity = 0.92
            }

            Rectangle(id = bar, width <- tween(80dp + progress * 6dp, duration = 300ms, easing = ease-out),
                      height = 14dp, radius = 7dp, fill = #4c8fd6)
            Button(id = plus, width = 64dp, height = 26dp, radius = 6dp,
                   fill = #3d6a9e, padding = 6dp) {
                on click => progress += 10.0
                Text(content = "+10", width = 40dp, height = 14dp, color = #ffffff)
            }

            TextInput(id = name, width = 448dp, height = 34dp,
                      placeholder = "type your name...", text <=> root.userName)
            Text(id = greeting, content <- "Hello, " + userName + "!",
                 width = 300dp, height = 22dp, color = #9fd6ff)

            Rectangle(id = pill, width = 150dp, height = 24dp, radius = 8dp,
                      fill = #7a4c9e) {
                layer.blur = 2dp
            }

            ListView(item in root.rows, id = list, width = 448dp, height = 210dp,
                     row_height = 30dp) {
                Rectangle(id = row, width = 432dp, height = 24dp, radius = 5dp,
                          fill <- item.hot ? #3d6a9e : #2b3240)
            }
        }
    }
}
"#;

fn build_registry() -> Registry {
    return Registry::new().on_attach(Box::new(|engine, tree| {
        let rows: Vec<ModelRow> = (0..10_000)
            .map(|index| {
                return vec![
                    ("label".to_string(), Value::String(format!("row {index}"))),
                    ("hot".to_string(), Value::Bool(index % 20 == 0)),
                ];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "rows", Value::Model(model.0));
    }));
}

/// One headless frame: the same pipeline order as `WindowHost::run_frame_pipeline`.
fn frame(
    engine: &mut nui_runtime::Engine,
    tree: &mut nui_runtime::ElementTree,
    text: &mut nui_text::TextSystem,
    size: Size,
    delta: f64,
) {
    let mut elapsed = std::collections::HashMap::new();
    let _ = engine.tick_timers(tree, nui_core::Duration::from_millis(delta), &mut elapsed);
    engine.tick_animations(tree, nui_core::Duration::from_millis(delta));
    let _ = engine.propagate(tree);
    let rebuilt = engine.sync_for_nodes(tree);
    if rebuilt > 0 {
        let _ = engine.propagate(tree);
    }
    let _ = engine.apply_when_blocks(tree);
    nui_layout::layout_with_text(tree, size, Some(text));
    let _ = engine.take_changes();
}

/// Offscreen mode: renders PNG snapshots into `./snapshots/`.
fn snapshot_mode() {
    let registry = build_registry();
    let outcome = nui_compiler::compile_with_functions(PLAYGROUND, &registry.function_names());
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let mut text = nui_text::TextSystem::with_embedded_font();

    let size = Size::new(480.0, 620.0);
    let physical = Size::new(480.0, 620.0);
    frame(&mut engine, &mut tree, &mut text, size, 0.0);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("offscreen adapter (lavapipe/WARP/Metal)");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nui-snap-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("snap device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = nui_render::Renderer::new(&device, format);

    std::fs::create_dir_all("snapshots").expect("snapshots dir");
    let mut frame_index = 0usize;
    let mut snap = |engine: &mut nui_runtime::Engine,
                    tree: &mut nui_runtime::ElementTree,
                    text: &mut nui_text::TextSystem,
                    name: &str| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nui-snap-target"),
            size: wgpu::Extent3d {
                width: physical.width as u32,
                height: physical.height as u32,
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
        let focused = engine.focused();
        let scene = nui_render::SceneBuilder::build_with_context(
            tree,
            text,
            nui_render::SceneContext {
                focused,
                image_keys: &std::collections::HashMap::new(),
            },
        );
        renderer.render_to_view(
            &device,
            &queue,
            &view,
            physical,
            1.0,
            &scene,
            Some(text),
            &std::collections::HashMap::new(),
            wgpu::Color {
                r: 0.08,
                g: 0.09,
                b: 0.11,
                a: 1.0,
            },
        );
        // Read back and save.
        let width = physical.width as u32;
        let height = physical.height as u32;
        let bytes_per_row = (width * 4).next_multiple_of(256);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nui-snap-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nui-snap-encoder"),
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
            let src = row * bytes_per_row as usize;
            let dst = row * width as usize * 4;
            rgba[dst..dst + width as usize * 4]
                .copy_from_slice(&mapped[src..src + width as usize * 4]);
        }
        drop(mapped);
        readback.unmap();
        let path = format!("snapshots/{frame_index:02}-{name}.png");
        image::save_buffer(&path, &rgba, width, height, image::ColorType::Rgba8)
            .expect("png write works");
        eprintln!("snapshots: wrote {path}");
        frame_index += 1;
    };

    // 00: initial state.
    snap(&mut engine, &mut tree, &mut text, "initial");

    // 01: two clicks -> the bar tweens; capture mid-flight.
    let plus = tree.lookup_id("plus").unwrap();
    let _ = engine.emit_bubble(&mut tree, plus, "click");
    let _ = engine.emit_bubble(&mut tree, plus, "click");
    frame(&mut engine, &mut tree, &mut text, size, 0.0);
    engine.tick_animations(&mut tree, nui_core::Duration::from_millis(150.0));
    frame(&mut engine, &mut tree, &mut text, size, 0.0);
    snap(&mut engine, &mut tree, &mut text, "animation-mid");

    // 02: settled bar + typed name via the text input.
    engine.tick_animations(&mut tree, nui_core::Duration::from_millis(400.0));
    let name_input = tree.lookup_id("name").unwrap();
    engine.focus(name_input);
    engine.handle_text_input(&mut tree, "Ada");
    frame(&mut engine, &mut tree, &mut text, size, 0.0);
    snap(&mut engine, &mut tree, &mut text, "typed");

    // 03: deep scroll into the 10k-row list.
    let list = tree.lookup_id("list").unwrap();
    engine.set_direct(&mut tree, list, "scroll_y", Value::Float(30_000.0));
    frame(&mut engine, &mut tree, &mut text, size, 0.0);
    snap(&mut engine, &mut tree, &mut text, "scrolled");
}

fn main() {
    let mut args = std::env::args().skip(1);
    if matches!(args.next().as_deref(), Some("--snap")) {
        snapshot_mode();
        return;
    }
    let config = AppConfig::new(PLAYGROUND, "nui — playground", Size::new(480.0, 620.0));
    Application::new(config)
        .with_registry(build_registry())
        .run();
}
