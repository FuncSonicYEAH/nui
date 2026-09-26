//! nui-layout: taffy adapter, dp conversion, and geometry write-back.
//!
//! Mapping (plan §5): `Column`/`Row`/`Wrap`/`Grid`/`Stack` map to taffy
//! containers; `spacing` (alias `gap`) becomes the gap, with
//! `row_spacing`/`column_spacing` as per-axis overrides; `padding` becomes
//! padding — plus the title inset for the chrome'd containers (`Panel`,
//! `Card`, `Dialog`), so their children start below the title bar.
//! `width`/`height` properties map to taffy dimensions (dp values pass
//! through, `%` maps to percentage, absent maps to the element's role
//! default). Layout results write back to the element properties
//! `x`/`y`/`width`/`height`, readable by bindings (plan §5 geometry
//! write-back with the iteration cap handled by the caller).

use taffy::prelude::*;

use nui_core::Value;
use nui_runtime::element::{Element, ElementId, ElementTree};
use nui_runtime::widget::chrome;

/// Default font size (dp) for `Text` elements without an explicit
/// `font.size` attached property.
const DEFAULT_FONT_SIZE_DP: f32 = 16.0;

/// Intrinsic content size of a leaf, handed to taffy as the measure
/// context.
///
/// Two shapes, because two kinds of leaf know their size at different
/// times. A `Text` shapes its single line once, before layout starts. A
/// multi-line text field breaks lines *at the width layout gives it*, so
/// its measurement can only happen inside the measure callback, where the
/// available width is known — and the result must be recomputed whenever
/// the field's content or width changes, which is exactly what a text
/// field does on every keystroke.
#[derive(Clone, Debug)]
enum Intrinsic {
    /// A size that does not depend on the available width.
    Fixed(taffy::Size<f32>),
    /// Text wrapped at whatever width layout offers.
    Wrapped {
        /// The text to break.
        content: String,
        /// Font size in dp.
        font_size: f32,
        /// Inset the painter applies on each side *beyond* the style padding
        /// taffy already removed (see [`chrome::text_inset_extra`]).
        inset: f32,
    },
}

/// The measure context a leaf carries.
type TextMeasure = Option<Intrinsic>;

/// How a container arranges its children (FUTURE 批次 5).
///
/// The first batch of controls never touched this crate — every control
/// was a leaf with an explicit box. This batch is where the adapter grows
/// a second display mode plus the alignment properties, so the dispatch
/// became a value instead of the old two-variant `Axis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// A flex container: `Column` / `Row` / `Wrap`, the panel shapes, the
    /// scrolling containers, and the window root.
    Flex {
        /// Main axis.
        direction: taffy::FlexDirection,
        /// Whether children wrap onto new lines (`Wrap`) or stay on one.
        wrap: bool,
    },
    /// A `Grid`: equal-width tracks, children placed in document order.
    Grid {
        /// Track count on the inline axis, if declared.
        columns: Option<u16>,
        /// Track count on the block axis, if declared.
        rows: Option<u16>,
    },
    /// A `Stack`: every child lands in the *same* cell, so they overlap in
    /// document order and the stack sizes to its largest child.
    Stack,
}

impl Arrangement {
    /// The arrangement an element asks for; `None` for leaf elements.
    ///
    /// `Panel` / `Card` / `Dialog` are columns: their chrome (background,
    /// border, title bar) is *painted*, not laid out, so the only layout
    /// consequence is the title inset folded into their top padding.
    fn of(element: &Element) -> Option<Arrangement> {
        return match element.ty.as_str() {
            "Window" | "Column" | "Panel" | "Card" | "Dialog" | "For" | "Scroll" | "ListView" => {
                Some(Arrangement::Flex {
                    direction: taffy::FlexDirection::Column,
                    wrap: false,
                })
            }
            "Row" => Some(Arrangement::Flex {
                direction: taffy::FlexDirection::Row,
                wrap: false,
            }),
            "Wrap" => Some(Arrangement::Flex {
                direction: taffy::FlexDirection::Row,
                wrap: true,
            }),
            "Grid" => Some(Arrangement::Grid {
                columns: count_property(element, "columns"),
                rows: count_property(element, "rows"),
            }),
            "Stack" => Some(Arrangement::Stack),
            _ => None,
        };
    }
}

/// Reads a dp `f32` property with a fallback.
fn f32_property(element: &Element, name: &str, fallback: f32) -> f32 {
    return optional_f32_property(element, name).unwrap_or(fallback);
}

/// Reads a dp `f32` property, or `None` when missing or non-numeric.
fn optional_f32_property(element: &Element, name: &str) -> Option<f32> {
    return element.get(name).and_then(|value| return dp_of(value));
}

/// Reads an enum-shaped property (`align`, `justify`, `orientation`).
fn enum_property<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    return element
        .get(name)
        .and_then(|value| return value.as_enum().ok());
}

/// Reads a track count (`columns`, `rows`); `0`/negative reads as absent,
/// so a binding that has not resolved yet cannot collapse a grid.
fn count_property(element: &Element, name: &str) -> Option<u16> {
    return optional_f32_property(element, name)
        .filter(|value| return *value >= 1.0)
        .map(|value| return value.round() as u16);
}

/// `align` -> how children sit on the cross axis. `stretch` is the
/// default, matching the Qt-Quick feel the rest of the adapter has.
fn align_items_of(element: &Element) -> taffy::style::AlignItems {
    return match enum_property(element, "align") {
        Some("start") => taffy::style::AlignItems::FlexStart,
        Some("center") => taffy::style::AlignItems::Center,
        Some("end") => taffy::style::AlignItems::FlexEnd,
        _ => taffy::style::AlignItems::Stretch,
    };
}

/// The same value in *grid* terms. A `Stack` is a one-cell grid, so its
/// item alignment is the whole story (`align` places on the block axis,
/// `justify` on the inline axis).
fn grid_items_of(name: Option<&str>) -> taffy::style::AlignItems {
    return match name {
        Some("start") => taffy::style::AlignItems::Start,
        Some("center") => taffy::style::AlignItems::Center,
        Some("end") => taffy::style::AlignItems::End,
        _ => taffy::style::AlignItems::Stretch,
    };
}

/// `align` / `justify` as a per-item self-alignment (`align_self` /
/// `justify_self`). The motivating case is a corner badge in a `Stack`:
/// the container stretches its children, and one child asks to be pinned
/// to a corner instead.
fn self_alignment_of(element: &Element, name: &str) -> Option<taffy::style::AlignSelf> {
    return Some(match enum_property(element, name)? {
        "start" => taffy::style::AlignSelf::Start,
        "center" => taffy::style::AlignSelf::Center,
        "end" => taffy::style::AlignSelf::End,
        "stretch" => taffy::style::AlignSelf::Stretch,
        _ => return None,
    });
}

/// `justify` -> main-axis distribution.
fn justify_content_of(element: &Element) -> taffy::style::JustifyContent {
    return match enum_property(element, "justify") {
        Some("center") => taffy::style::JustifyContent::Center,
        Some("end") => taffy::style::JustifyContent::FlexEnd,
        Some("space-between") => taffy::style::JustifyContent::SpaceBetween,
        Some("space-around") => taffy::style::JustifyContent::SpaceAround,
        Some("space-evenly") => taffy::style::JustifyContent::SpaceEvenly,
        _ => taffy::style::JustifyContent::FlexStart,
    };
}

/// Gap between children: `gap` (alias `spacing`), with `row_spacing` /
/// `column_spacing` overriding one axis each.
fn gaps(element: &Element) -> taffy::Size<taffy::style::LengthPercentage> {
    let base = f32_property(element, "gap", f32_property(element, "spacing", 0.0));
    return taffy::Size {
        width: length_value(f32_property(element, "column_spacing", base)),
        height: length_value(f32_property(element, "row_spacing", base)),
    };
}

/// Maps a `width`/`height` property to a taffy dimension; `None` when the
/// property was never written (the caller then applies the role default).
fn optional_dimension(element: &Element, name: &str) -> Option<TaffyDimension> {
    let value = element.get(name)?;
    return Some(match value {
        Value::Length(nui_core::Length::Dp(dp)) => TaffyDimension::length(dp.max(0.0)),
        Value::Length(nui_core::Length::Percent(percent)) => {
            TaffyDimension::percent(percent.max(0.0) / 100.0)
        }
        Value::Length(nui_core::Length::Auto) => TaffyDimension::auto(),
        Value::Int(inner) => TaffyDimension::length(*inner as f32),
        Value::Float(inner) => TaffyDimension::length(*inner as f32),
        _ => TaffyDimension::auto(),
    });
}

/// Internal alias keeping the mapping function readable.
type TaffyDimension = taffy::prelude::Dimension;

/// Whether the *layout* owns an axis' size on this element.
///
/// `write_back` stores the computed box in the element's own `width` /
/// `height` slot — the scene, hit testing and the widget tracker all read
/// it — which leaves one question the slot itself cannot answer on the
/// next pass: was that number declared by the document, or measured by us?
/// The difference decides whether taffy is told a size or asked for one, so
/// it is recorded separately, once, before the slot is first overwritten.
///
/// Getting this wrong is not academic: it is the difference between a
/// multi-line field that grows a line at a time as you type and one frozen
/// at the height it happened to have when the window first laid out.
fn is_auto(element: &Element, name: &str) -> bool {
    return element
        .get(&format!("layout.auto_{name}"))
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(false);
}

/// The document's declared size for an axis: the `width` / `height` slot,
/// unless the layout wrote it (see [`is_auto`]).
fn declared_dimension(element: &Element, name: &str) -> Option<TaffyDimension> {
    if is_auto(element, name) {
        return None;
    }
    return optional_dimension(element, name);
}

