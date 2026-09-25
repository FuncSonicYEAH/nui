//! Display-list scene: the flat draw data extracted from the element tree
//! each frame (plan §6: v1 rebuilds the render tree per frame; 万级节点无压力).
//!
//! The builder walks the tree pre-order so paint order matches document
//! order (later siblings paint on top, matching QML z semantics). M9 adds
//! the clip stack: `Scroll` elements (and anything with `clip = true`)
//! clip their subtree in the shader; scroll offsets translate the subtree.

use std::collections::HashMap;

use nui_core::{Color, Point, Rect, Size};
use nui_runtime::element::{ElementId, ElementTree};

use crate::image::ImageDraw;

/// Default font size (dp) for `Text` elements without `font.size`.
const DEFAULT_FONT_SIZE_DP: f32 = 16.0;

/// A rounded clip region in dp, resolved per draw (shader-side clipping).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipDraw {
    /// Clip bounds in dp (absolute, after scroll translation).
    pub bounds: Rect,
    /// Corner radius in dp.
    pub radius: f32,
}

impl ClipDraw {
    /// Intersects two clips; `None` inherits no clipping.
    pub fn intersect(self, other: ClipDraw) -> ClipDraw {
        let x0 = self.bounds.origin.x.max(other.bounds.origin.x);
        let y0 = self.bounds.origin.y.max(other.bounds.origin.y);
        let x1 = (self.bounds.origin.x + self.bounds.size.width)
            .min(other.bounds.origin.x + other.bounds.size.width);
        let y1 = (self.bounds.origin.y + self.bounds.size.height)
            .min(other.bounds.origin.y + other.bounds.size.height);
        return ClipDraw {
            bounds: Rect::new(
                Point::new(x0, y0),
                Size::new((x1 - x0).max(0.0), (y1 - y0).max(0.0)),
            ),
            radius: self.radius.min(other.radius),
        };
    }
}

/// One drawable text run's glyph quad (dp space; color premultiplied at
/// submission).
#[derive(Debug, Clone)]
pub struct TextDraw {
    /// Quad top-left in dp (element origin + glyph origin + mask offset).
    pub origin: Point,
    /// Quad size in dp (the mask's pixel size treated as dp).
    pub size: Size,
    /// Atlas mask rect `(x, y, width, height)` in texels.
    pub mask: (u32, u32, u32, u32),
    /// Atlas page index.
    pub page: u32,
    /// Tint color.
    pub color: Color,
    /// Inherited clip region, if any.
    pub clip: Option<ClipDraw>,
}

/// A soft shadow behind a rect (SDF approximation in the shader).
#[derive(Debug, Clone, Copy)]
pub struct Shadow {
    /// Offset in dp (positive = down/right).
    pub offset: Point,
    /// Softness in dp.
    pub blur: f32,
    /// Shadow color (alpha scaled by the rect's opacity).
    pub color: Color,
}

/// A linear gradient across the rect (shader-side interpolation).
///
/// The angle uses screen coordinates (y grows downward): `0` runs left to
/// right, `90` runs top to bottom. The gradient is defined in the rect's
/// own coordinate space, so it rotates with the element.
#[derive(Debug, Clone, Copy)]
pub struct GradientDraw {
    /// Color at the gradient's start edge.
    pub from: Color,
    /// Color at the gradient's end edge.
    pub to: Color,
    /// Direction in degrees (screen coordinates, 0 = rightward).
    pub angle: f32,
}

/// One drawable rounded rectangle.
#[derive(Debug, Clone)]
pub struct RectDraw {
    /// Geometry in dp.
    pub geometry: Rect,
    /// Corner radius in dp.
    pub corner_radius: f32,
    /// Fill color (ignored when `gradient` is set).
    pub fill: Color,
    /// Soft shadow, if any.
    pub shadow: Option<Shadow>,
    /// Inherited clip region, if any.
    pub clip: Option<ClipDraw>,
    /// Rotation in degrees, clockwise, around the rect's center. The
    /// layout box is unchanged; hit testing still uses the unrotated AABB.
    pub rotation: f32,
    /// Linear gradient overriding `fill`, if any. Border and shadow keep
    /// their own colors.
    pub gradient: Option<GradientDraw>,
}

/// Stroke end caps for polyline/arc stroking (FUTURE batch 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    /// Square ends exactly at the endpoints.
    Butt,
    /// Semicircular caps extending half the stroke width past the endpoints.
    Round,
}

