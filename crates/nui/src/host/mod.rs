//! WindowHost: per-window GPU state and the frame pipeline. Owns the winit
//! window, wgpu surface, engine, and layout adapter for one document.

use nui_compiler::DocumentIr;
use nui_core::{Event, Key, Size};
use nui_runtime::element::ElementId;
use nui_runtime::{ElementTree, Engine};
use winit::event::WindowEvent;
use winit::window::Window;

use crate::app::hit_test;

mod interaction;

use interaction::PointerGesture;

use nui_render::{Renderer, SceneBuilder};

/// The default window clear color, `#14171c` in **linear** RGB —
/// `wgpu::Color` is linear for sRGB targets, so the sRGB values
/// (0.08/0.09/0.11) must be converted first. Share this instead of
/// re-deriving it (a raw sRGB value renders as a darker seam).
pub const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.0069,
    g: 0.0082,
    b: 0.0117,
    a: 1.0,
};

/// The element hit by a pointer event (opaque handle for dispatch).
#[derive(Debug, Clone, Copy)]
pub struct HitTarget {
    /// Hit element.
    pub element: ElementId,
}

/// Per-window state: gpu + engine + layout bookkeeping.
pub struct WindowHost {
    window: std::sync::Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    pub tree: ElementTree,
    pub engine: Engine,
    /// Logical size (dp).
    size: Size,
    /// dpi scale factor.
    scale: f32,
    /// Whether a frame must be drawn (dirty data or active animation).
    needs_redraw: bool,
    /// Surface configuration (reconfigured on resize).
    surface_format: wgpu::TextureFormat,
    click: PointerGesture,
    /// Widget interaction state (hover/press/arm/focus, pointer capture).
    widgets: nui_runtime::WidgetStates,
    /// Timer elapsed accumulators.
    timer_elapsed: std::collections::HashMap<ElementId, f64>,
    /// Last frame timestamp, for the animation/timer frame delta.
    last_frame: Option<std::time::Instant>,
    /// Text shaping + glyph atlas (M6 text pipeline).
    text: nui_text::TextSystem,
    /// Failed-reload message drawn over the last good frame (M6 overlay).
    error_overlay: Option<String>,
    /// System clipboard (lazy; unavailable in headless environments).
    clipboard: Option<arboard::Clipboard>,
    /// Image path -> texture cache key ("path:content-hash"; empty on
    /// failed loads so we don't retry every frame).
    image_keys: std::collections::HashMap<String, String>,
    /// Decoded images by cache key, awaiting GPU upload.
    image_store: std::collections::HashMap<String, nui_render::DecodedImage>,
    /// Decode jobs in flight, keyed by path.
    image_inflight: std::collections::HashSet<String>,
    /// Sender for decode worker threads.
    image_tx: std::sync::mpsc::Sender<(
        String,
        String,
        Result<nui_render::DecodedImage, nui_render::ImageLoadError>,
    )>,
    /// Results from decode worker threads.
    image_results: std::sync::mpsc::Receiver<(
        String,
        String,
        Result<nui_render::DecodedImage, nui_render::ImageLoadError>,
    )>,
    /// Last seen cursor position (dp), for press/release events.
    cursor: nui_core::Point,
    /// Persistent winit -> nui event translator (keeps modifiers + cursor
    /// state across events; a per-event translator loses both).
    translator: nui_winit::EventTranslator,
    /// Whether the host saw a close request.
    close_requested: bool,
}