/// Builds the taffy style for one element from its properties.
///
/// Takes the tree rather than a lone element because one rule is
/// parent-dependent: a `Stack`'s children all land in its single cell, and
/// nothing else's do.
fn style_for(tree: &ElementTree, id: ElementId) -> Style {
    let element = &tree.arena[id];
    let arrangement = Arrangement::of(element);
    let padding = f32_property(element, "padding", chrome::default_padding(&element.ty));
    // A `Panel` / `Card` / `Dialog` reserves room for its title bar above
    // the content. The formula is shared with the painting side
    // (`nui_runtime::widget::chrome`), so the height reserved here and the
    // title drawn there cannot drift apart.
    let padding_top = padding + nui_runtime::widget::title_inset(element);
    let mut width = declared_dimension(element, "width");
    let mut height = declared_dimension(element, "height");

    // `Separator` is a hairline: `thickness` dp on one axis and a full
    // span on the other. Both axes stay overridable.
    if element.ty == "Separator" {
        let thickness = f32_property(element, "thickness", 1.0).max(0.0);
        if enum_property(element, "orientation") == Some("vertical") {
            width = width.or(Some(TaffyDimension::length(thickness)));
            height = height.or(Some(TaffyDimension::percent(1.0)));
        } else {
            width = width.or(Some(TaffyDimension::percent(1.0)));
            height = height.or(Some(TaffyDimension::length(thickness)));
        }
    }

    // An overlay (Dialog and friends, FUTURE 批次 4) is lifted out of its
    // parent's flex flow: a modal dialog must cover the window no matter
    // where in the document it was declared or what its siblings are
    // doing. Absolute positioning takes it out of the column; `write_back`
    // then centres it in the window (taffy's own alignment properties
    // position a node's *children*, not the node itself).
    let overlay = nui_runtime::widget::is_overlay(element);
    // A `Stack` is content-sized on both axes (it shrink-wraps its largest
    // child), so it opts out of the "containers fill the inline axis"
    // default the others share.
    let content_sized = matches!(arrangement, Some(Arrangement::Stack));
    // An explicit `flex_grow` says "share out the free space", and an
    // element that shares must not *also* default to filling its parent:
    // in `Row { Column(width = 194dp) Scroll(flex_grow = 1) }` the scroll
    // area would claim a full 100% and then grow, pushing the row past its
    // own width. CSS spells the same rule `flex: 1` ⇒ `flex-basis: 0`: on
    // the parent's main axis a growing element starts from zero, and its
    // share becomes its whole size.
    //
    // Only on that axis. A `flex_grow` inside a `Column` shares free
    // *height*; zeroing its width there would collapse it across the cross
    // axis instead — which is exactly what the default `Spacer` at the end
    // of a column must not do.
    let grows_along_x = optional_f32_property(element, "flex_grow")
        .is_some_and(|value| return value > 0.0)
        && element.parent.is_none_or(|parent| {
            return matches!(
                Arrangement::of(&tree.arena[parent]),
                Some(Arrangement::Flex {
                    direction: taffy::FlexDirection::Row,
                    ..
                })
            );
        });
    // Role defaults for the box: an overlay covers the window, a container
    // fills the inline axis (percent-sized children need real space to
    // resolve against), a leaf is content-sized.
    let default_width = if grows_along_x {
        TaffyDimension::length(0.0)
    } else if overlay || (arrangement.is_some() && !content_sized) {
        TaffyDimension::percent(1.0)
    } else {
        TaffyDimension::auto()
    };
    let default_height = if overlay {
        TaffyDimension::percent(1.0)
    } else {
        TaffyDimension::auto()
    };
    let mut style = Style {
        size: taffy::Size {
            width: width.unwrap_or(default_width),
            height: height.unwrap_or(default_height),
        },
        padding: taffy::Rect {
            left: length_value(padding),
            right: length_value(padding),
            top: length_value(padding_top),
            bottom: length_value(padding),
        },
        // Qt-Quick semantics: elements keep their natural size and overflow
        // their container (virtualized list content relies on this).
        flex_shrink: 0.0,
        ..Style::default()
    };
    // Margins: a CSS subset. `margin` sets all four sides,
    // `margin_horizontal` / `margin_vertical` override an axis, and a
    // per-side property overrides both. A `ListView` row uses
    // `margin_bottom` to fill its slot exactly — see
    // `nui_runtime::widget::row_slot` for why the slot must not be missed.
    let margin = optional_f32_property(element, "margin").unwrap_or(0.0);
    let margin_horizontal = optional_f32_property(element, "margin_horizontal").unwrap_or(margin);
    let margin_vertical = optional_f32_property(element, "margin_vertical").unwrap_or(margin);
    // Margins are `Dimension` (auto-capable), unlike padding's
    // `LengthPercentage` — hence the bare `length()` on the alias rather
    // than the `length_value` helper the box above uses.
    let margin_side = |name: &str, fallback: f32| {
        return taffy::style::LengthPercentageAuto::length(
            optional_f32_property(element, name).unwrap_or(fallback),
        );
    };
    style.margin = taffy::Rect {
        left: margin_side("margin_left", margin_horizontal),
        right: margin_side("margin_right", margin_horizontal),
        top: margin_side("margin_top", margin_vertical),
        bottom: margin_side("margin_bottom", margin_vertical),
    };
    if overlay {
        style.position = taffy::style::Position::Absolute;
    }
    if arrangement.is_some() {
        // Containers default to filling the cross axis and stretching
        // children, so percent-sized inner elements resolve against real
        // space (matches QML's default anchoring feel).
        style.align_self = Some(taffy::style::AlignSelf::Stretch);
    }
    // `Spacer` eats free space by default; anything else opts in.
    style.flex_grow = optional_f32_property(element, "flex_grow")
        .map(|value| return value.max(0.0))
        .unwrap_or(if element.ty == "Spacer" { 1.0 } else { 0.0 });

    if let Some(arrangement) = arrangement {
        style.align_items = Some(align_items_of(element));
        style.justify_content = Some(justify_content_of(element));
        style.gap = gaps(element);
        match arrangement {
            Arrangement::Flex { direction, wrap } => {
                style.flex_direction = direction;
                style.flex_wrap = if wrap {
                    taffy::style::FlexWrap::Wrap
                } else {
                    taffy::style::FlexWrap::NoWrap
                };
                if wrap {
                    // Lines keep their content height instead of being
                    // stretched to fill the container. The CSS default
                    // (`normal`, i.e. stretch) would give every line an
                    // equal share of the height, which is not what a
                    // flowing layout of natural-sized items means in a
                    // Qt-Quick-shaped framework.
                    style.align_content = Some(taffy::style::AlignContent::Start);
                }
            }
            Arrangement::Grid { columns, rows } => {
                style.display = taffy::style::Display::Grid;
                // Implicit rows size to their content rather than sharing
                // the container's height (`align_content: normal` stretches
                // them, CSS-style, which surprises anyone coming from QML's
                // `Grid`). Declared `rows` are still `1fr` tracks.
                style.align_content = Some(taffy::style::AlignContent::Start);
                // A grid declaring neither axis reads as a single column
                // rather than collapsing: the compiler has no element
                // schema that could reject it earlier.
                let columns = columns.or(if rows.is_none() { Some(1) } else { None });
                style.grid_template_columns = match columns {
                    Some(count) => taffy::style_helpers::evenly_sized_tracks(count),
                    None => Vec::new(),
                };
                style.grid_template_rows = match rows {
                    Some(count) => taffy::style_helpers::evenly_sized_tracks(count),
                    None => Vec::new(),
                };
            }
            Arrangement::Stack => {
                style.display = taffy::style::Display::Grid;
                // One auto track per axis: the single cell sizes to the
                // largest child, which is what makes a stack size to its
                // content when it declares no size of its own.
                style.grid_template_columns = vec![taffy::style_helpers::auto()];
                style.grid_template_rows = vec![taffy::style_helpers::auto()];
                // ...and the track *stretches* to the stack when the stack
                // declares a size, so `Stack(width, height)` with unsized
                // children fills exactly. (Grid layout makes this an
                // explicit property rather than the flex default.)
                style.align_content = Some(taffy::style::AlignContent::Stretch);
                style.justify_content = Some(taffy::style::JustifyContent::Stretch);
                // `align` places on the block axis, `justify` on the
                // inline one; both default to stretching the child to the
                // cell.
                style.align_items = Some(grid_items_of(enum_property(element, "align")));
                style.justify_items = Some(grid_items_of(enum_property(element, "justify")));
            }
        }
    }

    // Window IS the viewport: it always fills the window regardless of its
    // declared width/height (those document the requested initial size; the
    // real initial size comes from the host, and a resized window must not
    // leave a band of clear color beside the root — plan §M11).
    if element.ty == "Window" {
        style.size = taffy::Size {
            width: taffy::prelude::Dimension::percent(1.0),
            height: taffy::prelude::Dimension::percent(1.0),
        };
    }

    // A `Stack`'s children all sit in the cell its grid declares (line 1
    // spans one track), which is what makes them overlap.
    if tree.arena[id]
        .parent
        .is_some_and(|parent| return tree.arena[parent].ty == "Stack")
    {
        style.grid_row = taffy::Line {
            start: taffy::style_helpers::line(1),
            end: taffy::style_helpers::auto(),
        };
        style.grid_column = taffy::Line {
            start: taffy::style_helpers::line(1),
            end: taffy::style_helpers::auto(),
        };
    }

    // Per-item overrides of the container's alignment, applied last so a
    // child's own `align_self` beats both its parent's `align` and the
    // container default. `align_self` is also how a container stops
    // stretching itself inside its own parent.
    if let Some(align_self) = self_alignment_of(element, "align_self") {
        style.align_self = Some(align_self);
    }
    style.justify_self = self_alignment_of(element, "justify_self");

    // `visible = false` takes the element out of the layout entirely
    // (`display: none`): it occupies no space, its subtree is not
    // measured, and it keeps no box — which is also what makes it
    // unpaintable and unhittable, since both walks start from the box.
    //
    // Applied *last*, after the `Grid`/`Stack` arrangements above have
    // set their own display: "hidden" has to outrank "how it arranges".
    if !nui_runtime::widget::is_visible(&tree.arena[id]) {
        style.display = taffy::style::Display::None;
    }
    return style;
}