/// One stroked polyline: rendered as a capsule SDF per segment
/// ([`crate::stroke`]). An `Arc` flattens into this shape at build time.
#[derive(Debug, Clone)]
pub struct PolylineDraw {
    /// Vertices in absolute dp (element-local points + element origin).
    pub points: Vec<Point>,
    /// Stroke width in dp.
    pub width: f32,
    /// End cap style.
    pub cap: LineCap,
    /// Stroke color (opacity folded in at build time).
    pub color: Color,
    /// Inherited clip region, if any.
    pub clip: Option<ClipDraw>,
}

/// One offscreen-composited layer: a subtree rendered into its own texture
/// and drawn back with a group opacity (and optional blur, M9).
#[derive(Debug, Clone)]
pub struct LayerDraw {
    /// Layer top-left in dp (absolute).
    pub origin: Point,
    /// Layer size in dp (the texture covers exactly this rect).
    pub size: Size,
    /// Group opacity (0..=1).
    pub opacity: f32,
    /// Gaussian blur radius in dp; 0 = sharp.
    pub blur: f32,
    /// The subtree's draws, texture-local.
    pub scene: Box<Scene>,
}

/// A frame's flat draw list.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Rects in paint order.
    pub rects: Vec<RectDraw>,
    /// Element each rect came from (hit testing / inspection).
    pub sources: Vec<ElementId>,
    /// Glyph quads in paint order (bucketed per atlas page by the renderer).
    pub texts: Vec<TextDraw>,
    /// Image draws in paint order (expanded to quads by the renderer).
    pub images: Vec<ImageDraw>,
    /// Stroked polylines (incl. flattened arcs) in paint order, expanded
    /// to per-segment capsule quads by the renderer.
    pub polylines: Vec<PolylineDraw>,
    /// Offscreen layers in paint order (drawn last, M9).
    pub layers: Vec<LayerDraw>,
}

/// Per-frame context the scene builder needs beyond the tree.
#[derive(Debug, Clone, Copy)]
pub struct SceneContext<'a> {
    /// The engine's focused element (drives the text cursor).
    pub focused: Option<ElementId>,
    /// Image path -> texture cache key ("path:content-hash").
    pub image_keys: &'a HashMap<String, String>,
}

/// Builds a [`Scene`] from the element tree.
#[derive(Debug, Default)]
pub struct SceneBuilder {
    rects: Vec<RectDraw>,
    sources: Vec<ElementId>,
    texts: Vec<TextDraw>,
    images: Vec<ImageDraw>,
    polylines: Vec<PolylineDraw>,
    layers: Vec<LayerDraw>,
}

impl SceneBuilder {
    /// Creates an empty builder.
    pub fn new() -> SceneBuilder {
        return SceneBuilder::default();
    }

    /// Pushes one rect draw.
    pub fn push_rect(&mut self, geometry: Rect, corner_radius: f32, fill: Color) {
        self.rects.push(RectDraw {
            geometry,
            corner_radius,
            fill,
            shadow: None,
            clip: None,
            rotation: 0.0,
            gradient: None,
        });
    }

    /// Builds the scene with per-frame context (focus, image keys).
    pub fn build_with_context(
        tree: &ElementTree,
        text: &mut nui_text::TextSystem,
        context: SceneContext<'_>,
    ) -> Scene {
        let mut builder = SceneBuilder::new();
        for root in tree.roots.clone() {
            builder.walk_element(tree, root, Point::ZERO, None, text, &context, false);
        }
        return builder.build();
    }