impl WindowHost {
    /// Creates the host: gpu init, document compile, tree instantiate. The
    /// registry provides host functions and custom components (plan §5
    /// 宿主互操作); its function names participate in compile-time checks.
    pub async fn new(
        window: Window,
        source: &str,
        registry: nui_runtime::Registry,
        display: winit::event_loop::OwnedDisplayHandle,
    ) -> Result<WindowHost, String> {
        let (image_tx, image_rx) = std::sync::mpsc::channel();
        let outcome = nui_compiler::compile_with_functions(source, &registry.function_names());
        if !outcome.diagnostics.is_empty() {
            let first = &outcome.diagnostics[0];
            return Err(format!("compile error: {}", first.message));
        }
        let document: DocumentIr = outcome.document;
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let instance = nui_runtime::instantiate_with(&document, registry);
        let tree = instance.tree;
        let engine = instance.engine;

        let window = std::sync::Arc::new(window);
        // IME (fcitx5/ibus/XIM): composition events only flow when allowed.
        window.set_ime_allowed(true);
        let (_instance, surface, adapter) = init_gpu(&window, display)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("nui-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("device: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| return format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, surface_format);

        let mut host = WindowHost {
            window,
            surface,
            device,
            queue,
            renderer,
            tree,
            engine,
            size: Size::new(size.width as f32 / scale, size.height as f32 / scale),
            scale,
            needs_redraw: true,
            surface_format,
            click: PointerGesture::default(),
            widgets: nui_runtime::WidgetStates::new(),
            timer_elapsed: std::collections::HashMap::new(),
            last_frame: None,
            text: nui_text::TextSystem::with_system_fonts(),
            error_overlay: None,
            clipboard: None,
            image_keys: std::collections::HashMap::new(),
            image_store: std::collections::HashMap::new(),
            image_inflight: std::collections::HashSet::new(),
            image_tx,
            image_results: image_rx,
            cursor: nui_core::Point::ZERO,
            close_requested: false,
            translator: nui_winit::EventTranslator::new(scale),
        };
        host.engine.run_registry_init(&mut host.tree);
        host.run_frame_pipeline(nui_core::Duration::ZERO);
        return Ok(host);
    }

    /// The winit window.
    pub fn window(&self) -> &Window {
        return &self.window;
    }

    /// Sets or clears the on-screen error overlay (drawn over the last
    /// good frame; v1 wraps at nothing, capped at 24 lines).
    pub fn set_error_overlay(&mut self, message: Option<String>) {
        self.error_overlay = message;
    }

    /// Hot reload (plan §8): recompiles `source` and rebuilds the tree and
    /// engine (v1 full rebuild — element state is lost; the registry's
    /// attach hook re-registers models). On a compile error the host keeps
    /// the previous document untouched and returns the rendered
    /// diagnostics.
    pub fn reload(&mut self, source: &str) -> Result<(), String> {
        let mut instance = nui_runtime::Instance {
            tree: std::mem::take(&mut self.tree),
            engine: std::mem::take(&mut self.engine),
        };
        let outcome = nui_runtime::reload_from_source(&mut instance, source);
        if outcome.is_ok() {
            self.error_overlay = None;
            self.tree = instance.tree;
            self.engine = instance.engine;
            self.timer_elapsed.clear();
            // Handles are generational: the old capture refers to a dead
            // tree, and latched widget state would point at stale ids.
            self.click = PointerGesture::default();
            self.widgets.reset();
            self.needs_redraw = true;
        }
        return outcome;
    }

    /// Whether the window should close.
    pub fn should_close(&self) -> bool {
        return self.close_requested;
    }

    /// Whether a redraw is pending (dirty data or running animation).
    pub fn needs_redraw(&self) -> bool {
        return self.needs_redraw;
    }

    /// Whether dirty bindings, pending model syncs, animations, timers, or
    /// a redraw remain (frame scheduling input, plan §2 on-demand redraw).
    /// Running timers must keep the loop polling: once the loop falls back
    /// to `Wait` no frames run, so no timer would ever fire again.
    pub fn has_pending_work(&self) -> bool {
        return self.engine.has_dirty_bindings()
            || self.engine.has_pending_model_sync()
            || self.engine.has_active_animations()
            || self.has_running_timers()
            || self.needs_redraw;
    }

    /// Whether any `Timer` node is currently running (its interval is set,
    /// via a declared `running` property or a `timer.start()` call).
    fn has_running_timers(&self) -> bool {
        let mut found = false;
        self.tree.visit_pre_order(|_id, element| {
            if element.timer_interval.is_some() {
                found = true;
            }
        });
        return found;
    }