/// Extracts a dp f32 from a property value (Int/Float/Length::Dp).
fn dp_of(value: &Value) -> Option<f32> {
    return match value {
        Value::Int(inner) => Some(*inner as f32),
        Value::Float(inner) => Some(*inner as f32),
        Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
        _ => None,
    };
}

fn length_value(dp: f32) -> taffy::style::LengthPercentage {
    return taffy::style::LengthPercentage::length(dp);
}

/// Lays out the tree with `viewport` as the root constraint and writes the
/// results back to the elements' `x`/`y`/`width`/`height` properties (dp).
/// Returns the visited element count.
pub fn layout(tree: &mut ElementTree, viewport: nui_core::Size) -> usize {
    return layout_with_text(tree, viewport, None);
}

/// Lays out the tree with intrinsic `Text` sizing: `text` shapes each
/// `Text` element's content (plan §6.2 paragraph measurement integrated
/// with layout); `None` sizes `Text` leaves purely from the container.
///
/// A *multi-line* text field is measured the same way, except that its
/// wrapping width is only known inside taffy's measure callback — so it is
/// measured there rather than up front (see [`Intrinsic`]). A field with a
/// declared height keeps it: taffy prefers an explicit size over a measured
/// one, which is what makes `height = 120dp` mean "this tall, scroll inside
/// it" while omitting it means "as tall as the content".
pub fn layout_with_text(
    tree: &mut ElementTree,
    viewport: nui_core::Size,
    text: Option<&mut nui_text::TextSystem>,
) -> usize {
    // Pre-measure every Text element (content, font size) once per pass.
    let mut text = text;
    let mut taffy_tree: taffy::TaffyTree<TextMeasure> = taffy::TaffyTree::new();
    let mut node_map: Vec<(ElementId, taffy::NodeId)> = Vec::new();
    let mut roots = Vec::new();
    // Scoped so the shaping borrow of `text` ends before the measure
    // callback borrows it a second time.
    {
        let mut measure_element = |tree: &ElementTree, id: ElementId| -> TextMeasure {
            let element = &tree.arena[id];
            if element.ty == "Text" {
                let text_system = text.as_mut()?;
                let content = element.get("content")?.as_str().ok()?;
                let font_size = element
                    .get("font.size")
                    .and_then(|value| return dp_of(value))
                    .unwrap_or(DEFAULT_FONT_SIZE_DP);
                let (width, height) = text_system.measure(content, font_size);
                return Some(Intrinsic::Fixed(taffy::prelude::Size {
                    width: width.max(1.0),
                    height: height.max(1.0),
                }));
            }
            // Only a *multi-line* field is content-sized. A one-line field
            // keeps the M7 contract — its box comes from the document — and
            // giving it a measure now would silently resize every existing
            // form.
            if !element.is_text_input() || !element.is_multiline() {
                return None;
            }
            // The `text` *property* is the display truth: every edit syncs
            // it (`Engine::sync_text_property`), and an external write — a
            // binding, a `set_direct` — lands on it. Reading the editing
            // state instead would shadow the property with whatever the
            // field held when it was last reconciled, so a bound `text`
            // write would size the box for the *previous* string.
            let content = element
                .get("text")
                .and_then(|value| return value.as_str().ok().map(str::to_string))
                .unwrap_or_default();
            let font_size = chrome::text_size(element);
            return Some(Intrinsic::Wrapped {
                content,
                font_size,
                inset: chrome::text_inset_extra(element),
            });
        };

        // Create taffy nodes top-down.
        fn create_nodes(
            taffy_tree: &mut taffy::TaffyTree<TextMeasure>,
            tree: &ElementTree,
            id: ElementId,
            node_map: &mut Vec<(ElementId, taffy::NodeId)>,
            measure_element: &mut dyn FnMut(&ElementTree, ElementId) -> TextMeasure,
        ) -> taffy::NodeId {
            let style = style_for(tree, id);
            let measure = measure_element(tree, id);
            let node = match measure {
                Some(context) => taffy_tree
                    .new_leaf_with_context(style, Some(context))
                    .expect("in-memory layout tree"),
                None => taffy_tree.new_leaf(style).expect("in-memory layout tree"),
            };
            node_map.push((id, node));
            let children: Vec<ElementId> = tree.arena[id].children.clone();
            let mut child_nodes = Vec::with_capacity(children.len());
            for child in children {
                let child_node = create_nodes(taffy_tree, tree, child, node_map, measure_element);
                child_nodes.push(child_node);
            }
            if !child_nodes.is_empty() {
                taffy_tree
                    .set_children(node, &child_nodes)
                    .expect("in-memory layout tree");
            }
            return node;
        }

        for root in tree.roots.clone() {
            let node = create_nodes(
                &mut taffy_tree,
                tree,
                root,
                &mut node_map,
                &mut measure_element,
            );
            roots.push(node);
        }
    }

    let root_node = if roots.len() == 1 {
        roots[0]
    } else {
        let container = taffy_tree
            .new_leaf(Style {
                display: taffy::style::Display::Flex,
                ..Style::default()
            })
            .expect("in-memory layout tree");
        taffy_tree
            .set_children(container, &roots)
            .expect("in-memory layout tree");
        container
    };

    taffy_tree
        .compute_layout_with_measure(
            root_node,
            Size {
                width: AvailableSpace::Definite(viewport.width),
                height: AvailableSpace::Definite(viewport.height),
            },
            |known, available, _node, context, _style| {
                // `context` is `Option<&mut TextMeasure>`; only leaves with
                // a measure context carry one.
                let Some(intrinsic) = context.and_then(|inner| return inner.as_ref()) else {
                    return Size::ZERO;
                };
                let (content, font_size, inset) = match intrinsic {
                    Intrinsic::Fixed(size) => return *size,
                    Intrinsic::Wrapped {
                        content,
                        font_size,
                        inset,
                    } => (content, *font_size, *inset),
                };
                let Some(text_system) = text.as_mut() else {
                    return Size::ZERO;
                };
                // The width taffy offers is the field's content box (it has
                // already removed the style padding), so only the painter's
                // *extra* inset is left to subtract — and to add back, since
                // taffy adds the padding to the measured size itself.
                let width = known.width.or_else(|| {
                    return match available.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    };
                });
                let (widest, height) = match width {
                    Some(width) => text_system.measure_wrapped(
                        content,
                        font_size,
                        (width - inset * 2.0).max(1.0),
                    ),
                    // No definite width to wrap at: measure unwrapped, which
                    // still counts every hard line break.
                    None => text_system.measure(content, font_size),
                };
                return Size {
                    width: widest.max(1.0),
                    height: (height + inset * 2.0).max(1.0),
                };
            },
        )
        .expect("in-memory layout tree");

    // Write back geometry in dp. Taffy locations are parent-relative; the
    // scene and hit testing need ABSOLUTE positions, so accumulate down
    // the tree (M9 playground screenshots exposed this as a real bug —
    // nested elements were drawn at their relative offsets).
    fn write_back(
        tree: &mut ElementTree,
        id: ElementId,
        base: (f64, f64),
        taffy_tree: &taffy::TaffyTree<TextMeasure>,
        node_map: &Vec<(ElementId, taffy::NodeId)>,
        viewport: nui_core::Size,
        inherited_visible: bool,
    ) {
        let node = node_map
            .iter()
            .find(|(element, _)| return *element == id)
            .map(|(_, node)| return *node)
            .expect("every element has a taffy node");
        let layout = taffy_tree.layout(node).expect("computed layout");
        let mut x = base.0 + layout.location.x as f64;
        let mut y = base.1 + layout.location.y as f64;
        let width = layout.size.width as f64;
        let height = layout.size.height as f64;
        // An overlay ignores where taffy put it (absolute positioning
        // leaves it at the containing block's origin) and centres itself
        // in the window instead. That is the whole point of a modal
        // dialog: it is anchored to the window, not to the document flow
        // it happens to be declared in.
        if nui_runtime::widget::is_overlay(&tree.arena[id]) {
            x = (viewport.width as f64 - width) / 2.0;
            y = (viewport.height as f64 - height) / 2.0;
        }
        // `visible = false` means taffy gave this node (and its subtree) a
        // zero box, and that zero is *not* a measurement — it is the
        // absence of one. Writing it back would poison the element's own
        // size slot: `layout.auto_height` says the axis was declared by
        // the document, so `style_for` would read the zero back as the
        // declaration and the element would stay collapsed forever, even
        // after it became visible again.
        //
        // Hence: hidden subtrees keep the sizes they had (which are the
        // declared ones, or the last real measurement) and are re-laid out
        // the moment they come back. Visibility is inherited here for the
        // same reason it is in the scene walk — a child of a hidden
        // element is hidden too, and it is the child that would otherwise
        // take the zero.
        let visible = inherited_visible && nui_runtime::widget::is_visible(&tree.arena[id]);
        let element = &mut tree.arena[id];
        // Record which axes the document did *not* declare, before the
        // slots stop saying so. Decided once: after this, the slot holds a
        // number the layout produced, and only this flag distinguishes it
        // from a declared one on the next pass (see `is_auto`).
        for (name, slot) in [
            ("width", "layout.auto_width"),
            ("height", "layout.auto_height"),
        ] {
            if element.get(slot).is_none() {
                let auto = optional_dimension(element, name).is_none();
                element.set(slot, Value::Bool(auto));
            }
        }
        if visible {
            element.set("x", Value::Float(x));
            element.set("y", Value::Float(y));
            element.set("width", Value::Float(width));
            element.set("height", Value::Float(height));
        }
        let children = element.children.clone();
        for child in children {
            write_back(tree, child, (x, y), taffy_tree, node_map, viewport, visible);
        }
    }
    for root in tree.roots.clone() {
        write_back(
            tree,
            root,
            (0.0, 0.0),
            &taffy_tree,
            &node_map,
            viewport,
            true,
        );
    }
    return node_map.len();
}