    /// Recursive pre-order walk: draws the element, then recurses into
    /// children with the accumulated scroll offset and clip region.
    // The parameters are the walk state (offset/clip/scope) carried down
    // the recursion; bundling them would obscure the flow.
    #[allow(clippy::too_many_arguments)]
    fn walk_element(
        &mut self,
        tree: &ElementTree,
        id: ElementId,
        offset: Point,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
        context: &SceneContext<'_>,
        layer_root: bool,
    ) {
        let element = &tree.arena[id];
        let width = f_property(element, "width").unwrap_or(0.0);
        let height = f_property(element, "height").unwrap_or(0.0);
        let x = f_property(element, "x").unwrap_or(0.0) + offset.x;
        let y = f_property(element, "y").unwrap_or(0.0) + offset.y;
        let bounds = Rect::new(Point::new(x, y), Size::new(width, height));

        // Layer capture (M9): `layer.opacity < 1` or `layer.blur > 0`
        // routes the whole subtree into an offscreen texture. The texture
        // covers the element rect, so overflowing content is clipped by
        // the texture bounds.
        let layer_opacity = f_property(element, "layer.opacity").unwrap_or(1.0);
        let layer_blur = f_property(element, "layer.blur").unwrap_or(0.0);
        if !layer_root && width > 0.0 && height > 0.0 && (layer_opacity < 1.0 || layer_blur > 0.0) {
            let mut sub = SceneBuilder::new();
            // Texture-local coordinates: the element's own rect starts at
            // (0, 0) inside the layer texture. `layer_root` suppresses
            // re-capturing the same element.
            let sub_offset = Point::new(offset.x - x, offset.y - y);
            sub.walk_element(tree, id, sub_offset, None, text, context, true);
            self.layers.push(LayerDraw {
                origin: Point::new(x, y),
                size: Size::new(width, height),
                opacity: layer_opacity.clamp(0.0, 1.0),
                blur: layer_blur,
                scene: Box::new(sub.build()),
            });
            return;
        }

        match element.ty.as_str() {
            "Text" => {
                self.collect_text_one(tree, id, element, x, y, clip, text);
            }
            "TextInput" => {
                self.collect_input_one(tree, id, element, x, y, clip, text, context.focused);
            }
            "Image" => {
                self.collect_image_one(id, element, bounds, clip, context.image_keys);
            }
            "Polyline" => {
                self.collect_polyline_one(element, x, y, clip);
            }
            "Arc" => {
                self.collect_arc_one(element, x, y, clip);
            }
            _ => {
                self.draw_rect(element, id, bounds, clip);
            }
        }

        // Children: scroll elements translate + clip; `clip = true` clips.
        let mut child_offset = offset;
        let mut child_clip = clip;
        if element.ty == "Scroll" || element.ty == "ListView" {
            let scroll_y = f_property(element, "scroll_y").unwrap_or(0.0);
            child_offset = Point::new(offset.x, offset.y - scroll_y);
            child_clip = push_clip(
                child_clip,
                bounds,
                f_property(element, "radius").unwrap_or(0.0),
            );
        } else if bool_property(element, "clip") {
            child_clip = push_clip(
                child_clip,
                bounds,
                f_property(element, "radius").unwrap_or(0.0),
            );
        }
        for child in element.children.clone() {
            self.walk_element(tree, child, child_offset, child_clip, text, context, false);
        }
    }

    /// Draws a plain rect (containers, panels); `Scroll` without a fill
    /// paints nothing (it is a viewport, not a surface).
    fn draw_rect(
        &mut self,
        element: &nui_runtime::Element,
        id: ElementId,
        bounds: Rect,
        clip: Option<ClipDraw>,
    ) {
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let opacity = f_property(element, "opacity")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        // A gradient alone is a valid surface (it overrides `fill`); only
        // when neither is set is the element a transparent container (Qt
        // Quick Item semantics): Window, Column, Row, Scroll and Spacer let
        // the host clear color show through. The placeholder fill here is
        // overridden by the gradient in the shader.
        let gradient_from = color_property(element, "gradient.from");
        let Some(fill) = color_property(element, "fill").or(gradient_from) else {
            return;
        };
        let corner_radius = f_property(element, "radius").unwrap_or(0.0);
        // Rotation (degrees, clockwise, around the rect center). Only plain
        // rect surfaces rotate; Text/Image/TextInput decorations keep their
        // axis-aligned placement in v1.
        let rotation = f_property(element, "rotation").unwrap_or(0.0);
        // Linear gradient: both endpoints must be present; `gradient.angle`
        // defaults to 90 (top-to-bottom, matching the screen y axis). The
        // element's opacity folds into both endpoints, like `fill`.
        let gradient = match (
            color_property(element, "gradient.from"),
            color_property(element, "gradient.to"),
        ) {
            (Some(from), Some(to)) => Some(GradientDraw {
                from: from.with_alpha(from.alpha() * opacity),
                to: to.with_alpha(to.alpha() * opacity),
                angle: f_property(element, "gradient.angle").unwrap_or(90.0),
            }),
            _ => None,
        };
        let shadow = color_property(element, "shadow.color").map(|color| {
            return Shadow {
                offset: Point::new(
                    f_property(element, "shadow.dx").unwrap_or(0.0),
                    f_property(element, "shadow.dy").unwrap_or(0.0),
                ),
                blur: f_property(element, "shadow.blur").unwrap_or(0.0),
                color: color.with_alpha(color.alpha() * opacity),
            };
        });
        self.rects.push(RectDraw {
            geometry: bounds,
            corner_radius,
            fill: fill.with_alpha(fill.alpha() * opacity),
            shadow,
            clip,
            rotation,
            gradient,
        });
        self.sources.push(id);
    }