    /// Handles a normalized winit event; returns `true` when consumed as a
    /// redraw trigger (the caller then requests a redraw).
    pub fn handle_event(&mut self, event: WindowEvent) -> bool {
        let window_size = Size::new(self.size.width * self.scale, self.size.height * self.scale);
        let events = self.translator.translate(event, window_size);
        self.scale = self.translator.scale_factor();
        let mut any_action = false;
        for event in events {
            if self.dispatch_event(event) {
                any_action = true;
            }
        }
        return any_action;
    }

    /// Normalized event dispatch: hit testing + signal emission + engine
    /// propagation. Returns whether anything became dirty.
    ///
    /// The pointer arms, the modal gates and Space/Enter activation are
    /// one-line delegations to `interaction` — see that module for what
    /// each phase does and why the gates come first.
    pub fn dispatch_event(&mut self, event: Event) -> bool {
        return match event {
            Event::PointerMoved { position } => self.on_pointer_moved(position),
            Event::PointerPressed {
                button, position, ..
            } => self.on_pointer_pressed(button, position),
            Event::KeyPressed { key, modifiers } => {
                // Modal gate (批次 4): Escape dismisses the top dialog and
                // every other key is swallowed unless focus is inside it.
                if let Some(blocked) = self.modal_key_gate(key) {
                    return blocked;
                }
                if modifiers.ctrl {
                    match key {
                        Key::Character('c') | Key::Character('C') => {
                            if let Some(text) = self.engine.copy_focused(&self.tree) {
                                self.clipboard_copy(&text);
                            }
                            return false;
                        }
                        Key::Character('x') | Key::Character('X') => {
                            if let Some(text) = self.engine.cut_focused(&mut self.tree) {
                                self.clipboard_copy(&text);
                                self.run_frame_pipeline(nui_core::Duration::ZERO);
                                return true;
                            }
                            return false;
                        }
                        Key::Character('v') | Key::Character('V') => {
                            if let Some(text) = self.clipboard_paste() {
                                let handled = self.engine.handle_text_input(&mut self.tree, &text);
                                if handled {
                                    self.run_frame_pipeline(nui_core::Duration::ZERO);
                                }
                                return handled;
                            }
                            return false;
                        }
                        _ => {}
                    }
                }
                // Up/Down with focus (批次 6): a multi-line field walks its
                // shaped lines, a stepper takes a step. Both need the host —
                // a real font for the line table, a direction — so they are
                // asked before the generic key handling.
                if matches!(key, Key::ArrowUp | Key::ArrowDown)
                    && self.move_vertical(key == Key::ArrowUp, modifiers.shift)
                {
                    return true;
                }
                // Widget activation (批次 0).
                if self.activate_focused(key) {
                    return true;
                }
                let handled = self.engine.handle_key(&mut self.tree, key, modifiers);
                if handled {
                    self.run_frame_pipeline(nui_core::Duration::ZERO);
                }
                return handled;
            }
            Event::WheelScrolled { position, delta } => {
                // A modal dialog freezes scrolling outside its subtree.
                if self.modal_covers(position) {
                    return false;
                }
                // Route to the nearest `Scroll` ancestor of the hit element
                // (M9 scrolling): wheel down moves content up.
                let dy = match delta {
                    nui_core::WheelDelta::Lines { y, .. } => y * 40.0,
                    nui_core::WheelDelta::Pixels { y, .. } => y,
                };
                // A stepper eats the wheel before a scroll container sees it
                // (批次 6): one notch is one step, and scrolling *down* steps
                // *down* the range, as every spinner does. The whole control
                // is the stepper, not just its arrow strip. Pixels convert at
                // the same 40 dp a line is worth, so a trackpad flick too
                // small to be a notch steps nothing.
                let hit = hit_test(&self.tree, position).map(|hit| return hit.element);
                if let Some(id) = hit
                    && self.tree.arena[id].ty == "SpinBox"
                {
                    let notches = match delta {
                        nui_core::WheelDelta::Lines { y, .. } => f64::from(y.round()),
                        nui_core::WheelDelta::Pixels { y, .. } => f64::from((y / 40.0).round()),
                    };
                    if notches == 0.0 {
                        return false;
                    }
                    let _ = self.step_spin(id, -notches);
                    return true;
                }
                let mut current = hit;
                while let Some(id) = current {
                    if self.tree.arena[id].ty == "Scroll" || self.tree.arena[id].ty == "ListView" {
                        let scroll_y = match self.tree.arena[id].get("scroll_y") {
                            Some(nui_core::Value::Float(value)) => *value as f32,
                            _ => 0.0,
                        };
                        // Clamp at both ends. The upper bound is the one
                        // that matters: without it the wheel scrolls the
                        // content past its own bottom edge and leaves a
                        // blank strip under the last row. The limit is a
                        // judgement (content height vs viewport height), so
                        // it lives in `nui-runtime` where a test can reach
                        // it — the host has none.
                        let limit = nui_runtime::widget::max_scroll_y(&self.engine, &self.tree, id);
                        let next = (scroll_y + dy).clamp(0.0, limit);
                        self.engine.set_direct(
                            &mut self.tree,
                            id,
                            "scroll_y",
                            nui_core::Value::Float(next as f64),
                        );
                        self.run_frame_pipeline(nui_core::Duration::ZERO);
                        return true;
                    }
                    current = self.tree.arena[id].parent;
                }
                return false;
            }
            Event::TextInput { text } => {
                let handled = self.engine.handle_text_input(&mut self.tree, &text);
                if handled {
                    self.run_frame_pipeline(nui_core::Duration::ZERO);
                }
                return handled;
            }
            Event::ImePreedit { text } => {
                let focused = self.engine.focused().is_some_and(|id| {
                    return self.tree.arena[id].is_text_input();
                });
                if focused {
                    let _ = self.engine.set_preedit(&mut self.tree, &text);
                    self.run_frame_pipeline(nui_core::Duration::ZERO);
                }
                return focused;
            }
            Event::PointerReleased {
                button, position, ..
            } => self.on_pointer_released(button, position),
            Event::WindowResized { size } => {
                self.resize(size);
                true
            }
            Event::CloseRequested => {
                self.close_requested = true;
                false
            }
            _ => false,
        };
    }