/// The visual lines of `content` wrapped at `wrap_width`, in the shape the
/// editing core wants: char ranges plus a caret x per char.
///
/// This is the bridge between the two halves of a multi-line field.
/// `nui-text` owns the shaping (and the fonts); `nui-runtime` owns the
/// cursor arithmetic (and no fonts, deliberately — see
/// [`nui_runtime::text_input::VisualLine`]); this crate depends on both and
/// is where they meet.
///
/// `wrap_width` is the field's *content* width — its box minus
/// [`chrome::text_inset`] on each side — so the table describes the lines
/// as they are actually drawn, not as they would break in the full box.
pub fn visual_lines(
    text: &mut nui_text::TextSystem,
    content: &str,
    font_size: f32,
    wrap_width: f32,
) -> Vec<nui_runtime::text_input::VisualLine> {
    let layout = text.layout_wrapped(content, font_size, wrap_width.max(1.0));
    return layout
        .lines
        .iter()
        .map(|line| {
            let mut caret_x: Vec<f32> = (line.start..line.end)
                .map(|index| return layout.caret_x(index))
                .collect();
            // The line's end is a caret position too — where a caret on a
            // full line sits — and it is *not* `caret_x(end)`, which reports
            // the next line's start.
            caret_x.push(line.width);
            return nui_runtime::text_input::VisualLine {
                start: line.start,
                end: line.end,
                caret_x,
            };
        })
        .collect();
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Length;
    use nui_runtime::ElementTree;

    fn build_column_tree() -> (ElementTree, ElementId, ElementId, ElementId) {
        let mut tree = ElementTree::new();
        let column = Element::new("Column", None);
        let column_id = tree.insert(column);
        let mut first = Element::new("Rectangle", None);
        first.set("width", Value::Length(Length::Dp(100.0)));
        first.set("height", Value::Length(Length::Dp(40.0)));
        let first_id = tree.insert(first);
        let mut second = Element::new("Rectangle", None);
        second.set("width", Value::Length(Length::Dp(50.0)));
        second.set("height", Value::Length(Length::Dp(30.0)));
        let second_id = tree.insert(second);
        tree.append_child(column_id, first_id);
        tree.append_child(column_id, second_id);
        tree.push_root(column_id);
        return (tree, column_id, first_id, second_id);
    }

    #[test]
    fn column_stacks_children_with_spacing() {
        let (mut tree, column, first, second) = build_column_tree();
        tree.arena[column].set("spacing", Value::Length(Length::Dp(10.0)));
        tree.arena[column].set("height", Value::Length(Length::Dp(100.0)));
        let count = layout(&mut tree, nui_core::Size::new(400.0, 400.0));
        assert_eq!(count, 3);
        assert_eq!(tree.arena[first].get("y"), Some(&Value::Float(0.0)));
        // first ends at 40, plus 10 gap -> second starts at 50.
        assert_eq!(tree.arena[second].get("y"), Some(&Value::Float(50.0)));
        assert_eq!(tree.arena[first].get("width"), Some(&Value::Float(100.0)));
    }

    #[test]
    fn row_lays_out_horizontally() {
        let mut tree = ElementTree::new();
        let row = tree.insert(Element::new("Row", None));
        let box_element = tree.insert(Element::new("Rectangle", None));
        tree.arena[box_element].set("width", Value::Length(Length::Dp(60.0)));
        tree.arena[box_element].set("height", Value::Length(Length::Dp(20.0)));
        tree.append_child(row, box_element);
        tree.push_root(row);
        layout(&mut tree, nui_core::Size::new(400.0, 400.0));
        assert_eq!(tree.arena[box_element].get("x"), Some(&Value::Float(0.0)));
    }

    #[test]
    fn percentage_width_resolves_against_viewport() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Column", None));
        let child = tree.insert(Element::new("Rectangle", None));
        tree.arena[child].set("width", Value::Length(Length::Percent(50.0)));
        tree.arena[child].set("height", Value::Length(Length::Dp(10.0)));
        tree.append_child(root, child);
        tree.push_root(root);
        layout(&mut tree, nui_core::Size::new(400.0, 200.0));
        assert_eq!(tree.arena[child].get("width"), Some(&Value::Float(200.0)));
    }

    #[test]
    fn padding_offsets_children() {
        let (mut tree, column, first, _) = build_column_tree();
        tree.arena[column].set("padding", Value::Length(Length::Dp(8.0)));
        layout(&mut tree, nui_core::Size::new(400.0, 400.0));
        assert_eq!(tree.arena[first].get("x"), Some(&Value::Float(8.0)));
        assert_eq!(tree.arena[first].get("y"), Some(&Value::Float(8.0)));
    }

    #[test]
    fn window_root_fills_the_viewport() {
        // Window IS the viewport: its declared width/height must not pin
        // the root when the real window differs (resize, host size).
        let mut tree = ElementTree::new();
        let mut window = Element::new("Window", None);
        window.set("width", Value::Length(Length::Dp(480.0)));
        window.set("height", Value::Length(Length::Dp(620.0)));
        let window_id = tree.insert(window);
        tree.push_root(window_id);

        layout(&mut tree, nui_core::Size::new(800.0, 600.0));
        let root = &tree.arena[window_id];
        assert_eq!(root.get("width"), Some(&Value::Float(800.0)));
        assert_eq!(root.get("height"), Some(&Value::Float(600.0)));
        assert_eq!(root.get("x"), Some(&Value::Float(0.0)));
        assert_eq!(root.get("y"), Some(&Value::Float(0.0)));
    }
    #[test]
    fn an_overlay_is_lifted_out_of_the_column_flow() {
        // Two stacked rects then a dialog: the dialog must not be pushed
        // below them — it fills the window, so its box starts at (0, 0).
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", nui_core::Value::Float(400.0));
        root.set("height", nui_core::Value::Float(300.0));
        let root_id = tree.insert(root);
        tree.push_root(root_id);
        for _ in 0..2 {
            let mut rect = Element::new("Rectangle", None);
            rect.set(
                "width",
                nui_core::Value::Length(nui_core::Length::Dp(100.0)),
            );
            rect.set(
                "height",
                nui_core::Value::Length(nui_core::Length::Dp(40.0)),
            );
            let id = tree.insert(rect);
            tree.append_child(root_id, id);
        }
        let mut dialog = Element::new("Dialog", None);
        dialog.set(
            "width",
            nui_core::Value::Length(nui_core::Length::Dp(200.0)),
        );
        dialog.set(
            "height",
            nui_core::Value::Length(nui_core::Length::Dp(120.0)),
        );
        let dialog_id = tree.insert(dialog);
        tree.append_child(root_id, dialog_id);

        layout_with_text(&mut tree, nui_core::Size::new(400.0, 300.0), None);
        let x = f64_of(&tree, dialog_id, "x");
        let y = f64_of(&tree, dialog_id, "y");
        let width = f64_of(&tree, dialog_id, "width");
        let height = f64_of(&tree, dialog_id, "height");
        // Centred: (400 - 200) / 2 = 100, (300 - 120) / 2 = 90. Flex flow
        // would have put it at y = 80 (below the two 40dp rects).
        assert_eq!(x, 100.0);
        assert_eq!(y, 90.0);
        assert_eq!(width, 200.0);
        assert_eq!(height, 120.0);
    }

    #[test]
    fn an_overlay_flag_works_without_the_dialog_type() {
        // The mechanism is generic (FUTURE 批次 4): any element can opt in.
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", nui_core::Value::Float(400.0));
        root.set("height", nui_core::Value::Float(300.0));
        let root_id = tree.insert(root);
        tree.push_root(root_id);
        let mut toast = Element::new("Rectangle", None);
        toast.set("overlay", nui_core::Value::Bool(true));
        toast.set(
            "width",
            nui_core::Value::Length(nui_core::Length::Dp(100.0)),
        );
        toast.set(
            "height",
            nui_core::Value::Length(nui_core::Length::Dp(50.0)),
        );
        let toast_id = tree.insert(toast);
        tree.append_child(root_id, toast_id);
        layout_with_text(&mut tree, nui_core::Size::new(400.0, 300.0), None);
        assert_eq!(f64_of(&tree, toast_id, "x"), 150.0);
        assert_eq!(f64_of(&tree, toast_id, "y"), 125.0);
    }

    fn f64_of(tree: &ElementTree, id: ElementId, name: &str) -> f64 {
        return match tree.arena[id].get(name) {
            Some(nui_core::Value::Float(inner)) => *inner,
            other => panic!("expected Float {name}, got {other:?}"),
        };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod text_measure_tests {
    use super::*;
    use nui_core::Value;
    use nui_runtime::Element;

    #[test]
    fn text_elements_get_intrinsic_size() {
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", Value::Length(nui_core::Length::Dp(400.0)));
        root.set("height", Value::Length(nui_core::Length::Dp(300.0)));
        let root_id = tree.insert(root);
        tree.push_root(root_id);
        let mut label = Element::new("Text", None);
        label.set("content", Value::String("hello world".to_string()));
        let label_id = tree.insert(label);
        tree.append_child(root_id, label_id);

        let mut text = nui_text::TextSystem::with_embedded_font();
        layout_with_text(
            &mut tree,
            nui_core::Size::new(400.0, 300.0),
            Some(&mut text),
        );
        let width = tree.arena[label_id].get("width").cloned().unwrap();
        let width = match width {
            Value::Float(inner) => inner,
            other => panic!("expected Float width, got {other:?}"),
        };
        assert!(width > 20.0, "Text must be content-sized, got {width}");
        // Measurement is cached: a second pass reuses it.
        layout_with_text(
            &mut tree,
            nui_core::Size::new(400.0, 300.0),
            Some(&mut text),
        );
        let width_again = match tree.arena[label_id].get("width").cloned().unwrap() {
            Value::Float(inner) => inner,
            other => panic!("expected Float width, got {other:?}"),
        };
        assert_eq!(width, width_again);
    }
}

/// Multi-line text fields (FUTURE 批次 6): the wrap-aware measure, and the
/// line table the editing core walks with Up/Down.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod text_field_tests {
    use super::*;
    use nui_core::Length;
    use nui_runtime::Element;
    use nui_runtime::text_input::TextInputState;

    const VIEWPORT: nui_core::Size = nui_core::Size {
        width: 400.0,
        height: 300.0,
    };

    fn f64_of(tree: &ElementTree, id: ElementId, name: &str) -> f64 {
        return match tree.arena[id].get(name) {
            Some(Value::Float(inner)) => *inner,
            other => panic!("expected Float {name}, got {other:?}"),
        };
    }

    /// A viewport-filling `Column` root.
    fn root(tree: &mut ElementTree) -> ElementId {
        let mut element = Element::new("Column", None);
        element.set("width", Value::Length(Length::Dp(VIEWPORT.width)));
        element.set("height", Value::Length(Length::Dp(VIEWPORT.height)));
        let id = tree.insert(element);
        tree.push_root(id);
        return id;
    }

    /// A multi-line field of `width` dp holding `content`.
    fn area(width: f64, content: &str) -> Element {
        let mut element = Element::new("TextInput", None);
        element.set("multiline", Value::Bool(true));
        element.set("text", Value::String(content.to_string()));
        element.set("width", Value::Length(Length::Dp(width as f32)));
        return element;
    }

    #[test]
    fn a_multiline_field_grows_with_its_wrapped_lines() {
        let mut text = nui_text::TextSystem::with_embedded_font();
        let content = "alpha beta";
        let (full, line_height) = text.measure(content, DEFAULT_FONT_SIZE_DP);
        // A box wide enough for the *text*, but not for the text plus the
        // field's 8dp inset on each side: it must wrap into two lines, and
        // the measured height must include the inset the painter will use.
        let box_width = full + 8.0;

        let mut tree = ElementTree::new();
        let root_id = root(&mut tree);
        let field = tree.insert(area(f64::from(box_width), content));
        tree.append_child(root_id, field);
        layout_with_text(&mut tree, VIEWPORT, Some(&mut text));

        assert_eq!(
            f64_of(&tree, field, "height"),
            close(2.0 * f64::from(line_height) + 16.0),
            "two wrapped lines plus the field's own inset"
        );

        // A short text fits on one line: the field shrinks back.
        let mut short = ElementTree::new();
        let short_root = root(&mut short);
        let one_line = short.insert(area(200.0, "hi"));
        short.append_child(short_root, one_line);
        layout_with_text(&mut short, VIEWPORT, Some(&mut text));
        assert_eq!(
            f64_of(&short, one_line, "height"),
            close(f64::from(line_height) + 16.0)
        );

        // A declared height wins over the measurement, so `height = 30dp`
        // means "this tall" rather than "as tall as the content".
        let mut fixed = ElementTree::new();
        let fixed_root = root(&mut fixed);
        let mut element = area(f64::from(box_width), content);
        element.set("height", Value::Length(Length::Dp(30.0)));
        let boxed = fixed.insert(element);
        fixed.append_child(fixed_root, boxed);
        layout_with_text(&mut fixed, VIEWPORT, Some(&mut text));
        assert_eq!(f64_of(&fixed, boxed, "height"), 30.0);
    }

    /// Taffy rounds the computed layout to whole pixels, so a fractional
    /// measurement arrives rounded. Asserting through this keeps the test
    /// about the *decision* (one line or two, inset counted once) rather
    /// than about that rounding.
    fn close(value: f64) -> f64 {
        return value.round();
    }

    #[test]
    fn a_declared_padding_is_not_double_counted() {
        // `padding = 8dp` is the same inset the painter defaults to, so the
        // measure must add nothing of its own: the height is the wrapped
        // content plus exactly one inset's worth.
        let mut text = nui_text::TextSystem::with_embedded_font();
        let content = "alpha beta";
        let (full, line_height) = text.measure(content, DEFAULT_FONT_SIZE_DP);
        let box_width = full + 24.0;

        let mut tree = ElementTree::new();
        let root_id = root(&mut tree);
        let mut element = area(f64::from(box_width), content);
        element.set("padding", Value::Length(Length::Dp(8.0)));
        let field = tree.insert(element);
        tree.append_child(root_id, field);
        layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        // Content box = 400 - 16... no: the width is declared, so the
        // content box is `box_width - 16`, and the wrap happens there with
        // no extra inset. One line fits, and the padding is added once.
        assert_eq!(
            f64_of(&tree, field, "height"),
            close(f64::from(line_height) + 16.0),
            "one line plus the declared padding, counted once"
        );
    }

    #[test]
    fn a_content_sized_field_keeps_measuring_on_later_passes() {
        // The host lays out every frame, and `write_back` puts the computed
        // box into the element's own `height` slot. Without a record of
        // *why* that slot holds a number, the second pass would read its
        // own output as a declared height and the field would freeze at the
        // height it happened to have on the first frame — the bug this
        // test exists for.
        let mut text = nui_text::TextSystem::with_embedded_font();
        let mut tree = ElementTree::new();
        let root_id = root(&mut tree);
        let field = tree.insert(area(200.0, "one"));
        tree.append_child(root_id, field);

        layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        let one = f64_of(&tree, field, "height");
        // Three more passes with nothing changed: still one line, not zero
        // and not whatever rounding the previous pass left behind.
        for _ in 0..3 {
            layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        }
        assert_eq!(f64_of(&tree, field, "height"), one, "stable across passes");

        // Now the content changes: the box must follow it, on a *later*
        // pass (the host does not re-instantiate the tree to relayout).
        tree.arena[field].set("text", Value::String("one\ntwo\nthree".to_string()));
        layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        let three = f64_of(&tree, field, "height");
        assert!(
            three > one * 2.0,
            "three lines must be more than twice one: {one} -> {three}"
        );
    }

    #[test]
    fn a_declared_height_still_wins_on_later_passes() {
        // The other half of the same distinction: a declared height is a
        // request, so it must survive the pass that writes the box back.
        let mut text = nui_text::TextSystem::with_embedded_font();
        let mut tree = ElementTree::new();
        let root_id = root(&mut tree);
        let mut element = area(200.0, "one\ntwo\nthree");
        element.set("height", Value::Length(Length::Dp(30.0)));
        let field = tree.insert(element);
        tree.append_child(root_id, field);
        for _ in 0..3 {
            layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        }
        assert_eq!(f64_of(&tree, field, "height"), 30.0);
    }

    #[test]
    fn a_one_line_field_keeps_its_box_from_the_document() {
        // The M7 contract: a single-line field is not content-sized. Giving
        // it a measure now would silently resize every existing form.
        let mut text = nui_text::TextSystem::with_embedded_font();
        let mut tree = ElementTree::new();
        let root_id = root(&mut tree);
        let mut element = Element::new("TextInput", None);
        element.set("text", Value::String("hello".to_string()));
        let field = tree.insert(element);
        tree.append_child(root_id, field);
        layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        assert_eq!(f64_of(&tree, field, "width"), 400.0, "stretched");
        assert_eq!(f64_of(&tree, field, "height"), 0.0, "no intrinsic height");
    }

    #[test]
    fn visual_lines_partition_the_text_and_carry_caret_offsets() {
        let mut text = nui_text::TextSystem::with_embedded_font();
        let content = "alpha beta gamma delta";
        let (full, _) = text.measure(content, DEFAULT_FONT_SIZE_DP);
        let lines = visual_lines(&mut text, content, DEFAULT_FONT_SIZE_DP, full / 2.0);

        assert!(lines.len() >= 2, "narrow enough to wrap: {lines:?}");
        assert_eq!(lines[0].start, 0);
        assert_eq!(
            lines.last().map(|line| return line.end),
            Some(content.chars().count())
        );
        for pair in lines.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "no gap between lines");
        }
        for line in &lines {
            assert_eq!(
                line.caret_x.len(),
                line.end - line.start + 1,
                "one caret per char, plus the line's end"
            );
            let mut previous = -1.0_f32;
            for x in &line.caret_x {
                assert!(*x >= previous, "carets advance left to right");
                previous = *x;
            }
        }

        // The editing core can walk the table: a caret at the end of the
        // first line lands on the second, and coming back up lands in the
        // same column. The x is only near-identical, not identical —
        // proportional widths make "nearest caret" a many-to-one match, so
        // a round trip may settle on a neighbouring char of the same column.
        let mut state = TextInputState::from_text(content);
        let from = lines[0].end - 1;
        let column = lines[0].caret_x[from - lines[0].start];
        state.cursor = from;
        assert!(state.move_vertical(&lines, false, false));
        assert!(
            state.cursor >= lines[1].start && state.cursor < lines[1].end,
            "landed on the second line, got {}",
            state.cursor
        );
        assert!(state.move_vertical(&lines, true, false));
        assert!(
            state.cursor >= lines[0].start && state.cursor < lines[0].end,
            "and back on the first, got {}",
            state.cursor
        );
        let returned = lines[0].caret_x[state.cursor - lines[0].start];
        assert!(
            (returned - column).abs() < 14.0,
            "the visual column survives: {returned} vs {column}"
        );
    }

    #[test]
    fn visual_lines_count_hard_breaks_too() {
        let mut text = nui_text::TextSystem::with_embedded_font();
        let lines = visual_lines(&mut text, "one\ntwo\nthree", DEFAULT_FONT_SIZE_DP, 400.0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].end, 4, "the newline closes the line it ends");
        assert_eq!(lines[1].start, 4);
        assert_eq!(lines[2].end, "one\ntwo\nthree".chars().count());
        // An empty field is one empty line, not zero lines: the caret has
        // to be somewhere.
        let empty = visual_lines(&mut text, "", DEFAULT_FONT_SIZE_DP, 400.0);
        assert_eq!(empty.len(), 1);
        assert_eq!((empty[0].start, empty[0].end), (0, 0));
        assert_eq!(empty[0].caret_x, vec![0.0]);
    }
}

