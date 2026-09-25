//! nui-layout: taffy (flex) adapter, dp conversion, and geometry write-back.
//!
//! Mapping (plan §5): `Column`/`Row` map to flex containers with `spacing`
//! as gap and `padding` as padding; `width`/`height` properties map to
//! taffy dimensions (dp values pass through, `%` maps to percentage,
//! `auto`/absent maps to auto). Layout results write back to the element
//! properties `x`/`y`/`width`/`height`, readable by bindings (plan §5
//! geometry write-back with the iteration cap handled by the caller).

use taffy::prelude::*;

use nui_core::Value;
use nui_runtime::element::{Element, ElementId, ElementTree};

/// Default font size (dp) for `Text` elements without an explicit
/// `font.size` attached property.
const DEFAULT_FONT_SIZE_DP: f32 = 16.0;

/// Intrinsic content size of a `Text` element, computed once per layout
/// pass and handed to taffy as the leaf node's measure context.
type TextMeasure = Option<taffy::Size<f32>>;

/// Flex direction of a container node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Vertical stacking (`Column`).
    Vertical,
    /// Horizontal stacking (`Row`).
    Horizontal,
}

impl Axis {
    /// Maps a node type name to its axis; `None` for non-containers.
    pub fn from_type_name(name: &str) -> Option<Axis> {
        return match name {
            "Column" => Some(Axis::Vertical),
            "Row" => Some(Axis::Horizontal),
            // `For` rows and `Scroll` viewports stack vertically.
            "For" | "Scroll" | "ListView" => Some(Axis::Vertical),
            _ => None,
        };
    }

    fn to_taffy(self) -> taffy::FlexDirection {
        return match self {
            Axis::Vertical => taffy::FlexDirection::Column,
            Axis::Horizontal => taffy::FlexDirection::Row,
        };
    }
}

/// Reads a dp `f32` property with a fallback.
fn f32_property(element: &Element, name: &str, fallback: f32) -> f32 {
    return element
        .get(name)
        .and_then(|value| match value {
            Value::Int(inner) => return Some(*inner as f32),
            Value::Float(inner) => return Some(*inner as f32),
            Value::Length(nui_core::Length::Dp(inner)) => return Some(*inner),
            _ => return None,
        })
        .unwrap_or(fallback);
}

/// Maps a `width`/`height` property to a taffy dimension.
fn dimension(element: &Element, name: &str) -> TaffyDimension {
    let Some(value) = element.get(name) else {
        return TaffyDimension::auto();
    };
    return match value {
        Value::Length(nui_core::Length::Dp(dp)) => TaffyDimension::length(dp.max(0.0)),
        Value::Length(nui_core::Length::Percent(percent)) => {
            TaffyDimension::percent(percent.max(0.0) / 100.0)
        }
        Value::Length(nui_core::Length::Auto) => TaffyDimension::auto(),
        Value::Int(inner) => TaffyDimension::length(*inner as f32),
        Value::Float(inner) => TaffyDimension::length(*inner as f32),
        _ => TaffyDimension::auto(),
    };
}

/// Internal alias keeping the mapping function readable.
type TaffyDimension = taffy::prelude::Dimension;

/// Builds the taffy style for one element from its properties.
fn style_for(element: &Element) -> Style {
    let axis = Axis::from_type_name(&element.ty);
    let padding = f32_property(element, "padding", 0.0);
    let width = dimension(element, "width");
    let height = dimension(element, "height");
    let (size, align_self) = if axis.is_some() {
        // Containers default to filling the cross axis and stretching
        // children, so percent-sized inner elements resolve against real
        // space (matches QML's default anchoring feel).
        let resolved_width = if width == taffy::prelude::Dimension::auto() {
            taffy::prelude::Dimension::percent(1.0)
        } else {
            width
        };
        (
            taffy::Size {
                width: resolved_width,
                height,
            },
            Some(taffy::style::AlignSelf::Stretch),
        )
    } else {
        (taffy::Size { width, height }, None)
    };
    let mut style = Style {
        padding: taffy::Rect {
            left: length_value(padding),
            right: length_value(padding),
            top: length_value(padding),
            bottom: length_value(padding),
        },
        size,
        align_self,
        // Qt-Quick semantics: elements keep their natural size and overflow
        // their container (virtualized list content relies on this).
        flex_shrink: 0.0,
        ..Style::default()
    };
    let Some(axis) = axis else {
        return style;
    };
    style.flex_direction = axis.to_taffy();
    let spacing = f32_property(element, "spacing", 0.0);
    style.gap = taffy::Size {
        width: length_value(spacing),
        height: length_value(spacing),
    };
    return style;
}

