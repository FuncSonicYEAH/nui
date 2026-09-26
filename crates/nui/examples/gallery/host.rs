//! The gallery's host-side half: the page table, the document assembly,
//! and the models and behaviors the pages bind to.
//!
//! This file is deliberately *not* an example of its own — it is a module
//! shared by two binaries:
//!
//! - `examples/gallery/main.rs` (the windowed app and the `--snap`
//!   offscreen mode);
//! - `tests/gallery.rs`, which pulls it in with
//!   `#[path = "../examples/gallery/host.rs"]`.
//!
//! That is the whole point of the split: the test reads the **same**
//! `PAGES` table the window does, so a page cannot exist in the demo but
//! fail in the test (or vice versa). Everything here must therefore stay
//! free of windowing code — no wgpu surfaces, no event loop; just
//! document text, a registry, and a temp-file texture.
#![allow(clippy::unwrap_used)]

use nui_core::{Size, Value};
use nui_runtime::canvas::{CanvasCap, CanvasPainter};
use nui_runtime::element::ElementId;
use nui_runtime::{BehaviorContext, ComponentDesc, ElementBehavior, ModelRow, Registry, VecModel};

/// Logical window size. Wide enough for the sidebar plus the widest page.
pub(crate) const WINDOW: Size = Size {
    width: 1120.0,
    height: 760.0,
};
/// Sidebar width; the content area gets the rest.
const SIDEBAR_WIDTH: f32 = 194.0;

/// One page: its sidebar label and the fragment that draws it.
///
/// The order of [`PAGES`] *is* the sidebar order and the page's slot
/// number, so adding a page means adding one row here and one file under
/// `pages/`.
pub(crate) struct Page {
    /// Slug used for element ids and snapshot filenames.
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    /// The `.nui` fragment (a single top-level element).
    pub(crate) body: &'static str,
}

/// Every page, in sidebar order.
///
/// The order is also the slot number the sidebar selects with, so pages
/// are grouped by theme: the declarative language first, then the controls
/// built on it, then the drawing primitives, then the host-interop pages.
pub(crate) const PAGES: &[Page] = &[
    Page {
        key: "counter",
        label: "Counter",
        body: include_str!("pages/counter.nui"),
    },
    Page {
        key: "widgets",
        label: "Widgets",
        body: include_str!("pages/widgets.nui"),
    },
    Page {
        key: "text_fields",
        label: "Text fields",
        body: include_str!("pages/text_fields.nui"),
    },
    Page {
        key: "containers",
        label: "Containers",
        body: include_str!("pages/containers.nui"),
    },
    Page {
        key: "form",
        label: "Form",
        body: include_str!("pages/form.nui"),
    },
    Page {
        key: "list",
        label: "Virtual list",
        body: include_str!("pages/list.nui"),
    },
    Page {
        key: "todo",
        label: "Todo (model)",
        body: include_str!("pages/todo.nui"),
    },
    Page {
        key: "showcase",
        label: "Scroll + layers",
        body: include_str!("pages/showcase.nui"),
    },
    Page {
        key: "canvas",
        label: "Canvas",
        body: include_str!("pages/canvas.nui"),
    },
    Page {
        key: "paths",
        label: "Paths",
        body: include_str!("pages/paths.nui"),
    },
    Page {
        key: "strokes",
        label: "Strokes",
        body: include_str!("pages/strokes.nui"),
    },
    Page {
        key: "waves",
        label: "Waveline",
        body: include_str!("pages/waves.nui"),
    },
    Page {
        key: "transform",
        label: "Transform",
        body: include_str!("pages/transform.nui"),
    },
    Page {
        key: "image",
        label: "Image",
        body: include_str!("pages/image.nui"),
    },
];

/// The one host asset a page needs. Written before the document is
/// assembled, so `{{texture}}` always resolves; see [`write_demo_texture`].
fn texture_path() -> String {
    return std::env::temp_dir()
        .join("nui-gallery-texture.bmp")
        .display()
        .to_string();
}