/// Container behaviour added in FUTURE 批次 5: `Grid`, `Wrap`, `Stack`,
/// the alignment properties, and the two leaves that exist to be laid out
/// (`Separator`, `Spacer`).
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod container_tests {
    use super::*;
    use nui_core::Length;
    use nui_runtime::Element;

    const VIEWPORT: nui_core::Size = nui_core::Size {
        width: 400.0,
        height: 300.0,
    };

    /// A root container of the given type, sized to the viewport unless it
    /// is a `Stack` (which is content-sized on purpose).
    fn root(tree: &mut ElementTree, ty: &str) -> ElementId {
        let mut element = Element::new(ty, None);
        if ty != "Stack" {
            element.set("width", Value::Length(Length::Dp(VIEWPORT.width)));
            element.set("height", Value::Length(Length::Dp(VIEWPORT.height)));
        }
        let id = tree.insert(element);
        tree.push_root(id);
        return id;
    }

    /// A fixed-size leaf.
    fn box_of(tree: &mut ElementTree, width: f32, height: f32) -> ElementId {
        let mut element = Element::new("Rectangle", None);
        element.set("width", Value::Length(Length::Dp(width)));
        element.set("height", Value::Length(Length::Dp(height)));
        return tree.insert(element);
    }

    fn set(tree: &mut ElementTree, id: ElementId, name: &str, value: Value) {
        tree.arena[id].set(name, value);
    }

    fn number(tree: &ElementTree, id: ElementId, name: &str) -> f64 {
        return match tree.arena[id].get(name) {
            Some(Value::Float(inner)) => *inner,
            Some(Value::Int(inner)) => *inner as f64,
            other => panic!("expected a numeric {name}, got {other:?}"),
        };
    }

    fn at(tree: &ElementTree, id: ElementId) -> (f64, f64) {
        return (number(tree, id, "x"), number(tree, id, "y"));
    }

    fn size_of(tree: &ElementTree, id: ElementId) -> (f64, f64) {
        return (number(tree, id, "width"), number(tree, id, "height"));
    }

    #[test]
    fn grid_places_children_in_rows_of_n_columns() {
        let mut tree = ElementTree::new();
        let grid = root(&mut tree, "Grid");
        set(&mut tree, grid, "columns", Value::Int(2));
        set(
            &mut tree,
            grid,
            "column_spacing",
            Value::Length(Length::Dp(10.0)),
        );
        set(
            &mut tree,
            grid,
            "row_spacing",
            Value::Length(Length::Dp(20.0)),
        );
        let cells: Vec<ElementId> = (0..4)
            .map(|_| return box_of(&mut tree, 30.0, 20.0))
            .collect();
        for cell in &cells {
            tree.append_child(grid, *cell);
        }
        layout(&mut tree, VIEWPORT);

        // Two equal tracks: (400 - 10) / 2 = 195 each. A child that
        // declares its own size keeps it — only auto-sized items stretch.
        assert_eq!(at(&tree, cells[0]), (0.0, 0.0));
        assert_eq!(at(&tree, cells[1]), (205.0, 0.0));
        assert_eq!(
            at(&tree, cells[2]),
            (0.0, 40.0),
            "second row clears the 20dp row gap plus the 20dp child"
        );
        assert_eq!(at(&tree, cells[3]), (205.0, 40.0));
        for cell in cells {
            assert_eq!(
                size_of(&tree, cell),
                (30.0, 20.0),
                "a declared size survives the grid"
            );
        }
    }

    #[test]
    fn a_grid_child_without_a_width_fills_its_track() {
        let mut tree = ElementTree::new();
        let grid = root(&mut tree, "Grid");
        set(&mut tree, grid, "columns", Value::Int(2));
        let mut only_height = Element::new("Rectangle", None);
        only_height.set("height", Value::Length(Length::Dp(20.0)));
        let id = tree.insert(only_height);
        tree.append_child(grid, id);
        layout(&mut tree, VIEWPORT);
        // Track width is 400 / 2; the auto-width child stretches to it.
        assert_eq!(size_of(&tree, id), (200.0, 20.0));
    }

    #[test]
    fn a_grid_without_columns_is_a_single_column() {
        let mut tree = ElementTree::new();
        let grid = root(&mut tree, "Grid");
        let first = box_of(&mut tree, 30.0, 20.0);
        let second = box_of(&mut tree, 30.0, 20.0);
        tree.append_child(grid, first);
        tree.append_child(grid, second);
        layout(&mut tree, VIEWPORT);
        assert_eq!(at(&tree, first), (0.0, 0.0));
        assert_eq!(at(&tree, second), (0.0, 20.0), "no columns means one track");
    }

    #[test]
    fn wrap_moves_overflowing_children_to_the_next_line() {
        let mut tree = ElementTree::new();
        let wrap = root(&mut tree, "Wrap");
        set(&mut tree, wrap, "align", Value::Enum("start".to_string()));
        let first = box_of(&mut tree, 150.0, 20.0);
        let second = box_of(&mut tree, 150.0, 20.0);
        let third = box_of(&mut tree, 150.0, 20.0);
        tree.append_child(wrap, first);
        tree.append_child(wrap, second);
        tree.append_child(wrap, third);
        layout(&mut tree, VIEWPORT);

        // 150 + 150 fits in 400; the third 150 does not and wraps.
        assert_eq!(at(&tree, first), (0.0, 0.0));
        assert_eq!(at(&tree, second), (150.0, 0.0));
        assert_eq!(at(&tree, third), (0.0, 20.0), "third child starts line two");
    }

    #[test]
    fn a_row_squeezes_children_instead_of_wrapping() {
        // The contrast that makes `Wrap` worth having: a `Row` keeps one
        // line, so the third child overflows to the right (Qt-Quick
        // semantics: elements are not shrunk, they overflow).
        let mut tree = ElementTree::new();
        let row = root(&mut tree, "Row");
        for _ in 0..3 {
            let child = box_of(&mut tree, 150.0, 20.0);
            tree.append_child(row, child);
        }
        layout(&mut tree, VIEWPORT);
        let children = tree.arena[row].children.clone();
        assert_eq!(at(&tree, children[2]), (300.0, 0.0));
    }

    #[test]
    fn a_stack_overlaps_its_children_at_the_same_origin() {
        let mut tree = ElementTree::new();
        let stack = root(&mut tree, "Stack");
        set(&mut tree, stack, "align", Value::Enum("start".to_string()));
        set(
            &mut tree,
            stack,
            "justify",
            Value::Enum("start".to_string()),
        );
        let big = box_of(&mut tree, 60.0, 30.0);
        let small = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(stack, big);
        tree.append_child(stack, small);
        layout(&mut tree, VIEWPORT);

        assert_eq!(at(&tree, big), (0.0, 0.0));
        assert_eq!(at(&tree, small), (0.0, 0.0), "both children share the cell");
        // The stack sizes to its largest child (it declares no size).
        assert_eq!(size_of(&tree, stack), (60.0, 30.0));
    }

    #[test]
    fn a_stack_stretches_an_unsized_child_over_the_cell() {
        let mut tree = ElementTree::new();
        let stack = root(&mut tree, "Stack");
        let big = box_of(&mut tree, 60.0, 30.0);
        // No size of its own: the default alignment stretches it over the
        // cell, which is what makes a stack a container rather than a
        // free-floating overlay.
        let stretchy = tree.insert(Element::new("Rectangle", None));
        tree.append_child(stack, big);
        tree.append_child(stack, stretchy);
        layout(&mut tree, VIEWPORT);
        assert_eq!(size_of(&tree, stretchy), (60.0, 30.0));
    }

    #[test]
    fn a_stack_alignment_pins_a_child_to_a_corner() {
        let mut tree = ElementTree::new();
        let stack = root(&mut tree, "Stack");
        set(&mut tree, stack, "align", Value::Enum("end".to_string()));
        set(&mut tree, stack, "justify", Value::Enum("end".to_string()));
        let big = box_of(&mut tree, 60.0, 30.0);
        let badge = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(stack, big);
        tree.append_child(stack, badge);
        layout(&mut tree, VIEWPORT);
        // Bottom-right of the 60x30 cell: this is the corner-badge recipe.
        assert_eq!(at(&tree, badge), (40.0, 20.0));
    }

    #[test]
    fn align_and_justify_place_a_row_child() {
        let mut tree = ElementTree::new();
        let row = root(&mut tree, "Row");
        set(&mut tree, row, "align", Value::Enum("center".to_string()));
        set(&mut tree, row, "justify", Value::Enum("center".to_string()));
        let child = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(row, child);
        layout(&mut tree, VIEWPORT);
        // Centred in 400x300: (400-20)/2 = 190, (300-10)/2 = 145.
        assert_eq!(at(&tree, child), (190.0, 145.0));
    }

    #[test]
    fn space_between_pushes_children_apart() {
        let mut tree = ElementTree::new();
        let row = root(&mut tree, "Row");
        set(
            &mut tree,
            row,
            "justify",
            Value::Enum("space-between".to_string()),
        );
        set(&mut tree, row, "align", Value::Enum("start".to_string()));
        let first = box_of(&mut tree, 20.0, 10.0);
        let second = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(row, first);
        tree.append_child(row, second);
        layout(&mut tree, VIEWPORT);
        assert_eq!(at(&tree, first), (0.0, 0.0));
        assert_eq!(
            at(&tree, second).0,
            380.0,
            "the last child ends at the edge"
        );
    }

    #[test]
    fn gap_is_an_alias_for_spacing() {
        let mut tree = ElementTree::new();
        let column = root(&mut tree, "Column");
        set(&mut tree, column, "gap", Value::Length(Length::Dp(12.0)));
        let first = box_of(&mut tree, 20.0, 10.0);
        let second = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(column, first);
        tree.append_child(column, second);
        layout(&mut tree, VIEWPORT);
        assert_eq!(at(&tree, second).1, 22.0);
    }

    #[test]
    fn a_spacer_eats_the_free_space() {
        let mut tree = ElementTree::new();
        let column = root(&mut tree, "Column");
        let header = box_of(&mut tree, 20.0, 20.0);
        let spacer = tree.insert(Element::new("Spacer", None));
        let footer = box_of(&mut tree, 20.0, 20.0);
        tree.append_child(column, header);
        tree.append_child(column, spacer);
        tree.append_child(column, footer);
        layout(&mut tree, VIEWPORT);

        // 300 tall, two 20dp boxes: the spacer takes the remaining 260.
        assert_eq!(number(&tree, spacer, "height"), 260.0);
        assert_eq!(
            at(&tree, footer),
            (0.0, 280.0),
            "the footer is pushed to the bottom"
        );
    }

    #[test]
    fn an_explicit_flex_grow_overrides_the_default() {
        let mut tree = ElementTree::new();
        let column = root(&mut tree, "Column");
        let spacer = tree.insert(Element::new("Spacer", None));
        set(&mut tree, spacer, "flex_grow", Value::Float(0.0));
        let filler = box_of(&mut tree, 20.0, 20.0);
        set(&mut tree, filler, "flex_grow", Value::Float(1.0));
        tree.append_child(column, spacer);
        tree.append_child(column, filler);
        layout(&mut tree, VIEWPORT);
        assert_eq!(number(&tree, spacer, "height"), 0.0);
        assert_eq!(number(&tree, filler, "height"), 300.0);
    }

    #[test]
    fn a_separator_is_a_hairline_that_spans_its_parent() {
        let mut tree = ElementTree::new();
        let column = root(&mut tree, "Column");
        let above = box_of(&mut tree, 20.0, 20.0);
        let separator = tree.insert(Element::new("Separator", None));
        let below = box_of(&mut tree, 20.0, 20.0);
        tree.append_child(column, above);
        tree.append_child(column, separator);
        tree.append_child(column, below);
        layout(&mut tree, VIEWPORT);

        assert_eq!(size_of(&tree, separator), (400.0, 1.0));
        assert_eq!(at(&tree, separator), (0.0, 20.0));
        assert_eq!(at(&tree, below), (0.0, 21.0));
    }

    #[test]
    fn a_vertical_separator_spans_the_height_instead() {
        let mut tree = ElementTree::new();
        let row = root(&mut tree, "Row");
        let separator = tree.insert(Element::new("Separator", None));
        set(
            &mut tree,
            separator,
            "orientation",
            Value::Enum("vertical".to_string()),
        );
        set(
            &mut tree,
            separator,
            "thickness",
            Value::Length(Length::Dp(2.0)),
        );
        tree.append_child(row, separator);
        layout(&mut tree, VIEWPORT);
        assert_eq!(size_of(&tree, separator), (2.0, 300.0));
    }

    #[test]
    fn a_titled_panel_starts_its_children_below_the_title_bar() {
        let mut tree = ElementTree::new();
        let panel = root(&mut tree, "Panel");
        set(&mut tree, panel, "padding", Value::Length(Length::Dp(16.0)));
        set(
            &mut tree,
            panel,
            "title",
            Value::String("Details".to_string()),
        );
        let child = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(panel, child);
        layout(&mut tree, VIEWPORT);

        // 16dp padding + (16dp title + 2 x 8dp gaps) = 48.
        assert_eq!(at(&tree, child), (16.0, 48.0));
    }

    #[test]
    fn a_panel_without_a_title_uses_its_padding_alone() {
        let mut tree = ElementTree::new();
        let panel = root(&mut tree, "Panel");
        let child = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(panel, child);
        layout(&mut tree, VIEWPORT);
        // `Panel`'s default padding is 16dp; no title, so no inset.
        assert_eq!(at(&tree, child), (16.0, 16.0));
    }

    #[test]
    fn a_bigger_title_font_pushes_the_body_further_down() {
        let mut tree = ElementTree::new();
        let panel = root(&mut tree, "Panel");
        set(
            &mut tree,
            panel,
            "font.size",
            Value::Length(Length::Dp(24.0)),
        );
        set(
            &mut tree,
            panel,
            "title",
            Value::String("Details".to_string()),
        );
        let child = box_of(&mut tree, 20.0, 10.0);
        tree.append_child(panel, child);
        layout(&mut tree, VIEWPORT);
        assert_eq!(at(&tree, child).1, 16.0 + 24.0 + 16.0);
    }

    #[test]
    fn a_dialog_lays_its_body_out_as_a_column_below_the_title() {
        // A dialog used to be a leaf with children: its content landed
        // wherever taffy's default row put it, on top of the title.
        let mut tree = ElementTree::new();
        let mut dialog = Element::new("Dialog", None);
        dialog.set("open", Value::Bool(true));
        dialog.set("title", Value::String("Confirm".to_string()));
        dialog.set("width", Value::Length(Length::Dp(320.0)));
        dialog.set("height", Value::Length(Length::Dp(170.0)));
        let dialog_id = tree.insert(dialog);
        tree.push_root(dialog_id);
        let first = box_of(&mut tree, 30.0, 10.0);
        let second = box_of(&mut tree, 30.0, 10.0);
        tree.append_child(dialog_id, first);
        tree.append_child(dialog_id, second);
        layout(&mut tree, VIEWPORT);

        // Centred 320x170 panel: x = (400-320)/2 = 40, y = (300-170)/2 = 65.
        assert_eq!(at(&tree, dialog_id), (40.0, 65.0));
        // Children stack below the title inset (20 padding + 16 + 16).
        assert_eq!(at(&tree, first), (60.0, 117.0));
        assert_eq!(at(&tree, second), (60.0, 127.0));
    }

    #[test]
    fn align_self_overrides_the_container_for_one_child() {
        // The corner-badge recipe: the stack stretches its children, and
        // the badge asks to be pinned instead.
        let mut tree = ElementTree::new();
        let stack = root(&mut tree, "Stack");
        set(&mut tree, stack, "width", Value::Length(Length::Dp(60.0)));
        set(&mut tree, stack, "height", Value::Length(Length::Dp(40.0)));
        let disc = tree.insert(Element::new("Rectangle", None));
        let badge = box_of(&mut tree, 20.0, 10.0);
        set(
            &mut tree,
            badge,
            "align_self",
            Value::Enum("end".to_string()),
        );
        set(
            &mut tree,
            badge,
            "justify_self",
            Value::Enum("end".to_string()),
        );
        tree.append_child(stack, disc);
        tree.append_child(stack, badge);
        layout(&mut tree, VIEWPORT);

        assert_eq!(
            size_of(&tree, disc),
            (60.0, 40.0),
            "the disc takes the cell"
        );
        assert_eq!(at(&tree, badge), (40.0, 30.0), "the badge takes the corner");
    }

    #[test]
    fn align_self_stops_a_container_filling_its_parent() {
        // A `Column` stretched inside its parent can shrink-wrap instead.
        let mut tree = ElementTree::new();
        let outer = root(&mut tree, "Column");
        let inner = tree.insert(Element::new("Column", None));
        set(
            &mut tree,
            inner,
            "align_self",
            Value::Enum("start".to_string()),
        );
        set(&mut tree, inner, "width", Value::Length(Length::Dp(80.0)));
        set(&mut tree, inner, "height", Value::Length(Length::Dp(20.0)));
        let child = box_of(&mut tree, 10.0, 10.0);
        tree.append_child(outer, inner);
        tree.append_child(inner, child);
        layout(&mut tree, VIEWPORT);
        assert_eq!(size_of(&tree, inner), (80.0, 20.0));
        assert_eq!(at(&tree, inner), (0.0, 0.0));
    }

    #[test]
    fn window_stacks_its_children_vertically() {
        // The root is a column: a second child goes below the first
        // instead of beside it, and `padding`/`gap` apply to the document
        // root like any other container.
        let mut tree = ElementTree::new();
        let window = tree.insert(Element::new("Window", None));
        tree.push_root(window);
        set(
            &mut tree,
            window,
            "padding",
            Value::Length(Length::Dp(10.0)),
        );
        let first = box_of(&mut tree, 20.0, 30.0);
        let second = box_of(&mut tree, 20.0, 30.0);
        tree.append_child(window, first);
        tree.append_child(window, second);
        layout(&mut tree, VIEWPORT);
        assert_eq!(at(&tree, first), (10.0, 10.0));
        assert_eq!(at(&tree, second), (10.0, 40.0));
    }
}

