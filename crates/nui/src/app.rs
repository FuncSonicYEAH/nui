//! Application and window host: the run loop that ties platform input,
//! engine evaluation, layout, and rendering together (plan §2 per-frame
//! pipeline: 输入 → 绑定/动画 → 布局 → 渲染 → 提交).

use std::collections::HashMap;

use nui_compiler::compile;
use nui_core::{Point, Size};
use nui_runtime::{ElementTree, Engine};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::host::WindowHost;

/// Application builder: source text + window configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// `.nui` source of the root component.
    pub source: String,
    /// Initial logical window size (dp).
    pub size: Size,
    /// Window title.
    pub title: String,
}

impl AppConfig {
    /// Creates a config with a title and size.
    pub fn new(source: impl Into<String>, title: impl Into<String>, size: Size) -> AppConfig {
        return AppConfig {
            source: source.into(),
            title: title.into(),
            size,
        };
    }
}

/// Wakes the event loop from a watcher's background thread (plan §8: file
/// changes generate no winit events, so a sleeping `ControlFlow::Wait`
/// loop must be poked to notice them).
#[derive(Clone)]
pub struct ReloadWaker {
    proxy: winit::event_loop::EventLoopProxy<()>,
}

impl ReloadWaker {
    /// Wraps an event-loop proxy.
    pub fn new(proxy: winit::event_loop::EventLoopProxy<()>) -> ReloadWaker {
        return ReloadWaker { proxy };
    }

    /// Wakes the loop; the next pass polls the watcher.
    pub fn wake(&self) {
        let _ = self.proxy.send_event(());
    }
}

/// A source of hot-reload document updates (plan §8). The run loop calls
/// [`DocumentWatcher::poll_reload`] once per event-loop pass and reloads
/// the window host whenever a fresh source arrives. Implemented by
/// nui-preview's file watcher; hosts can roll their own.
pub trait DocumentWatcher {
    /// Called once before the event loop starts; store the waker to wake
    /// the loop from background threads when the document changes.
    fn attach(&mut self, waker: ReloadWaker);
    /// Returns the new document source when it changed, `None` otherwise.
    fn poll_reload(&mut self) -> Option<String>;
}

/// The nui application: owns the event loop and the window hosts.
pub struct Application {
    config: AppConfig,
    /// Host registry (functions + custom components); consumed when the
    /// window host is created.
    registry: Option<nui_runtime::Registry>,
    /// Hot-reload watcher (plan §8); polled each event-loop pass.
    watcher: Option<Box<dyn DocumentWatcher>>,
    event_loop: Option<EventLoop<()>>,
}

impl Application {
    /// Creates an application from `config` (no host extensions).
    pub fn new(config: AppConfig) -> Application {
        return Application {
            config,
            registry: None,
            watcher: None,
            event_loop: None,
        };
    }

    /// Builder: adds a host registry — custom Rust components and host
    /// functions become available to the document (plan §5 宿主互操作).
    pub fn with_registry(mut self, registry: nui_runtime::Registry) -> Application {
        self.registry = Some(registry);
        return self;
    }

    /// Builder: adds a hot-reload watcher polled by the run loop (plan §8).
    pub fn with_watcher(mut self, watcher: Box<dyn DocumentWatcher>) -> Application {
        self.watcher = Some(watcher);
        return self;
    }

    /// Runs the app until the window closes. Blocking; returns when done.
    pub fn run(mut self) {
        let event_loop = match self.event_loop.take() {
            Some(event_loop) => Some(event_loop),
            None => EventLoop::builder().build().ok(),
        };
        let Some(event_loop) = event_loop else {
            eprintln!("nui: no event loop available in this environment");
            return;
        };
        let config = self.config.clone();
        let mut watcher = self.watcher.take();
        if let Some(watcher) = &mut watcher {
            watcher.attach(ReloadWaker::new(event_loop.create_proxy()));
        }
        let mut handler = NuiAppHandler {
            config,
            registry: self.registry.take(),
            watcher,
            host: None,
        };
        let _ = event_loop.run_app(&mut handler);
    }
}

/// The `ApplicationHandler` impl: one window, one engine host.
struct NuiAppHandler {
    config: AppConfig,
    registry: Option<nui_runtime::Registry>,
    watcher: Option<Box<dyn DocumentWatcher>>,
    host: Option<WindowHost>,
}