/// Writes a 64x64 24-bit BMP for the nine-slice demo: a rounded panel
/// (corner radius 20) filled with a blue-to-orange gradient, the pixels
/// outside the rounding filled with the page background so the corners
/// read as cut out. Hand-rolled because a BMP header is 54 bytes and the
/// alternative is an image encoder in the dependency list.
///
/// The panel touches all four edges on purpose: `slice = 20dp` keeps
/// those rounded corners fixed while the middle bands stretch, which is
/// exactly what the image page shows.
fn write_demo_texture(path: &str) {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    const RADIUS: f32 = 20.0;
    // `nui::host::CLEAR_COLOR` as sRGB — the content area's background.
    const PAGE: [u8; 3] = [20, 23, 28];
    let row_padding = (4 - (WIDTH * 3) % 4) % 4;
    let row_size = WIDTH * 3 + row_padding;
    let data_size = 54 + row_size * HEIGHT;
    let mut bytes: Vec<u8> = Vec::with_capacity(data_size as usize);
    // BITMAPFILEHEADER
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&WIDTH.to_le_bytes());
    bytes.extend_from_slice(&HEIGHT.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(row_size * HEIGHT).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 16]);
    // Pixels, bottom-up rows, BGR order.
    for y in (0..HEIGHT).rev() {
        for x in 0..WIDTH {
            // Distance from each corner circle's centre; a pixel whose
            // nearest corner axis keeps it outside the rounding is page.
            let dx = (RADIUS - 0.5 - x as f32).clamp(0.0, RADIUS);
            let dy = (RADIUS - 0.5 - y as f32).clamp(0.0, RADIUS);
            let inside = (dx * dx + dy * dy).sqrt() <= RADIUS - 0.5;
            let (r, g, b) = if inside {
                let t = x as f32 / (WIDTH - 1) as f32;
                (
                    (85.0 + (255.0 - 85.0) * t) as u8,
                    (170.0 + (140.0 - 170.0) * t) as u8,
                    (238.0 + (60.0 - 238.0) * t) as u8,
                )
            } else {
                (PAGE[0], PAGE[1], PAGE[2])
            };
            bytes.extend_from_slice(&[b, g, r]);
        }
        bytes.extend(std::iter::repeat_n(0u8, row_padding as usize));
    }
    std::fs::write(path, bytes).expect("demo texture write works");
}

/// Builds the gallery document: sidebar + one content area holding every
/// page fragment.
///
/// Also materialises the host-side assets a fragment refers to, so that
/// calling this one function is enough to get a complete document.
pub(crate) fn document() -> String {
    let texture = texture_path();
    write_demo_texture(&texture);
    let mut sidebar = String::new();
    let mut content = String::new();
    for (slot, page) in PAGES.iter().enumerate() {
        // Sidebar entry. `Rectangle` + `on click` rather than a `Button`:
        // any element receives `click` (the host emits it on the captured
        // element, not only on widget types), and a plain rect gives the
        // selected state one `fill` binding instead of a variant's skin.
        sidebar.push_str(&format!(
            r#"
                Rectangle(id = nav_{key}, width = 170dp, height = 32dp,
                          radius = 7dp,
                          fill <- root.page == {slot} ? #2f3a4d : #00000000) {{
                    on click => root.page = {slot}
                    Text(content = "{label}", padding = 9dp, font.size = 14dp,
                         color <- root.page == {slot} ? #e8ecf2 : #9aa4b2)
                }}
"#,
            key = page.key,
            label = page.label,
        ));
        content.push_str(&with_visibility(
            &page.body.replace("{{texture}}", &texture),
            slot,
        ));
        content.push('\n');
    }
    return format!(
        r#"
component Gallery {{
    property page: Int = 0

    // Host-supplied models, seeded by `build_registry`. They are component
    // properties rather than element attributes on purpose: `root.x` is
    // checked against this table at compile time, while a read through an
    // element id (`widgets_page.volume`) is deliberately unchecked — which
    // is what lets each page keep its own state without declaring it here.
    property listRows: Model
    property showcaseRows: Model
    property todoItems: Model

    Window(id = root) {{
        Row(spacing = 0dp, height = 100%) {{
            Column(id = sidebar, width = {SIDEBAR_WIDTH}dp, height = 100%,
                   spacing = 4dp, padding = 12dp, fill = #171b22) {{
                Text(content = "nui gallery", font.size = 17dp, height = 36dp,
                     color = #e8ecf2)
{sidebar}
            }}

            Scroll(id = content, height = 100%, flex_grow = 1.0) {{
{content}
            }}
        }}
    }}
}}
"#
    );
}