/// Batch 7: margins, and what a `ListView` row does with its slot.
#[cfg(test)]
mod margin_tests {
    use super::*;
    use nui_core::Length;
    use nui_runtime::Element;

    const VIEWPORT: nui_core::Size = nui_core::Size {
        width: 400.0,
        height: 300.0,
    };

    /// A viewport-filling `Column` root.
    fn column() -> (ElementTree, ElementId) {
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", Value::Length(Length::Dp(VIEWPORT.width)));
        root.set("height", Value::Length(Length::Dp(VIEWPORT.height)));
        let id = tree.insert(root);
        tree.push_root(id);
        return (tree, id);
    }

    /// Appends a `height`-tall leaf, optionally carrying one margin
    /// property.
    fn leaf(
        tree: &mut ElementTree,
        parent: ElementId,
        height: f32,
        margin: Option<(&str, f32)>,
    ) -> ElementId {
        let mut element = Element::new("Rectangle", None);
        element.set("height", Value::Length(Length::Dp(height)));
        if let Some((name, value)) = margin {
            element.set(name, Value::Length(Length::Dp(value)));
        }
        let id = tree.insert(element);
        tree.append_child(parent, id);
        return id;
    }

    fn y_of(tree: &ElementTree, id: ElementId) -> f64 {
        return match tree.arena[id].get("y") {
            Some(Value::Float(inner)) => *inner,
            other => panic!("expected a numeric y, got {other:?}"),
        };
    }