    /// Extracts one `Text` element's glyph quads.
    #[allow(clippy::too_many_arguments)]
    fn collect_text_one(
        &mut self,
        _tree: &ElementTree,
        id: ElementId,
        element: &nui_runtime::Element,
        x: f32,
        y: f32,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
    ) {
        let width = f_property(element, "width").unwrap_or(0.0);
        let height = f_property(element, "height").unwrap_or(0.0);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let Some(content) = element
            .get("content")
            .and_then(|value| return value.as_str().ok())
        else {
            return;
        };
        let font_size = element
            .get("font.size")
            .and_then(|value| return dp_of(value))
            .unwrap_or(DEFAULT_FONT_SIZE_DP);
        let color = text_color_of(element);
        let opacity = f_property(element, "opacity")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let tint = color.with_alpha(color.alpha() * opacity);
        self.push_glyphs(text, content, font_size, x, y, tint, clip);
        let _ = id;
    }

    /// Extracts one `TextInput` element's visuals: background, text or
    /// placeholder, selection highlight, and the cursor when focused.
    #[allow(clippy::too_many_arguments)]
    fn collect_input_one(
        &mut self,
        _tree: &ElementTree,
        id: ElementId,
        element: &nui_runtime::Element,
        x: f32,
        y: f32,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
        focused: Option<ElementId>,
    ) {
        let width = f_property(element, "width").unwrap_or(0.0);
        let height = f_property(element, "height").unwrap_or(0.0);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let radius = f_property(element, "radius").unwrap_or(6.0);
        let fill = color_property(element, "fill")
            .unwrap_or_else(|| return Color::from_rgb8(0x26, 0x2b, 0x33));
        let opacity = f_property(element, "opacity")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        self.rects.push(RectDraw {
            geometry: Rect::new(Point::new(x, y), Size::new(width, height)),
            corner_radius: radius,
            fill: fill.with_alpha(fill.alpha() * opacity),
            shadow: None,
            clip,
            rotation: 0.0,
            gradient: None,
        });
        self.sources.push(id);

        let font_size = element
            .get("font.size")
            .and_then(|value| return dp_of(value))
            .unwrap_or(DEFAULT_FONT_SIZE_DP);
        let padding = f_property(element, "padding").unwrap_or(8.0);
        let line_height = font_size * 1.2;
        let text_color = color_property(element, "color").unwrap_or(Color::WHITE);
        let Some(state) = element.text_input.as_ref() else {
            return;
        };
        let baseline_y = y + (height - line_height) / 2.0;
        // Composition display (fcitx5/ibus/XIM): the preedit text is shown
        // inline at the cursor; the cursor moves to its end and an
        // underline marks the composed span.
        let composing = focused == Some(id) && !state.preedit.is_empty();
        let chars: Vec<char> = state.text.chars().collect();
        let display = if composing {
            let before: String = chars[..state.cursor].iter().collect();
            let after: String = chars[state.cursor..].iter().collect();
            format!("{before}{}{after}", state.preedit)
        } else {
            state.text.clone()
        };
        if display.is_empty() {
            let placeholder = element
                .get("placeholder")
                .and_then(|value| return value.as_str().ok())
                .unwrap_or("");
            if !placeholder.is_empty() {
                let dim = Color::from_rgb8(0x8a, 0x93, 0xa5).with_alpha(opacity);
                self.push_glyphs(
                    text,
                    placeholder,
                    font_size,
                    x + padding,
                    baseline_y,
                    dim,
                    clip,
                );
            }
        } else {
            let tint = text_color.with_alpha(text_color.alpha() * opacity);
            self.push_glyphs(
                text,
                &display,
                font_size,
                x + padding,
                baseline_y,
                tint,
                clip,
            );
        }
        if focused != Some(id) {
            return;
        }
        if composing {
            // Underline under the composed span.
            let preedit_chars = state.preedit.chars().count();
            let display_chars: Vec<char> = display.chars().collect();
            let underline_start =
                x + padding + measure_width(text, &display_chars[..state.cursor], font_size);
            let underline_end = x
                + padding
                + measure_width(
                    text,
                    &display_chars[..state.cursor + preedit_chars],
                    font_size,
                );
            self.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(underline_start, y + height * 0.82),
                    Size::new((underline_end - underline_start).max(2.0), 2.0),
                ),
                corner_radius: 0.0,
                fill: text_color.with_alpha(text_color.alpha() * opacity),
                shadow: None,
                clip,
                rotation: 0.0,
                gradient: None,
            });
        }
        // Selection highlight under the glyphs.
        if state.has_selection() {
            let (start, end) = state.selection();
            let chars: Vec<char> = state.text.chars().collect();
            let start_x = measure_width(text, &chars[..start], font_size);
            let end_x = measure_width(text, &chars[..end], font_size);
            let selection_color = Color::from_rgb8(0x33, 0x66, 0x99).with_alpha(0.5 * opacity);
            self.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(x + padding + start_x, baseline_y),
                    Size::new((end_x - start_x).max(1.0), line_height),
                ),
                corner_radius: 2.0,
                fill: selection_color,
                shadow: None,
                clip,
                rotation: 0.0,
                gradient: None,
            });
        }
        // Cursor: a 2dp caret, centered vertically; during composition it
        // sits after the preedit text.
        let cursor_chars = (state.cursor
            + if composing {
                state.preedit.chars().count()
            } else {
                0
            })
        .min(display.chars().count());
        let display_chars: Vec<char> = display.chars().collect();
        let cursor_x = x + padding + measure_width(text, &display_chars[..cursor_chars], font_size);
        self.rects.push(RectDraw {
            geometry: Rect::new(
                Point::new(cursor_x, y + height * 0.2),
                Size::new(2.0, height * 0.6),
            ),
            corner_radius: 1.0,
            fill: Color::WHITE.with_alpha(opacity),
            shadow: None,
            clip,
            rotation: 0.0,
            gradient: None,
        });
    }

    /// Extracts one `Image` draw.
    fn collect_image_one(
        &mut self,
        id: ElementId,
        element: &nui_runtime::Element,
        bounds: Rect,
        clip: Option<ClipDraw>,
        image_keys: &HashMap<String, String>,
    ) {
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let Some(source) = element
            .get("source")
            .and_then(|value| return value.as_str().ok())
        else {
            return;
        };
        let Some(key) = image_keys.get(source) else {
            return;
        };
        let tint = color_property(element, "tint").unwrap_or(Color::WHITE);
        let slice = f_property(element, "slice").unwrap_or(0.0);
        self.images.push(ImageDraw {
            geometry: bounds,
            tint,
            key: key.clone(),
            slice,
            clip,
        });
        let _ = id;
    }

    /// Extracts one `Polyline` element's stroke.
    fn collect_polyline_one(
        &mut self,
        element: &nui_runtime::Element,
        x: f32,
        y: f32,
        clip: Option<ClipDraw>,
    ) {
        let Some(text) = element
            .get("points")
            .and_then(|value| return value.as_str().ok())
        else {
            return;
        };
        let points = parse_points(text);
        let Some((width, cap, color)) = stroke_style_of(element) else {
            return;
        };
        self.push_stroke(points, width, cap, color, clip, Point::new(x, y));
    }

    /// Extracts one `Arc` element's stroke, flattening the arc into
    /// polyline points on the CPU (v1; no GPU tessellation yet).
    fn collect_arc_one(
        &mut self,
        element: &nui_runtime::Element,
        x: f32,
        y: f32,
        clip: Option<ClipDraw>,
    ) {
        let cx = f_property(element, "cx").unwrap_or(0.0);
        let cy = f_property(element, "cy").unwrap_or(0.0);
        let radius = f_property(element, "radius").unwrap_or(0.0);
        let start = f_property(element, "start").unwrap_or(0.0);
        let end = f_property(element, "end").unwrap_or(360.0);
        let points = arc_points(cx, cy, radius, start, end);
        let Some((width, cap, color)) = stroke_style_of(element) else {
            return;
        };
        self.push_stroke(points, width, cap, color, clip, Point::new(x, y));
    }

    /// Pushes one stroked polyline, offsetting element-local points by the
    /// element origin. Degenerate input (< 2 points) is dropped.
    fn push_stroke(
        &mut self,
        points: Vec<Point>,
        width: f32,
        cap: LineCap,
        color: Color,
        clip: Option<ClipDraw>,
        origin: Point,
    ) {
        if points.len() < 2 || width <= 0.0 {
            return;
        }
        let points: Vec<Point> = points
            .into_iter()
            .map(|point| return Point::new(point.x + origin.x, point.y + origin.y))
            .collect();
        self.polylines.push(PolylineDraw {
            points,
            width,
            cap,
            color,
            clip,
        });
    }

    /// Shapes one line of glyphs and pushes the quads.
    #[allow(clippy::too_many_arguments)]
    fn push_glyphs(
        &mut self,
        text: &mut nui_text::TextSystem,
        content: &str,
        font_size: f32,
        origin_x: f32,
        origin_y: f32,
        tint: Color,
        clip: Option<ClipDraw>,
    ) {
        let shaped = text.shape(content, font_size);
        for glyph in &shaped.glyphs {
            let Some(quad) = text.glyph_quad(&glyph.key) else {
                continue;
            };
            self.texts.push(TextDraw {
                origin: Point::new(
                    origin_x + glyph.x + quad.left as f32,
                    origin_y + glyph.y + quad.top as f32,
                ),
                size: Size::new(quad.slot.width as f32, quad.slot.height as f32),
                mask: (quad.slot.x, quad.slot.y, quad.slot.width, quad.slot.height),
                page: quad.slot.page,
                color: tint,
                clip,
            });
        }
    }

    /// Finalizes the scene.
    pub fn build(self) -> Scene {
        return Scene {
            rects: self.rects,
            sources: self.sources,
            texts: self.texts,
            images: self.images,
            polylines: self.polylines,
            layers: self.layers,
        };
    }
}