/// Splices `visible <- root.page == <slot>` into a fragment's opening tag.
///
/// The fragments are raw `.nui` text, so the only handle is the text
/// itself: the opening parenthesis of the top-level element. That must be
/// the first one in *code* — a fragment documents itself in a header
/// comment, and prose is exactly where an unmatched parenthesis turns up
/// (`the count lives on `counter_page``). Hence [`first_code_offset`], and
/// hence the assertions: failing loudly at startup beats emitting a
/// document with a stray property somewhere it does not parse.
fn with_visibility(body: &str, slot: usize) -> String {
    let start = first_code_offset(body);
    let code = &body[start..];
    let open = code
        .find('(')
        .unwrap_or_else(|| panic!("page fragment has no element: {code:.40}"));
    let brace = code.find('{').unwrap_or(code.len());
    assert!(
        open < brace,
        "a page fragment must open with an element's `(`, not a block",
    );
    let mut out = String::with_capacity(body.len() + 56);
    out.push_str(&body[..start + open + 1]);
    out.push_str(&format!("\n            visible <- root.page == {slot},"));
    out.push_str(&code[open + 1..]);
    return out;
}

/// Byte offset of the first line that is neither blank nor a `//` comment.
fn first_code_offset(body: &str) -> usize {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            offset += line.len();
            continue;
        }
        return offset;
    }
    return offset;
}

/// The host-side half: models and behaviors the pages bind to.
///
/// A single attach hook, because `Registry::on_attach` keeps one — three
/// separate calls would silently leave only the last.
pub(crate) fn build_registry() -> Registry {
    let mut registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let Some(root) = tree.lookup_id("root") else {
            return;
        };
        // The virtual list: 10,000 rows, of which the virtualizer keeps
        // ~16 alive. `hot` marks every 25th row so scrolling has a
        // landmark to steer by.
        let rows: Vec<ModelRow> = (0..10_000)
            .map(|index| {
                return vec![
                    ("label".to_string(), Value::String(format!("row {index}"))),
                    ("hot".to_string(), Value::Bool(index % 25 == 0)),
                ];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        engine.set_direct(tree, root, "listRows", Value::Model(model.0));

        // The scroll demo wants a list short enough to scroll by hand.
        let short: Vec<ModelRow> = (0..24)
            .map(|index| {
                return vec![
                    ("label".to_string(), Value::String(format!("row {index}"))),
                    ("hot".to_string(), Value::Bool(index % 4 == 0)),
                ];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(short)));
        engine.set_direct(tree, root, "showcaseRows", Value::Model(model.0));

        // The todo rows: two of them done, so the done/undone colour and
        // the tween are both visible on arrival.
        let todo_row = |label: &str, done: bool| {
            return vec![
                ("label".to_string(), Value::String(label.to_string())),
                ("done".to_string(), Value::Bool(done)),
            ];
        };
        let model = engine.add_model(Box::new(VecModel::from_rows(vec![
            todo_row("buy oat milk", true),
            todo_row("wire up For + Model", true),
            todo_row("ship the todo page", false),
            todo_row("hot reload (M5)", false),
        ])));
        engine.set_direct(tree, root, "todoItems", Value::Model(model.0));
    }));
    registry.register_component(
        ComponentDesc {
            name: "Canvas".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(ChartBehavior::new()))),
    );
    registry.register_component(
        ComponentDesc {
            name: "TodoCheckbox".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(TodoCheckbox))),
    );
    registry.register_component(
        ComponentDesc {
            name: "TodoRemove".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(TodoRemove))),
    );
    return registry;
}