    #[test]
    fn a_bottom_margin_holds_the_next_sibling_at_the_slot() {
        // The `ListView` row case: a 32dp card in a 36dp slot. The next
        // row has to start at 36, not at 32 — that difference is exactly
        // what the virtualizer's `row * row_height` arithmetic assumes.
        let (mut tree, root) = column();
        leaf(&mut tree, root, 32.0, Some(("margin_bottom", 4.0)));
        let second = leaf(&mut tree, root, 32.0, None);
        layout(&mut tree, VIEWPORT);
        assert_eq!(y_of(&tree, second), 36.0);
    }

    #[test]
    fn the_slot_holds_across_frames() {
        // `write_back` writes the measured box straight back into each
        // element's own `y`/`height` slots. A margin is not re-derived
        // from those, so the third frame must agree with the first — the
        // "only true on the first pass" trap this file keeps re-learning.
        let (mut tree, root) = column();
        leaf(&mut tree, root, 32.0, Some(("margin_bottom", 4.0)));
        let second = leaf(&mut tree, root, 32.0, None);
        layout(&mut tree, VIEWPORT);
        layout(&mut tree, VIEWPORT);
        layout(&mut tree, VIEWPORT);
        assert_eq!(y_of(&tree, second), 36.0, "three frames in");
    }