    /// Scans `Image` elements for sources, dispatching off-thread decode
    /// jobs (plan §6.3 图片后台解码) and collecting finished ones.
    fn pump_images(&mut self) {
        let mut paths: Vec<String> = Vec::new();
        self.tree.visit_pre_order(|_id, element| {
            if element.ty == "Image"
                && let Some(source) = element
                    .get("source")
                    .and_then(|value| return value.as_str().ok())
            {
                paths.push(source.to_string());
            }
        });
        for path in paths {
            if self.image_keys.contains_key(&path) || self.image_inflight.contains(&path) {
                continue;
            }
            self.image_inflight.insert(path.clone());
            let sender = self.image_tx.clone();
            std::thread::spawn(move || {
                let bytes = std::fs::read(&path);
                let key = match &bytes {
                    Ok(bytes) => nui_render::cache_key(&path, nui_render::content_hash(bytes)),
                    Err(error) => {
                        let _ = sender.send((
                            path.clone(),
                            String::new(),
                            Err(nui_render::ImageLoadError {
                                message: format!("read failed: {error}"),
                            }),
                        ));
                        return;
                    }
                };
                let decoded = nui_render::decode_file(&path);
                let _ = sender.send((path, key, decoded));
            });
        }
        let mut dirty = false;
        while let Ok((path, key, result)) = self.image_results.try_recv() {
            self.image_inflight.remove(&path);
            match result {
                Ok(decoded) => {
                    self.image_store.insert(key.clone(), decoded);
                    dirty = true;
                }
                Err(error) => {
                    eprintln!("nui: image `{path}` failed: {error}");
                }
            }
            self.image_keys.insert(path, key);
        }
        if dirty {
            self.needs_redraw = true;
        }
    }