/// Clicking the swatch toggles the row's `done` field in the model, which
/// re-drives every binding on that row — the scope lookup is what ties an
/// element back to the model row that produced it.
#[derive(Debug)]
struct TodoCheckbox;

impl ElementBehavior for TodoCheckbox {
    fn on_signal(&mut self, context: &mut BehaviorContext<'_>, element: ElementId, signal: &str) {
        if signal != "click" {
            return;
        }
        let Some(scope) = context.row_scope(element) else {
            return;
        };
        let done = context
            .model_field(&scope, "done")
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
        context.set_model_field(&scope, "done", Value::Bool(!done));
    }
}

/// Clicking removes the row; the next frame rebuilds the `For` rows.
#[derive(Debug)]
struct TodoRemove;

impl ElementBehavior for TodoRemove {
    fn on_signal(&mut self, context: &mut BehaviorContext<'_>, element: ElementId, signal: &str) {
        if signal != "click" {
            return;
        }
        if let Some(scope) = context.row_scope(element) {
            context.remove_model_row(&scope);
        }
    }
}

/// The canvas page's painter: an area line and a 270° gauge, recorded once
/// at instantiation through the host-side command buffer.
#[derive(Debug)]
struct ChartBehavior {
    painter: CanvasPainter,
}

impl ChartBehavior {
    fn new() -> ChartBehavior {
        let painter = CanvasPainter::new();
        // Area chart: a polyline through sample points, filled down to the
        // baseline and stroked on top.
        painter.move_to(0.0, 90.0);
        painter.line_to(0.0, 60.0);
        painter.cubic_to(40.0, 20.0, 70.0, 80.0, 105.0, 45.0);
        painter.cubic_to(140.0, 10.0, 180.0, 55.0, 210.0, 30.0);
        painter.line_to(240.0, 50.0);
        painter.line_to(240.0, 90.0);
        painter.close();
        painter.fill(nui_core::Color::from_rgba8(85, 170, 238, 90));
        painter.clear();
        painter.move_to(0.0, 60.0);
        painter.cubic_to(40.0, 20.0, 70.0, 80.0, 105.0, 45.0);
        painter.cubic_to(140.0, 10.0, 180.0, 55.0, 210.0, 30.0);
        painter.line_to(240.0, 50.0);
        painter.stroke(
            3.0,
            CanvasCap::Round,
            nui_core::Color::from_rgb8(85, 170, 238),
        );
        painter.clear();
        // Gauge: 270° of arc, opened at the bottom.
        let segments = 36;
        let start = 135.0f32;
        let sweep = 270.0f32;
        let radians = |degrees: f32| return degrees * std::f32::consts::PI / 180.0;
        painter.move_to(
            120.0 + 60.0 * radians(start).cos(),
            170.0 + 60.0 * radians(start).sin(),
        );
        for step in 1..=segments {
            let angle = radians(start + sweep * step as f32 / segments as f32);
            painter.line_to(120.0 + 60.0 * angle.cos(), 170.0 + 60.0 * angle.sin());
        }
        painter.stroke(
            8.0,
            CanvasCap::Round,
            nui_core::Color::from_rgb8(123, 196, 127),
        );
        return ChartBehavior { painter };
    }
}

impl ElementBehavior for ChartBehavior {
    fn on_signal(
        &mut self,
        _context: &mut BehaviorContext<'_>,
        _element: ElementId,
        _signal: &str,
    ) {
    }

    fn canvas(&self) -> Option<CanvasPainter> {
        return Some(self.painter.clone());
    }
}