    #[test]
    fn a_side_margin_overrides_the_axis_which_overrides_margin() {
        let (mut tree, root) = column();
        let first = leaf(&mut tree, root, 10.0, Some(("margin_vertical", 5.0)));
        leaf(&mut tree, root, 10.0, None);
        // A per-side property beats `margin_vertical`, which beats
        // `margin`.
        let third = leaf(&mut tree, root, 10.0, Some(("margin_top", 1.0)));
        layout(&mut tree, VIEWPORT);
        assert_eq!(y_of(&tree, first), 5.0);
        // second starts at 5 + 10 + 5 = 20 and is 10 tall, so third starts
        // at 30 plus its own top margin — 1, overriding the axis' 5.
        assert_eq!(y_of(&tree, third), 31.0);
    }

    #[test]
    fn margin_sets_every_side() {
        let (mut tree, root) = column();
        let only = leaf(&mut tree, root, 10.0, Some(("margin", 7.0)));
        layout(&mut tree, VIEWPORT);
        assert_eq!(y_of(&tree, only), 7.0);
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use nui_core::Length;
    use nui_runtime::Element;

    const VIEWPORT: nui_core::Size = nui_core::Size {
        width: 400.0,
        height: 300.0,
    };
    const LEAF_HEIGHT: f32 = 20.0;

    /// A viewport-filling `Column` root.
    fn column() -> (ElementTree, ElementId) {
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", Value::Length(Length::Dp(VIEWPORT.width)));
        root.set("height", Value::Length(Length::Dp(VIEWPORT.height)));
        let id = tree.insert(root);
        tree.push_root(id);
        return (tree, id);
    }

    /// Appends a 20dp-tall leaf, hidden when `visible` is false.
    fn leaf(tree: &mut ElementTree, parent: ElementId, visible: bool) -> ElementId {
        let mut element = Element::new("Rectangle", None);
        element.set("height", Value::Length(Length::Dp(LEAF_HEIGHT)));
        element.set("visible", Value::Bool(visible));
        let id = tree.insert(element);
        tree.append_child(parent, id);
        return id;
    }

    fn number_of(tree: &ElementTree, id: ElementId, name: &str) -> f64 {
        return match tree.arena[id].get(name) {
            Some(Value::Float(inner)) => *inner,
            Some(Value::Int(inner)) => *inner as f64,
            Some(Value::Length(Length::Dp(inner))) => f64::from(*inner),
            other => panic!("expected a numeric {name}, got {other:?}"),
        };
    }

    #[test]
    fn a_hidden_element_takes_no_space() {
        let (mut tree, root) = column();
        let first = leaf(&mut tree, root, true);
        let hidden = leaf(&mut tree, root, false);
        let third = leaf(&mut tree, root, true);
        layout(&mut tree, VIEWPORT);
        assert_eq!(number_of(&tree, first, "y"), 0.0);
        assert_eq!(
            number_of(&tree, third, "y"),
            f64::from(LEAF_HEIGHT),
            "the hidden sibling left no gap behind it"
        );
        // No box in *layout* — the sibling's y is the proof, the zero
        // taffy produced never reaches the slot. `write_back` keeps the
        // hidden element's own `height` at its declared 20dp: writing the
        // zero would poison `layout.auto_height` and the element would
        // stay collapsed forever, even after it became visible again.
        assert_eq!(number_of(&tree, hidden, "height"), f64::from(LEAF_HEIGHT));
    }

    #[test]
    fn a_hidden_container_takes_its_children_with_it() {
        let (mut tree, root) = column();
        leaf(&mut tree, root, true);
        let mut group = Element::new("Column", None);
        group.set("visible", Value::Bool(false));
        let group_id = tree.insert(group);
        tree.append_child(root, group_id);
        let child = leaf(&mut tree, group_id, true);
        let last = leaf(&mut tree, root, true);
        layout(&mut tree, VIEWPORT);
        assert_eq!(
            number_of(&tree, last, "y"),
            f64::from(LEAF_HEIGHT),
            "the hidden group's own children took no room"
        );
        // The hidden child keeps its declared slot (see the note in
        // `write_back`); hiding is layout's job, not the slot's.
        assert_eq!(number_of(&tree, child, "height"), f64::from(LEAF_HEIGHT));
    }

    #[test]
    fn a_grid_stays_a_grid_when_it_is_visible() {
        // The `display: none` write happens *after* the arrangement sets
        // `Grid` / `Stack`, so a visible one must still arrive as a grid.
        // (Guards the ordering in `style_for`, not the flag itself.)
        let (mut tree, root) = column();
        let mut grid = Element::new("Grid", None);
        grid.set("columns", Value::Int(2));
        let grid_id = tree.insert(grid);
        tree.append_child(root, grid_id);
        leaf(&mut tree, grid_id, true);
        let second = leaf(&mut tree, grid_id, true);
        layout(&mut tree, VIEWPORT);
        assert_eq!(
            number_of(&tree, second, "y"),
            0.0,
            "two columns means one row"
        );
    }

    #[test]
    fn visibility_holds_across_frames() {
        // `write_back` copies the measured box back into each element's
        // own slots — *except* for hidden subtrees, whose slots would
        // otherwise record the zero taffy gives them, and a zero read
        // back as a declaration collapses the element for good ("shown
        // with a zero size" was exactly that trap). So across frames the
        // hidden leaf keeps its declared 20dp and comes back intact.
        let (mut tree, root) = column();
        let hidden = leaf(&mut tree, root, false);
        let shown = leaf(&mut tree, root, true);
        layout(&mut tree, VIEWPORT);
        layout(&mut tree, VIEWPORT);
        layout(&mut tree, VIEWPORT);
        assert_eq!(number_of(&tree, shown, "y"), 0.0, "three frames in");
        assert_eq!(number_of(&tree, hidden, "height"), f64::from(LEAF_HEIGHT));
    }
}