    /// The lazy clipboard (creation can fail without a display server).
    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        return self.clipboard.as_mut();
    }

    fn clipboard_copy(&mut self, text: &str) {
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text.to_string());
        }
    }

    fn clipboard_paste(&mut self) -> Option<String> {
        return self.clipboard()?.get_text().ok();
    }

    /// The full frame pipeline (plan §2): timers + animations tick ->
    /// bindings propagate -> `For` row sync -> when blocks -> layout ->
    /// render. `frame_delta` drives timers and animations; `Duration::ZERO`
    /// just settles state.
    pub fn run_frame_pipeline(&mut self, frame_delta: nui_core::Duration) {
        self.pump_images();
        let _ = self
            .engine
            .tick_timers(&mut self.tree, frame_delta, &mut self.timer_elapsed);
        self.engine.tick_animations(&mut self.tree, frame_delta);
        let _ = self.engine.propagate(&mut self.tree);
        // Model-driven `For` rows reconcile after propagation; freshly
        // instantiated row bindings fill on the second pass.
        let rebuilt = self.engine.sync_for_nodes(&mut self.tree);
        if rebuilt > 0 {
            let _ = self.engine.propagate(&mut self.tree);
        }
        let _ = self.engine.apply_when_blocks(&mut self.tree);
        nui_layout::layout_with_text(&mut self.tree, self.size, Some(&mut self.text));
        // Widget state mirrors focus and layout into element properties
        // (批次 0). Run after layout so the first pass sees real boxes;
        // `set_direct` only dirties the tree when a value actually moved.
        let input = nui_runtime::PointerInput {
            position: self.cursor,
            inside: true,
            down: self.click.captured.is_some(),
        };
        let _ = self.widgets.update(&mut self.engine, &mut self.tree, input);
        self.needs_redraw = true;
        // Drain change notifications (observers hook here).
        let _ = self.engine.take_changes();
    }

    /// One rendered frame: advances the frame clock, runs the pipeline, and
    /// submits a frame. Called from the run loop on `RedrawRequested`.
    pub fn render_frame(&mut self) {
        let delta = self.frame_delta();
        self.run_frame_pipeline(delta);
        self.draw();
    }

    /// Time since the previous rendered frame (real wall-clock delta for
    /// timers and animations; the first frame reports zero).
    fn frame_delta(&mut self) -> nui_core::Duration {
        let now = std::time::Instant::now();
        let delta = match self.last_frame {
            Some(previous) => now.duration_since(previous).as_millis() as f64,
            None => 0.0,
        };
        self.last_frame = Some(now);
        return nui_core::Duration::from_millis(delta);
    }

    /// Resizes the surface and re-lays-out.
    pub fn resize(&mut self, logical: Size) {
        self.size = logical;
        let physical = winit::dpi::PhysicalSize::new(
            (logical.width * self.scale).max(1.0) as u32,
            (logical.height * self.scale).max(1.0) as u32,
        );
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: physical.width,
                height: physical.height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
        nui_layout::layout(&mut self.tree, logical);
        self.needs_redraw = true;
    }

    /// Draws one frame: scene build (rects + glyphs + layers) + error
    /// overlay + wgpu submit.
    pub fn draw(&mut self) {
        let focused = self.engine.focused();
        let mut scene = SceneBuilder::build_with_context(
            &self.tree,
            &mut self.text,
            nui_render::SceneContext {
                focused,
                image_keys: &self.image_keys,
            },
        );
        if let Some(message) = self.error_overlay.clone() {
            overlay_text(&mut scene, &mut self.text, &message);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Usable, but reconfigure for optimal presentation.
                self.resize(self.size);
                frame
            }
            // Outdated/Lost need reconfigure; Timeout/Occluded skip the frame.
            _ => {
                self.resize(self.size);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let physical = Size::new(self.size.width * self.scale, self.size.height * self.scale);
        self.renderer.render_to_view(
            &self.device,
            &self.queue,
            &view,
            physical,
            self.scale,
            &scene,
            Some(&mut self.text),
            &self.image_store,
            CLEAR_COLOR,
        );
        // wgpu 30 presents through the queue (the texture drops as presented).
        self.queue.present(frame);
        self.needs_redraw = false;
    }
}