impl ApplicationHandler for NuiAppHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.host.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.size.width,
                self.config.size.height,
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("nui: window creation failed: {error}");
                return;
            }
        };
        let registry = self.registry.take().unwrap_or_default();
        let display = event_loop.owned_display_handle();
        match pollster::block_on(WindowHost::new(
            window,
            &self.config.source,
            registry,
            display,
        )) {
            Ok(host) => self.host = Some(host),
            Err(error) => eprintln!("nui: renderer init failed: {error}"),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(host) = &mut self.host else {
            return;
        };
        if host.window().id() != window_id {
            return;
        }
        // Rendered frames: pipeline (with the real frame delta) + submit.
        // While work remains (animations, timers, dirty bindings), keep
        // polling; otherwise the loop waits for events.
        if let WindowEvent::RedrawRequested = event {
            host.render_frame();
            if host.has_pending_work() {
                host.window().request_redraw();
                event_loop.set_control_flow(ControlFlow::Poll);
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }
        if host.handle_event(event) {
            return;
        }
        if host.should_close() {
            event_loop.exit();
            return;
        }
        if host.needs_redraw() {
            host.window().request_redraw();
        }
        // On-demand redraw (plan §2): no dirty data and no active animation
        // means no request_redraw.
        if !host.has_pending_work() {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(host) = &mut self.host else {
            return;
        };
        // Hot reload (plan §8): the watcher polls between passes; a failed
        // recompile keeps the last good document on screen and reports to
        // stderr (v1 has no text pipeline for an in-window overlay).
        let reloaded = self
            .watcher
            .as_mut()
            .and_then(|watcher| return watcher.poll_reload());
        if let Some(source) = reloaded {
            let title = &self.config.title;
            match host.reload(&source) {
                Ok(()) => {
                    host.set_error_overlay(None);
                    host.window().set_title(title);
                }
                Err(rendered) => {
                    host.set_error_overlay(Some(rendered.clone()));
                    host.window().set_title(&format!("{title} — compile error"));
                    eprintln!("nui: reload failed, keeping the last good document:\n{rendered}");
                }
            }
        }
        if host.has_pending_work() {
            host.window().request_redraw();
        }
    }
}

/// Rectangle hit test over the element tree. Children are checked first
/// (innermost wins: smaller area = more specific); `Scroll` ancestors
/// translate their subtree, so scrolled content hit-tests correctly (M9).
pub fn hit_test(tree: &ElementTree, position: Point) -> Option<crate::host::HitTarget> {
    fn walk(
        tree: &ElementTree,
        id: nui_runtime::ElementId,
        offset: Point,
        position: Point,
        best: &mut Option<(crate::host::HitTarget, f32)>,
    ) {
        let element = &tree.arena[id];
        let scroll = if element.ty == "Scroll" || element.ty == "ListView" {
            f_of(element, "scroll_y").unwrap_or(0.0)
        } else {
            0.0
        };
        let child_offset = Point::new(offset.x, offset.y - scroll);
        for child in element.children.clone() {
            walk(tree, child, child_offset, position, best);
        }
        let x = f_of(element, "x").unwrap_or(0.0) + offset.x;
        let y = f_of(element, "y").unwrap_or(0.0) + offset.y;
        let width = f_of(element, "width").unwrap_or(0.0);
        let height = f_of(element, "height").unwrap_or(0.0);
        let rect = nui_core::Rect::new(Point::new(x, y), Size::new(width, height));
        if rect.contains(position) {
            let area = width * height;
            if best
                .as_ref()
                .is_none_or(|(_, best_area)| return area < *best_area)
            {
                *best = Some((crate::host::HitTarget { element: id }, area));
            }
        }
    }
    let mut best: Option<(crate::host::HitTarget, f32)> = None;
    for root in tree.roots.clone() {
        walk(tree, root, Point::ZERO, position, &mut best);
    }
    return best.map(|(target, _)| return target);
}

/// Reads an f32 property.
fn f_of(element: &nui_runtime::Element, name: &str) -> Option<f32> {
    return element.get(name).and_then(|value| {
        return match value {
            nui_core::Value::Int(inner) => Some(*inner as f32),
            nui_core::Value::Float(inner) => Some(*inner as f32),
            nui_core::Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
            _ => None,
        };
    });
}

/// Compiles a source and reports diagnostics through stderr; used by the
/// facade to fail fast on a bad document.
pub fn compile_or_report(source: &str) -> Option<nui_compiler::DocumentIr> {
    let outcome = compile(source);
    if !outcome.diagnostics.is_empty() {
        for diagnostic in &outcome.diagnostics {
            eprint!(
                "{}",
                nui_syntax::render_diagnostic(source, "app.nui", diagnostic)
            );
        }
        return None;
    }
    return Some(outcome.document);
}

/// Re-exported for the facade's internals: typed engine + tree pair.
pub type EnginePair = (ElementTree, Engine);

/// Timer elapsed bookkeeping per element (shared by hosts).
pub type TimerElapsed = HashMap<nui_runtime::ElementId, f64>;