/// Pushes a child clip: intersects with the inherited one.
fn push_clip(inherited: Option<ClipDraw>, bounds: Rect, radius: f32) -> Option<ClipDraw> {
    let next = ClipDraw { bounds, radius };
    return match inherited {
        Some(current) => Some(current.intersect(next)),
        None => Some(next),
    };
}

/// The text color: `color` property first, `fill` fallback, white default.
fn text_color_of(element: &nui_runtime::Element) -> Color {
    return match color_property(element, "color") {
        Some(color) => color,
        None => match color_property(element, "fill") {
            Some(color) => color,
            None => Color::WHITE,
        },
    };
}

/// The pixel width of a char prefix at `font_size` (cursor/selection x).
fn measure_width(text: &mut nui_text::TextSystem, chars: &[char], font_size: f32) -> f32 {
    let prefix: String = chars.iter().collect();
    return text.measure(&prefix, font_size).0;
}

/// Extracts a dp f32 from a property value.
fn dp_of(value: &nui_core::Value) -> Option<f32> {
    return match value {
        nui_core::Value::Int(inner) => Some(*inner as f32),
        nui_core::Value::Float(inner) => Some(*inner as f32),
        nui_core::Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
        _ => None,
    };
}

/// Reads an `f32`-shaped property.
fn f_property(element: &nui_runtime::Element, name: &str) -> Option<f32> {
    return element.get(name).and_then(|value| {
        return match value {
            nui_core::Value::Int(inner) => Some(*inner as f32),
            nui_core::Value::Float(inner) => Some(*inner as f32),
            nui_core::Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
            _ => None,
        };
    });
}