fn length_value(dp: f32) -> taffy::style::LengthPercentage {
    return taffy::style::LengthPercentage::length(dp);
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

/// Lays out the tree with `viewport` as the root constraint and writes the
/// results back to the elements' `x`/`y`/`width`/`height` properties (dp).
/// Returns the visited element count.
pub fn layout(tree: &mut ElementTree, viewport: nui_core::Size) -> usize {
    return layout_with_text(tree, viewport, None);
}

/// Lays out the tree with intrinsic `Text` sizing: `text` shapes each
/// `Text` element's content (plan §6.2 paragraph measurement integrated
/// with layout); `None` sizes `Text` leaves purely from the container.
pub fn layout_with_text(
    tree: &mut ElementTree,
    viewport: nui_core::Size,
    text: Option<&mut nui_text::TextSystem>,
) -> usize {
    // Pre-measure every Text element (content, font size) once per pass.
    let mut text = text;
    let mut measure_element = |tree: &ElementTree, id: ElementId| -> TextMeasure {
        let element = &tree.arena[id];
        if element.ty != "Text" {
            return None;
        }
        let Some(text_system) = &mut text else {
            return None;
        };
        let content = element.get("content")?.as_str().ok()?;
        let font_size = element
            .get("font.size")
            .and_then(|value| return dp_of(value))
            .unwrap_or(DEFAULT_FONT_SIZE_DP);
        let (width, height) = text_system.measure(content, font_size);
        return Some(taffy::prelude::Size {
            width: width.max(1.0),
            height: height.max(1.0),
        });
    };

    let mut taffy_tree: taffy::TaffyTree<TextMeasure> = taffy::TaffyTree::new();
    let mut node_map: Vec<(ElementId, taffy::NodeId)> = Vec::new();

    // Create taffy nodes top-down.
    fn create_nodes(
        taffy_tree: &mut taffy::TaffyTree<TextMeasure>,
        tree: &ElementTree,
        id: ElementId,
        node_map: &mut Vec<(ElementId, taffy::NodeId)>,
        measure_element: &mut dyn FnMut(&ElementTree, ElementId) -> TextMeasure,
    ) -> taffy::NodeId {
        let element = &tree.arena[id];
        let style = style_for(element);
        let measure = measure_element(tree, id);
        let node = match measure {
            Some(size) => taffy_tree
                .new_leaf_with_context(style, Some(size))
                .expect("in-memory layout tree"),
            None => taffy_tree.new_leaf(style).expect("in-memory layout tree"),
        };
        node_map.push((id, node));
        let children: Vec<ElementId> = element.children.clone();
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

    let mut roots = Vec::new();
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
            |_known, _available, _node, context, _style| {
                // `context` is `Option<&mut TextMeasure>`; only leaves with
                // a measure context (Text elements) carry a size.
                let measure: TextMeasure = match context {
                    Some(inner) => *inner,
                    None => None,
                };
                return measure.unwrap_or(Size::ZERO);
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
    ) {
        let node = node_map
            .iter()
            .find(|(element, _)| return *element == id)
            .map(|(_, node)| return *node)
            .expect("every element has a taffy node");
        let layout = taffy_tree.layout(node).expect("computed layout");
        let x = base.0 + layout.location.x as f64;
        let y = base.1 + layout.location.y as f64;
        let width = layout.size.width as f64;
        let height = layout.size.height as f64;
        let element = &mut tree.arena[id];
        element.set("x", Value::Float(x));
        element.set("y", Value::Float(y));
        element.set("width", Value::Float(width));
        element.set("height", Value::Float(height));
        let children = element.children.clone();
        for child in children {
            write_back(tree, child, (x, y), taffy_tree, node_map);
        }
    }
    for root in tree.roots.clone() {
        write_back(tree, root, (0.0, 0.0), &taffy_tree, &node_map);
    }
    return node_map.len();
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