/// Appends the error overlay's glyph quads to the scene (red text, top-left,
/// capped so runaway diagnostics stay readable).
fn overlay_text(scene: &mut nui_render::Scene, text: &mut nui_text::TextSystem, message: &str) {
    const FONT_SIZE: f32 = 13.0;
    const MARGIN: f32 = 10.0;
    const MAX_LINES: usize = 24;
    const LINE_SPACING: f32 = 1.25;
    let red = nui_core::Color::from_rgb8(255, 99, 99);
    let mut y = MARGIN;
    for line in message.lines().take(MAX_LINES) {
        let shaped = text.shape(line, FONT_SIZE);
        for glyph in &shaped.glyphs {
            let Some(quad) = text.glyph_quad(&glyph.key) else {
                continue;
            };
            scene.texts.push(nui_render::TextDraw {
                origin: nui_core::Point::new(
                    MARGIN + glyph.x + quad.left as f32,
                    y + glyph.y + quad.top as f32,
                ),
                size: nui_core::Size::new(quad.slot.width as f32, quad.slot.height as f32),
                mask: (quad.slot.x, quad.slot.y, quad.slot.width, quad.slot.height),
                page: quad.slot.page,
                color: red,
                clip: None,
            });
        }
        y += shaped.height.max(FONT_SIZE * LINE_SPACING);
    }
}

/// GPU initialization with a backend fallback chain (M11 渲染 fallback):
/// the default backends (Vulkan / DX12 / Metal) are tried first; when they
/// all fail — e.g. old Intel GPUs with broken Vulkan drivers — GL is retried
/// with the winit display handle, which GLES requires to present. Set
/// `NUI_BACKEND=vulkan|gl|dx12|metal|primary` to force one attempt.
fn init_gpu(
    window: &std::sync::Arc<Window>,
    display: winit::event_loop::OwnedDisplayHandle,
) -> Result<(wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter), String> {
    let mut attempts: Vec<(&'static str, wgpu::Backends, bool)> = Vec::new();
    match std::env::var("NUI_BACKEND").as_deref() {
        Ok("gl") => attempts.push(("GL (forced)", wgpu::Backends::GL, true)),
        Ok("vulkan") => attempts.push(("Vulkan (forced)", wgpu::Backends::VULKAN, false)),
        Ok("dx12") => attempts.push(("DX12 (forced)", wgpu::Backends::DX12, false)),
        Ok("metal") => attempts.push(("Metal (forced)", wgpu::Backends::METAL, false)),
        Ok("primary") => attempts.push(("primary (forced)", wgpu::Backends::PRIMARY, false)),
        Ok(other) => {
            return Err(format!(
                "unknown NUI_BACKEND `{other}` (expected vulkan|gl|dx12|metal|primary)"
            ));
        }
        Err(_) => {
            attempts.push(("default", wgpu::Backends::all(), false));
            attempts.push(("GL + display handle", wgpu::Backends::GL, true));
        }
    }
    let mut last_error = String::from("no attempts were made");
    for (label, backends, with_display) in attempts {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        if with_display {
            descriptor.display = Some(Box::new(display.clone()));
        }
        let instance = wgpu::Instance::new(descriptor);
        let Ok(surface) = instance.create_surface(window.clone()) else {
            last_error = format!("{label}: surface creation failed");
            continue;
        };
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })) {
            Ok(adapter) => {
                let info = adapter.get_info();
                eprintln!(
                    "nui: renderer on {label}: {} ({:?})",
                    info.name, info.backend
                );
                return Ok((instance, surface, adapter));
            }
            Err(error) => {
                last_error = format!("{label}: {error}");
            }
        }
    }
    return Err(format!(
        "no suitable graphics adapter — tried: {last_error}"
    ));
}