/// Reads a color property.
fn color_property(element: &nui_runtime::Element, name: &str) -> Option<Color> {
    return element.get(name).and_then(|value| {
        return match value {
            nui_core::Value::Color(color) => Some(*color),
            _ => None,
        };
    });
}

/// Reads a bool property.
fn bool_property(element: &nui_runtime::Element, name: &str) -> bool {
    return element
        .get(name)
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(false);
}

/// Parses `"x,y x,y ..."` point pairs (whitespace between pairs, comma
/// within a pair). Malformed pairs are skipped.
fn parse_points(text: &str) -> Vec<Point> {
    let mut points = Vec::new();
    for pair in text.split_whitespace() {
        let Some((x, y)) = pair.split_once(',') else {
            continue;
        };
        if let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
            points.push(Point::new(x, y));
        }
    }
    return points;
}

/// Flattens an arc into polyline points. Screen coordinates: y grows down,
/// so a positive sweep runs clockwise. The segment count scales with the
/// sweep at ~4 degrees per segment, clamped to 8..96; a full circle drops
/// the duplicated closing point so no zero-length segment reaches the GPU.
fn arc_points(cx: f32, cy: f32, radius: f32, start_deg: f32, end_deg: f32) -> Vec<Point> {
    let sweep = end_deg - start_deg;
    if radius <= 0.0 || sweep == 0.0 {
        return Vec::new();
    }
    let segments = ((sweep.abs() / 4.0).ceil() as usize).clamp(8, 96);
    let mut points = Vec::with_capacity(segments + 1);
    for step in 0..=segments {
        let angle = (start_deg + sweep * step as f32 / segments as f32).to_radians();
        points.push(Point::new(
            cx + radius * angle.cos(),
            cy + radius * angle.sin(),
        ));
    }
    let closed = match (points.first().copied(), points.last().copied()) {
        (Some(first), Some(last)) => {
            (last.x - first.x).abs() < 1e-4 && (last.y - first.y).abs() < 1e-4
        }
        _ => false,
    };
    if closed {
        points.pop();
    }
    return points;
}

/// Reads the stroke style: `stroke.width` (dp) is required, `stroke.cap`
/// maps `"round"`/`"butt"` (default round), and the color comes from
/// `color` then `stroke.color` (white default) with the element's opacity
/// folded in.
fn stroke_style_of(element: &nui_runtime::Element) -> Option<(f32, LineCap, Color)> {
    let width = f_property(element, "stroke.width").unwrap_or(0.0);
    if width <= 0.0 {
        return None;
    }
    let cap = match element
        .get("stroke.cap")
        .and_then(|value| return value.as_str().ok())
    {
        Some("butt") => LineCap::Butt,
        _ => LineCap::Round,
    };
    let opacity = f_property(element, "opacity")
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let color = color_property(element, "color")
        .or_else(|| return color_property(element, "stroke.color"))
        .unwrap_or(Color::WHITE);
    return Some((width, cap, color.with_alpha(color.alpha() * opacity)));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Value;
    use nui_runtime::Element;

    fn sized_element(width: f32, height: f32) -> Element {
        let mut element = Element::new("Rectangle", None);
        element.set("x", Value::Float(5.0));
        element.set("y", Value::Float(6.0));
        element.set("width", Value::Length(nui_core::Length::Dp(width)));
        element.set("height", Value::Length(nui_core::Length::Dp(height)));
        element.set("fill", Value::Color(Color::from_rgb8(40, 40, 40)));
        return element;
    }

    #[test]
    fn sized_rect_becomes_draw_with_opacity_and_fill() {
        let mut tree = ElementTree::new();
        let mut element = sized_element(100.0, 50.0);
        element.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        element.set("opacity", Value::Float(0.5));
        element.set("radius", Value::Float(8.0));
        let id = tree.insert(element);
        tree.push_root(id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert_eq!(scene.rects.len(), 1);
        assert_eq!(scene.sources, vec![id]);
        let draw = &scene.rects[0];
        assert_eq!(draw.geometry.origin.x, 5.0);
        assert_eq!(draw.geometry.size.width, 100.0);
        assert_eq!(draw.corner_radius, 8.0);
        // Premultiplied by opacity at build time.
        assert!((draw.fill.alpha() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn zero_sized_and_transparent_rects_are_pruned() {
        let mut tree = ElementTree::new();
        let zero = tree.insert(sized_element(0.0, 50.0));
        let mut transparent = sized_element(10.0, 10.0);
        transparent.set("opacity", Value::Float(0.0));
        let transparent_id = tree.insert(transparent);
        let visible = tree.insert(sized_element(10.0, 10.0));
        for id in [zero, transparent_id, visible] {
            tree.push_root(id);
        }
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert_eq!(scene.rects.len(), 1);
        assert_eq!(scene.sources, vec![visible]);
    }

    #[test]
    fn fill_less_containers_paint_nothing() {
        // Qt Quick Item semantics: no `fill` = transparent, for every
        // container type (the host clear color shows through). Regression
        // guard for the old Color::BLACK fallback that made Window/Column
        // opaque black surfaces.
        let mut tree = ElementTree::new();
        let window = tree.insert(Element::new("Window", None));
        let mut column = Element::new("Column", None);
        column.set("width", Value::Float(100.0));
        column.set("height", Value::Float(50.0));
        let column_id = tree.insert(column);
        tree.append_child(window, column_id);
        tree.push_root(window);

        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert_eq!(scene.rects.len(), 0);
        assert_eq!(scene.sources.len(), 0);
    }

    #[test]
    fn clip_and_scroll_translate_and_clip_children() {
        let mut tree = ElementTree::new();
        let mut scroll = Element::new("Scroll", None);
        scroll.set("width", Value::Float(100.0));
        scroll.set("height", Value::Float(50.0));
        scroll.set("scroll_y", Value::Float(20.0));
        scroll.set("fill", Value::Color(Color::from_rgb8(10, 10, 10)));
        let scroll_id = tree.insert(scroll);
        tree.push_root(scroll_id);
        let mut child = Element::new("Rectangle", None);
        child.set("width", Value::Float(100.0));
        child.set("height", Value::Float(30.0));
        child.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        let child_id = tree.insert(child);
        tree.append_child(scroll_id, child_id);

        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        // Scroll background + child rect.
        assert_eq!(scene.rects.len(), 2);
        let child_draw = &scene.rects[1];
        // Child translated up by scroll_y = 20 (child layout y is 0 inside
        // the scroll content).
        assert_eq!(child_draw.geometry.origin.y, -20.0);
        // Child inherits the scroll viewport clip.
        let clip = child_draw.clip.expect("child must be clipped");
        assert_eq!(clip.bounds, Rect::new(Point::ZERO, Size::new(100.0, 50.0)));
        let scroll_draw = &scene.rects[0];
        assert!(
            scroll_draw.clip.is_none(),
            "the scroll element itself is unclipped"
        );
    }

    #[test]
    fn polyline_becomes_a_draw_with_parsed_points_and_opacity() {
        let mut tree = ElementTree::new();
        let mut polyline = Element::new("Polyline", None);
        polyline.set("x", Value::Float(5.0));
        polyline.set("y", Value::Float(6.0));
        polyline.set("points", Value::String("0,0 40,20 80,10".to_string()));
        polyline.set("stroke.width", Value::Float(2.0));
        polyline.set("color", Value::Color(Color::from_rgb8(255, 0, 0)));
        polyline.set("opacity", Value::Float(0.5));
        let id = tree.insert(polyline);
        tree.push_root(id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert_eq!(scene.polylines.len(), 1);
        let draw = &scene.polylines[0];
        // Points shifted by the element origin (5, 6).
        assert_eq!(draw.points[0], Point::new(5.0, 6.0));
        assert_eq!(draw.points[2], Point::new(85.0, 16.0));
        assert_eq!(draw.width, 2.0);
        assert_eq!(draw.cap, LineCap::Round, "round is the default cap");
        assert!((draw.color.alpha() - 0.5).abs() < 1e-6, "opacity folded");
        assert_eq!(draw.clip, None);
    }

    #[test]
    fn arc_flattens_into_a_ring_of_points() {
        // Full circle: 360/4 = 90 segments, and the duplicated closing
        // point is dropped so the GPU never sees a zero-length segment.
        let points = arc_points(50.0, 50.0, 30.0, 0.0, 360.0);
        assert_eq!(points.len(), 90);
        assert_eq!(points[0], Point::new(80.0, 50.0), "angle 0 starts at +x");
        for point in &points {
            let distance = (point.x - 50.0).hypot(point.y - 50.0);
            assert!((distance - 30.0).abs() < 1e-3, "all points on the ring");
        }
        // Quarter sweep: ceil(90/4) = 23 segments -> 24 points, clockwise
        // on screen (y down): angle 90 lands below the center.
        let quarter = arc_points(50.0, 50.0, 30.0, 0.0, 90.0);
        assert_eq!(quarter.len(), 24);
        assert_eq!(quarter[0], Point::new(80.0, 50.0));
        let last = quarter[quarter.len() - 1];
        assert!((last.x - 50.0).abs() < 1e-3);
        assert!((last.y - 80.0).abs() < 1e-3);
        // Degenerate arcs produce nothing.
        assert!(arc_points(0.0, 0.0, 0.0, 0.0, 360.0).is_empty());
        assert!(arc_points(0.0, 0.0, 10.0, 45.0, 45.0).is_empty());
    }

    #[test]
    fn strokes_without_width_or_enough_points_are_skipped() {
        let mut tree = ElementTree::new();
        let mut no_width = Element::new("Polyline", None);
        no_width.set("points", Value::String("0,0 40,20".to_string()));
        let no_width_id = tree.insert(no_width);
        tree.push_root(no_width_id);
        let mut no_points = Element::new("Polyline", None);
        no_points.set("stroke.width", Value::Float(2.0));
        let no_points_id = tree.insert(no_points);
        tree.push_root(no_points_id);
        let mut one_point = Element::new("Polyline", None);
        one_point.set("points", Value::String("0,0".to_string()));
        one_point.set("stroke.width", Value::Float(2.0));
        let one_point_id = tree.insert(one_point);
        tree.push_root(one_point_id);
        let mut arc_no_radius = Element::new("Arc", None);
        arc_no_radius.set("stroke.width", Value::Float(2.0));
        arc_no_radius.set("end", Value::Float(180.0));
        let arc_id = tree.insert(arc_no_radius);
        tree.push_root(arc_id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert_eq!(scene.polylines.len(), 0);
    }

    #[test]
    fn point_pairs_survive_extra_whitespace_and_skip_garbage() {
        // Whitespace separates pairs; the comma must sit inside a pair
        // ("40, 20" splits into two tokens and both are dropped).
        let points = parse_points("  0,0   40,20  80,10 bad oops,again -5,2.5 ");
        assert_eq!(
            points,
            vec![
                Point::new(0.0, 0.0),
                Point::new(40.0, 20.0),
                Point::new(80.0, 10.0),
                Point::new(-5.0, 2.5),
            ]
        );
    }
}
