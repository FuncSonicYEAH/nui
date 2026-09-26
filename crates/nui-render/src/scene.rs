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
use crate::props::{bool_property, color_property, dp_of, f_property};

/// Default font size (dp) for `Text` elements without `font.size`.
const DEFAULT_FONT_SIZE_DP: f32 = 16.0;

/// Line height as a multiple of the font size, for the metrics the scene
/// needs to *predict* (the one-line field's baseline, its selection and
/// caret box) rather than read from a shaped layout.
///
/// Matches `nui-text`'s shaping constant; the wrapped path does not use it,
/// because it reads each line's height from the layout instead.
const TEXT_LINE_HEIGHT_FACTOR: f32 = 1.2;

/// Placeholder text color (muted, blue-grey).
const PLACEHOLDER_COLOR: Color = Color::from_rgb8(0x8a, 0x93, 0xa5);

/// Selection highlight: a translucent blue behind the glyphs.
const SELECTION_COLOR: Color = Color::from_rgb8(0x33, 0x66, 0x99);

/// The caret's color. Fixed rather than `color`-derived: a caret has to be
/// visible against the field's own fill, not against its text.
const CARET_COLOR: Color = Color::WHITE;

/// The glyph a password field draws, one per char.
const MASK_CHAR: &str = "\u{2022}";

/// A `required` label's asterisk: the same red the `danger` variant uses,
/// so a form's "this one is mandatory" and its "this one deletes" at least
/// belong to the same palette.
const REQUIRED_COLOR: Color = Color::from_rgb8(0xc0, 0x43, 0x43);

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

/// One filled path: flattened rings drawn through the triangle pipeline
/// ([`crate::path`]). Stroked paths additionally become [`PolylineDraw`]s.
#[derive(Debug, Clone)]
pub struct PathDraw {
    /// Flattened rings in absolute dp (element-local points + origin).
    /// Open subpaths are included; rings with fewer than 3 points are
    /// ignored by the fill.
    pub loops: Vec<Vec<Point>>,
    /// Fill color (opacity folded in at build time).
    pub fill: Color,
    /// Inherited clip region. v1 note: the triangle pipeline does not
    /// apply shader-side clipping yet (hard edges; scissor/feather is the
    /// later AA review) — the field travels with the draw for when it does.
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
    /// Filled paths in paint order (triangulated by the renderer).
    pub paths: Vec<PathDraw>,
    /// Offscreen layers in paint order (drawn last, M9).
    pub layers: Vec<LayerDraw>,
    /// Overlay content (对话框 / Toast / 下拉) in paint order. Drawn after
    /// `layers`, so an overlay outranks every ordinary element regardless
    /// of where it sits in the tree (FUTURE 批次 4).
    ///
    /// Kept as a full sub-[`Scene`] rather than a flat list because an
    /// overlay is free to use any primitive (a dialog scrim is a rect, a
    /// tooltip is text, a dropdown is both) and nesting an overlay inside
    /// another must keep working.
    pub overlays: Vec<OverlayDraw>,
}

/// One overlay layer: a subtree hoisted out of normal paint order and
/// composited on top of everything else.
///
/// Unlike [`LayerDraw`] there is no texture: the overlay's draws are
/// appended to the main pass directly, just later. That keeps the cost at
/// zero extra render targets for the common case (an opaque scrim plus a
/// panel) and reuses every existing pipeline.
#[derive(Debug, Clone)]
pub struct OverlayDraw {
    /// Absolute dp origin of the overlay's own box.
    pub origin: Point,
    /// The overlay subtree's draws, already flattened and in window
    /// coordinates (the hoist walk reuses the parent's offset).
    pub scene: Box<Scene>,
}

impl Scene {
    /// Appends `inner`'s draws to this scene's tails: rects to rects,
    /// glyphs to glyphs, and so on.
    ///
    /// This is what keeps an overlay on top. The renderer draws one
    /// pipeline at a time — all rects, then all paths, then all strokes,
    /// then all images, then all glyphs — so tail-appending an overlay's
    /// scrim to `rects` and its label to `texts` puts both after every
    /// ordinary draw in their own bucket. An overlay that mixed the two
    /// the other way round (label before scrim) would still be correct,
    /// because a glyph can never be covered by a later rect: the scrim
    /// paints in an earlier pipeline.
    pub fn absorb_overlay(&mut self, inner: Scene) {
        self.rects.extend(inner.rects);
        self.sources.extend(inner.sources);
        self.texts.extend(inner.texts);
        self.images.extend(inner.images);
        self.polylines.extend(inner.polylines);
        self.paths.extend(inner.paths);
        self.layers.extend(inner.layers);
        // `inner.overlays` is empty: the overlay walk builds its own
        // sub-scene, which flattens any overlay it contained already.
    }
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
    paths: Vec<PathDraw>,
    layers: Vec<LayerDraw>,
    overlays: Vec<OverlayDraw>,
    /// Window size in dp, taken from the first root element's box. A
    /// `Dialog`'s backdrop must cover the whole window while the dialog's
    /// own layout box is only the panel, so the scrim size cannot come
    /// from the element itself.
    viewport: Size,
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
            // The first root's box is the window: `layout_with_text` sizes
            // an unsized root to the viewport, so this is the only place
            // that knows the window extent while walking.
            let element = &tree.arena[root];
            let width = f_property(element, "width").unwrap_or(0.0);
            let height = f_property(element, "height").unwrap_or(0.0);
            builder.viewport = Size::new(
                builder.viewport.width.max(width),
                builder.viewport.height.max(height),
            );
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
        // `visible = false` removes the element *and its subtree* from the
        // frame. Layout already gave it no box, but the box is only
        // consulted by some of the arms below — text, images, paths and
        // strokes paint from `x`/`y` directly — so the walk stops here
        // instead of relying on a zero rect to prune each of them.
        if !nui_runtime::widget::is_visible(element) {
            return;
        }
        let width = f_property(element, "width").unwrap_or(0.0);
        let height = f_property(element, "height").unwrap_or(0.0);
        let x = f_property(element, "x").unwrap_or(0.0) + offset.x;
        let y = f_property(element, "y").unwrap_or(0.0) + offset.y;
        let bounds = Rect::new(Point::new(x, y), Size::new(width, height));

        // Overlay hoist (FUTURE 批次 4): an element with `overlay = true`
        // leaves the normal paint order entirely and is composited after
        // `layers` — i.e. above everything. `open = false` removes it
        // (subtree included) rather than hiding it, so a closed dialog
        // costs nothing and cannot be hit.
        //
        // `layer_root` guards against re-hoisting: the overlay's own walk
        // runs with it set, exactly like the M9 capture path.
        if !layer_root && nui_runtime::widget::is_overlay(element) {
            if !nui_runtime::widget::is_open(element) {
                return;
            }
            // Walked with the same offset and absolute `x`/`y`, so the
            // sub-scene stays in *window* coordinates and the flattening
            // step needs no re-origining. The viewport is carried over so
            // a dialog's backdrop still knows how big the window is.
            let mut sub = SceneBuilder::new();
            sub.viewport = self.viewport;
            sub.walk_element(tree, id, offset, None, text, context, true);
            self.overlays.push(OverlayDraw {
                origin: Point::new(x, y),
                scene: Box::new(sub.build()),
            });
            return;
        }

        // Layer capture (M9): `layer.opacity < 1` or `layer.blur > 0`
        // routes the whole subtree into an offscreen texture. The texture
        // covers the element rect, so overflowing content is clipped by
        // the texture bounds.
        let layer_opacity = f_property(element, "layer.opacity").unwrap_or(1.0);
        let layer_blur = f_property(element, "layer.blur").unwrap_or(0.0);
        if !layer_root && width > 0.0 && height > 0.0 && (layer_opacity < 1.0 || layer_blur > 0.0) {
            let mut sub = SceneBuilder::new();
            sub.viewport = self.viewport;
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
            "Waveline" => {
                self.collect_waveline_one(element, bounds, clip);
            }
            "Path" => {
                self.collect_path_one(element, x, y, clip);
            }
            "Canvas" => {
                self.collect_canvas_one(element, bounds);
            }
            // Built-in controls (FUTURE 批次 1-5): each resolves to widget
            // parts painted in control-local space. `Panel` / `Card` /
            // `Separator` are containers with chrome — their children are
            // laid out normally below, this paints what is behind them.
            "Button" | "CheckBox" | "Switch" | "Slider" | "RadioButton" | "Dialog" | "Panel"
            | "Card" | "Separator" | "SpinBox" => {
                self.collect_control_one(element, id, bounds, clip, text);
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

    /// Paints one built-in control by resolving it to
    /// [`crate::widget::WidgetPart`]s and appending them to the draw list.
    ///
    /// Each control is a handful of primitives — a surface, an indicator
    /// box, a track, a thumb, a label — instead of a bespoke shader. The
    /// state styling comes from [`crate::widget::Palette`] +
    /// [`crate::widget::VisualState`], so a control's look is data.
    fn collect_control_one(
        &mut self,
        element: &nui_runtime::Element,
        id: ElementId,
        bounds: Rect,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
    ) {
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let state = crate::widget::VisualState::of(element);
        let base = crate::widget::Palette::of(element);
        let variant = element
            .get("variant")
            .and_then(|value| return value.as_enum().ok())
            .map(|name| return name.to_string())
            .unwrap_or_else(|| return "default".to_string());
        let palette = crate::widget::variant_palette(&variant, base);
        let opacity = f_property(element, "opacity")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        let parts = crate::widget::parts_for(
            element.ty.as_str(),
            element,
            bounds,
            self.viewport,
            state,
            palette,
        );
        let mut painted_surface = false;
        for part in parts {
            painted_surface |= paint_part(self, part, bounds, clip, opacity, text);
        }
        if painted_surface {
            self.sources.push(id);
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
        let shaped = text.shape(content, font_size);
        let content_width = shaped.width;
        self.push_placed(text, &shaped.glyphs, Point::new(x, y), tint, clip);
        if nui_runtime::widget::is_required(element) {
            // The asterisk trails the text, at the colour a form marker has
            // to be to read as one (批次 6).
            let marker_x = x + content_width + nui_runtime::widget::ASTERISK_GAP_DP;
            self.push_glyphs(
                text,
                nui_runtime::widget::REQUIRED_MARK,
                font_size,
                marker_x,
                y,
                REQUIRED_COLOR.with_alpha(opacity),
                clip,
            );
        }
        let _ = id;
    }

    /// Extracts one `TextInput` element's visuals: background, text or
    /// placeholder, selection highlight, and the cursor when focused.
    ///
    /// One-line and multi-line fields share everything except how the text
    /// is placed: a one-line field centres its single line in the box and
    /// clips the overflow, a multi-line one starts at the inset, wraps, and
    /// keeps its caret on whichever visual line the cursor is on. The
    /// `password` mask is applied to the *display* string before either
    /// path measures anything, so the caret, the selection and the IME
    /// underline all follow the dots.
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

        let font_size = nui_runtime::widget::chrome::text_size(element);
        let inset = nui_runtime::widget::chrome::text_inset(element);
        let text_color = color_property(element, "color").unwrap_or(Color::WHITE);
        let Some(state) = element.text_input.as_ref() else {
            return;
        };
        let is_focused = focused == Some(id);
        // Composition display (fcitx5/ibus/XIM): the preedit text is shown
        // inline at the cursor; the cursor moves to its end and an
        // underline marks the composed span.
        let composing = is_focused && !state.preedit.is_empty();
        let preedit_chars = state.preedit.chars().count();
        let cursor = (state.cursor + if composing { preedit_chars } else { 0 })
            .min(state.text.chars().count() + preedit_chars);
        let display = if composing {
            let chars: Vec<char> = state.text.chars().collect();
            let before: String = chars[..state.cursor.min(chars.len())].iter().collect();
            let after: String = chars[state.cursor.min(chars.len())..].iter().collect();
            format!("{before}{}{after}", state.preedit)
        } else {
            state.text.clone()
        };
        let content = InputContent {
            display: if element.is_password() {
                // A password field draws one dot per char, the preedit
                // included: the mask is applied to the *display* string, so
                // the caret, the selection and the underline all follow the
                // dots' widths. The char count is unchanged, which is what
                // keeps every index in `content` meaningful.
                mask_text(&display)
            } else {
                display
            },
            cursor,
            caret_text: state.cursor,
            preedit_chars,
            selection: state.selection(),
            composing,
        };
        let tint = text_color.with_alpha(text_color.alpha() * opacity);
        let placeholder = element
            .get("placeholder")
            .and_then(|value| return value.as_str().ok())
            .unwrap_or("")
            .to_string();
        let bounds = Rect::new(Point::new(x, y), Size::new(width, height));
        if element.is_multiline() {
            self.paint_wrapped_input(
                &content,
                &placeholder,
                bounds,
                inset,
                font_size,
                tint,
                opacity,
                radius,
                clip,
                text,
                is_focused,
            );
        } else {
            self.paint_line_input(
                &content,
                &placeholder,
                bounds,
                inset,
                font_size,
                tint,
                opacity,
                clip,
                text,
                is_focused,
            );
        }
    }

    /// Paints a one-line field: its single line centred in the box, the
    /// selection, the caret, and the IME underline.
    ///
    /// The field does not clip its own text: a one-line field with more
    /// content than width is a *document* mistake (`max_length`, a wider
    /// box, or `multiline`), and silently cutting it in half would hide it.
    #[allow(clippy::too_many_arguments)]
    fn paint_line_input(
        &mut self,
        content: &InputContent,
        placeholder: &str,
        bounds: Rect,
        inset: f32,
        font_size: f32,
        tint: Color,
        opacity: f32,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
        focused: bool,
    ) {
        let (x, y) = (bounds.origin.x, bounds.origin.y);
        let height = bounds.size.height;
        let line_height = font_size * TEXT_LINE_HEIGHT_FACTOR;
        let origin_y = y + (height - line_height) / 2.0;
        if content.display.is_empty() {
            if !placeholder.is_empty() {
                let dim = PLACEHOLDER_COLOR.with_alpha(opacity);
                self.push_glyphs(text, placeholder, font_size, x + inset, origin_y, dim, clip);
            }
        } else {
            self.push_glyphs(
                text,
                &content.display,
                font_size,
                x + inset,
                origin_y,
                tint,
                clip,
            );
        }
        if !focused {
            return;
        }
        let chars: Vec<char> = content.display.chars().collect();
        if content.composing {
            // Underline under the composed span.
            let from = measure_width(
                text,
                &chars[..content.caret_text.min(chars.len())],
                font_size,
            );
            let to = measure_width(text, &chars[..content.cursor.min(chars.len())], font_size);
            self.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(x + inset + from, y + height * 0.82),
                    Size::new((to - from).max(2.0), 2.0),
                ),
                corner_radius: 0.0,
                fill: tint,
                shadow: None,
                clip,
                rotation: 0.0,
                gradient: None,
            });
        }
        // Selection highlight under the glyphs.
        let (select_from, select_to) = content.selection_display_range();
        if select_from < select_to {
            let start_x = measure_width(text, &chars[..select_from.min(chars.len())], font_size);
            let end_x = measure_width(text, &chars[..select_to.min(chars.len())], font_size);
            self.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(x + inset + start_x, origin_y),
                    Size::new((end_x - start_x).max(1.0), line_height),
                ),
                corner_radius: 2.0,
                fill: SELECTION_COLOR.with_alpha(0.5 * opacity),
                shadow: None,
                clip,
                rotation: 0.0,
                gradient: None,
            });
        }
        // Cursor: a 2dp caret, centred vertically; during composition it
        // sits after the preedit text.
        let cursor_x =
            x + inset + measure_width(text, &chars[..content.cursor.min(chars.len())], font_size);
        self.rects.push(RectDraw {
            geometry: Rect::new(
                Point::new(cursor_x, y + height * 0.2),
                Size::new(2.0, height * 0.6),
            ),
            corner_radius: 1.0,
            fill: CARET_COLOR.with_alpha(opacity),
            shadow: None,
            clip,
            rotation: 0.0,
            gradient: None,
        });
    }

    /// Paints a multi-line field: the wrapped lines laid out from the
    /// inset, with the selection and the caret on their own visual lines.
    ///
    /// The content is clipped to the field's own box — unlike the one-line
    /// painter — because a multi-line field with a declared `height` is
    /// *meant* to hold more than it shows (that is what makes it scrollable
    /// later); letting the overflow paint outside would cover its
    /// neighbours.
    #[allow(clippy::too_many_arguments)]
    fn paint_wrapped_input(
        &mut self,
        content: &InputContent,
        placeholder: &str,
        bounds: Rect,
        inset: f32,
        font_size: f32,
        tint: Color,
        opacity: f32,
        radius: f32,
        clip: Option<ClipDraw>,
        text: &mut nui_text::TextSystem,
        focused: bool,
    ) {
        let wrap_width = (bounds.size.width - inset * 2.0).max(1.0);
        let layout = text.layout_wrapped(&content.display, font_size, wrap_width);
        let inner_clip = push_clip(clip, bounds, radius);
        let origin = Point::new(bounds.origin.x + inset, bounds.origin.y + inset);
        if content.display.is_empty() {
            if !placeholder.is_empty() {
                let dim = PLACEHOLDER_COLOR.with_alpha(opacity);
                self.push_glyphs(
                    text,
                    placeholder,
                    font_size,
                    origin.x,
                    origin.y,
                    dim,
                    inner_clip,
                );
            }
        } else {
            self.push_placed(text, &layout.glyphs, origin, tint, inner_clip);
        }
        if !focused {
            return;
        }
        // The selection, line by line: a range crossing a wrap is two rects.
        let (select_from, select_to) = content.selection_display_range();
        for (line, from, to) in range_spans(&layout, select_from, select_to) {
            let metrics = &layout.lines[line];
            self.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(origin.x + from, origin.y + metrics.top),
                    Size::new((to - from).max(1.0), metrics.height),
                ),
                corner_radius: 2.0,
                fill: SELECTION_COLOR.with_alpha(0.5 * opacity),
                shadow: None,
                clip: inner_clip,
                rotation: 0.0,
                gradient: None,
            });
        }
        // The IME underline, on whichever line the composition landed.
        if content.composing {
            let span_end = content.cursor;
            for (line, from, to) in range_spans(&layout, content.caret_text, span_end) {
                let metrics = &layout.lines[line];
                let bottom = origin.y + metrics.top + metrics.height;
                self.rects.push(RectDraw {
                    geometry: Rect::new(
                        Point::new(origin.x + from, bottom - 2.0),
                        Size::new((to - from).max(2.0), 2.0),
                    ),
                    corner_radius: 0.0,
                    fill: tint,
                    shadow: None,
                    clip: inner_clip,
                    rotation: 0.0,
                    gradient: None,
                });
            }
        }
        let (caret_x, caret_top, caret_height) = layout.caret_box(content.cursor);
        self.rects.push(RectDraw {
            geometry: Rect::new(
                Point::new(
                    origin.x + caret_x,
                    origin.y + caret_top + caret_height * 0.1,
                ),
                Size::new(2.0, caret_height * 0.8),
            ),
            corner_radius: 1.0,
            fill: CARET_COLOR.with_alpha(opacity),
            shadow: None,
            clip: inner_clip,
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

    /// Extracts one `Waveline` element's stroke: a procedural multi-harmonic
    /// wave, or a data-driven line when `levels` is set (Cava-style
    /// normalized samples, the iNiR visualizer look). `mirror = true` adds
    /// the point-reflected twin. Both render through the stroke pipeline.
    fn collect_waveline_one(
        &mut self,
        element: &nui_runtime::Element,
        bounds: Rect,
        clip: Option<ClipDraw>,
    ) {
        let Some((width, cap, color)) = stroke_style_of(element) else {
            return;
        };
        let amplitude = f_property(element, "amplitude").unwrap_or(10.0);
        if bounds.size.width <= 0.0 || amplitude <= 0.0 {
            return;
        }
        let frequency = f_property(element, "frequency").unwrap_or(2.0);
        let phase = f_property(element, "phase").unwrap_or(0.0);
        // ~3dp between samples is smooth enough at stroke widths of 1-8dp.
        let samples = (bounds.size.width / 3.0).clamp(24.0, 160.0) as usize;
        let mirror = bool_property(element, "mirror");
        let levels = element
            .get("levels")
            .and_then(|value| return value.as_str().ok())
            .map(parse_levels)
            .filter(|levels| return levels.len() >= 2);
        let points = match &levels {
            Some(levels) => level_points(bounds, amplitude, levels, samples, false),
            None => wave_points(bounds, amplitude, frequency, phase, samples, false),
        };
        self.push_stroke(points, width, cap, color, clip, bounds.origin);
        if mirror {
            let twin = match &levels {
                Some(levels) => level_points(bounds, amplitude, levels, samples, true),
                None => wave_points(bounds, amplitude, frequency, phase, samples, true),
            };
            self.push_stroke(twin, width, cap, color, clip, bounds.origin);
        }
    }

    /// Extracts one `Path` element: parses the `d` attribute, flattens it
    /// into rings, and emits the fill plus (when `stroke.width` is set)
    /// one stroke per ring. A broken `d` string silently paints nothing
    /// (v1; compile-time validation is a later item).
    fn collect_path_one(
        &mut self,
        element: &nui_runtime::Element,
        x: f32,
        y: f32,
        clip: Option<ClipDraw>,
    ) {
        let Some(data) = element
            .get("d")
            .and_then(|value| return value.as_str().ok())
            .and_then(|text| return nui_core::path::parse_path(text).ok())
        else {
            return;
        };
        // ~0.1dp tolerance keeps curves visually smooth at UI stroke widths.
        let loops = nui_core::path::flatten(&data, 0.1);
        let opacity = f_property(element, "opacity")
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if let (Some(fill), false) = (color_property(element, "fill"), opacity <= 0.0) {
            let loops: Vec<Vec<Point>> = loops
                .iter()
                .map(|loop_points| {
                    return loop_points
                        .iter()
                        .map(|point| return Point::new(point.x + x, point.y + y))
                        .collect();
                })
                .collect();
            self.paths.push(PathDraw {
                loops,
                fill: fill.with_alpha(fill.alpha() * opacity),
                clip,
            });
        }
        if let Some((width, cap, color)) = stroke_style_of(element) {
            for loop_points in &loops {
                self.push_stroke(
                    loop_points.clone(),
                    width,
                    cap,
                    color,
                    clip,
                    Point::new(x, y),
                );
            }
        }
    }

    /// Extracts one `Canvas` element (FUTURE batch 3): interprets the
    /// behavior's command buffer into fills (earcut) and strokes (capsule
    /// pipeline) inside a texture-local sub-scene, composited through the
    /// offscreen layer pipeline. Zero new GPU code; v1 reinterprets the
    /// buffer every frame. The element's inherited clip does not reach the
    /// sub-scene — the layer texture bounds ARE the clip, matching how
    /// `layer.*` capture works.
    fn collect_canvas_one(&mut self, element: &nui_runtime::Element, bounds: Rect) {
        let Some(painter) = element.canvas.as_ref() else {
            return;
        };
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        // ~0.1dp tolerance keeps curves visually smooth at UI stroke widths.
        let frame = nui_runtime::canvas::interpret(&painter.ops(), 0.1);
        let mut sub = SceneBuilder::new();
        for (color, loops) in frame.fills {
            sub.paths.push(PathDraw {
                loops,
                fill: color,
                clip: None,
            });
        }
        for (color, width, cap, loops) in frame.strokes {
            let cap = match cap {
                nui_runtime::canvas::CanvasCap::Butt => LineCap::Butt,
                nui_runtime::canvas::CanvasCap::Round => LineCap::Round,
            };
            for points in loops {
                if points.len() < 2 {
                    continue;
                }
                sub.polylines.push(PolylineDraw {
                    points,
                    width,
                    cap,
                    color,
                    clip: None,
                });
            }
        }
        self.layers.push(LayerDraw {
            origin: bounds.origin,
            size: bounds.size,
            opacity: 1.0,
            blur: 0.0,
            scene: Box::new(sub.build()),
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
        self.push_placed(
            text,
            &shaped.glyphs,
            Point::new(origin_x, origin_y),
            tint,
            clip,
        );
    }

    /// Pushes the quads for already-shaped glyphs, placed relative to the
    /// block origin.
    ///
    /// The multi-line path shapes through a wrap (so it has to own the
    /// layout) while the single-line path shapes per call; both end here,
    /// because turning a glyph into a texture quad is the same work either
    /// way.
    fn push_placed(
        &mut self,
        text: &mut nui_text::TextSystem,
        glyphs: &[nui_text::ShapedGlyph],
        origin: Point,
        tint: Color,
        clip: Option<ClipDraw>,
    ) {
        for glyph in glyphs {
            let Some(quad) = text.glyph_quad(&glyph.key) else {
                continue;
            };
            self.texts.push(TextDraw {
                origin: Point::new(
                    origin.x + glyph.x + quad.left as f32,
                    origin.y + glyph.y + quad.top as f32,
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
    ///
    /// Overlays are *flattened in* here, at the tail of each draw list.
    /// That is what gives them their z-order: the renderer paints rects,
    /// then paths, then strokes, then images, then glyphs, so appending an
    /// overlay's scrim to `rects` and its label to `texts` puts both above
    /// every ordinary element.
    ///
    /// The overlay walk already flattened nested overlays (it calls
    /// `build` on its own sub-builder), so one level of flattening here is
    /// enough — `inner.overlays` is always empty.
    pub fn build(mut self) -> Scene {
        let overlays = std::mem::take(&mut self.overlays);
        let mut scene = Scene {
            rects: self.rects,
            sources: self.sources,
            texts: self.texts,
            images: self.images,
            polylines: self.polylines,
            paths: self.paths,
            layers: self.layers,
            overlays: Vec::new(),
        };
        for overlay in overlays {
            // The hoist already built the sub-scene, so this is a plain
            // append: no second `build` pass, no coordinate shift.
            scene.absorb_overlay(*overlay.scene);
        }
        return scene;
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

/// One text field's resolved content: what to draw, and where the caret and
/// the selection are *in that string*.
///
/// Resolving this once — preedit inlined, password masked — is what keeps
/// the one-line and multi-line painters pure placement: neither has to know
/// about IME or masking, only about where things go.
struct InputContent {
    /// The string to shape and draw.
    display: String,
    /// Caret position in `display`, as a char index.
    cursor: usize,
    /// Caret position in the *text* (i.e. with the preedit excluded).
    caret_text: usize,
    /// How many chars the preedit contributes to `display`.
    preedit_chars: usize,
    /// Selection range in the *text*.
    selection: (usize, usize),
    /// Whether an IME composition is live.
    composing: bool,
}

impl InputContent {
    /// Maps a text char index to its index in `display`, skipping the
    /// preedit's chars when the index is past the caret.
    fn display_index(&self, index: usize) -> usize {
        if !self.composing || index <= self.caret_text {
            return index;
        }
        return index + self.preedit_chars;
    }

    /// The selection as indices into `display`.
    fn selection_display_range(&self) -> (usize, usize) {
        return (
            self.display_index(self.selection.0),
            self.display_index(self.selection.1),
        );
    }
}

/// Replaces every char with a bullet, keeping the char count — the caret
/// and the selection positions in `display` stay valid.
fn mask_text(text: &str) -> String {
    return MASK_CHAR.repeat(text.chars().count());
}

/// Where a char range `start..end` sits in a wrapped layout: one
/// `(line index, x0, x1)` span per visual line the range touches, x
/// measured from the layout's origin.
///
/// One range can be several rects — that is what a selection crossing a
/// wrap looks like. Shared by the selection highlight and the IME
/// underline, which cover the same geometry at different heights.
fn range_spans(
    layout: &nui_text::WrappedLayout,
    start: usize,
    end: usize,
) -> Vec<(usize, f32, f32)> {
    let mut spans = Vec::new();
    for (index, line) in layout.lines.iter().enumerate() {
        let from = start.max(line.start);
        let to = end.min(line.end);
        if from >= to {
            continue;
        }
        // `caret_x` at a line's *end* reports the next line's start, so the
        // end of a span is the line's own width.
        let x0 = layout.caret_x(from);
        let x1 = if to >= line.end {
            line.width
        } else {
            layout.caret_x(to)
        };
        spans.push((index, x0.min(x1), x0.max(x1)));
    }
    return spans;
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

/// Parses `"0.2 0.5 1.0"` normalized samples (whitespace-separated, values
/// clamped to 0..=1). Malformed entries are skipped.
fn parse_levels(text: &str) -> Vec<f32> {
    return text
        .split_whitespace()
        .filter_map(|token| return token.parse::<f32>().ok())
        .map(|level| return level.clamp(0.0, 1.0))
        .collect();
}

/// Builds a wave polyline across `bounds`: three sine harmonics (weights
/// sum to 1.0) give the line an organic, Cava-idle-like motion instead of
/// a geometric sine. Points are element-local around the vertical middle;
/// `flip` reflects the line for `mirror = true`.
fn wave_points(
    bounds: Rect,
    amplitude: f32,
    frequency: f32,
    phase_deg: f32,
    samples: usize,
    flip: bool,
) -> Vec<Point> {
    let width = bounds.size.width;
    let center = bounds.size.height * 0.5;
    let sign = if flip { 1.0 } else { -1.0 };
    let phase = phase_deg.to_radians();
    let mut points = Vec::with_capacity(samples + 1);
    for step in 0..=samples {
        let t = step as f32 / samples as f32;
        let angle = t * frequency * std::f32::consts::TAU + phase;
        let wave = angle.sin() * 0.62
            + (angle * 2.3 + 1.7).sin() * 0.23
            + (angle * 0.71 + 4.1).sin() * 0.15;
        points.push(Point::new(t * width, center + sign * wave * amplitude));
    }
    return points;
}

/// Builds a polyline from normalized `levels` (0..=1, linear interpolation
/// between samples): `level * amplitude` above the middle, mirrored below
/// when `flip`. This is the data-driven Cava mode.
fn level_points(
    bounds: Rect,
    amplitude: f32,
    levels: &[f32],
    samples: usize,
    flip: bool,
) -> Vec<Point> {
    let width = bounds.size.width;
    let center = bounds.size.height * 0.5;
    let sign = if flip { 1.0 } else { -1.0 };
    let count = levels.len();
    let mut points = Vec::with_capacity(samples + 1);
    for step in 0..=samples {
        let t = step as f32 / samples as f32;
        let index = t * (count - 1) as f32;
        let lower = index.floor() as usize;
        let upper = (lower + 1).min(count - 1);
        let mix = index - lower as f32;
        let level = levels[lower] + (levels[upper] - levels[lower]) * mix;
        points.push(Point::new(
            t * width,
            center + sign * level.clamp(0.0, 1.0) * amplitude,
        ));
    }
    return points;
}

/// Paints one widget part into the draw list. Returns whether it drew a
/// surface (so the caller can register the element as a hit-test source).
fn paint_part(
    builder: &mut SceneBuilder,
    part: crate::widget::WidgetPart,
    bounds: Rect,
    clip: Option<ClipDraw>,
    opacity: f32,
    text: &mut nui_text::TextSystem,
) -> bool {
    use crate::widget::WidgetPart;
    let origin = bounds.origin;
    let scale = |color: Color| return color.with_alpha(color.alpha() * opacity);
    return match part {
        WidgetPart::Rect {
            x,
            y,
            width,
            height,
            radius,
            color,
        } => {
            builder.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(origin.x + x, origin.y + y),
                    Size::new(width, height),
                ),
                corner_radius: radius,
                fill: scale(color),
                shadow: None,
                clip,
                rotation: 0.0,
                gradient: None,
            });
            true
        }
        WidgetPart::Surface {
            x,
            y,
            width,
            height,
            radius,
            fill,
            border,
            shadow,
        } => {
            builder.rects.push(RectDraw {
                geometry: Rect::new(
                    Point::new(origin.x + x, origin.y + y),
                    Size::new(width, height),
                ),
                corner_radius: radius,
                fill: scale(fill),
                shadow: shadow.map(|shadow| {
                    return Shadow {
                        offset: Point::new(shadow.dx, shadow.dy),
                        blur: shadow.blur,
                        color: scale(shadow.color),
                    };
                }),
                clip,
                rotation: 0.0,
                gradient: None,
            });
            if let Some((stroke, color)) = border {
                push_rect_outline(
                    builder,
                    origin,
                    x,
                    y,
                    width,
                    height,
                    stroke,
                    radius,
                    scale(color),
                    clip,
                );
            }
            true
        }
        WidgetPart::Outline {
            x,
            y,
            width,
            height,
            stroke,
            radius,
            color,
        } => {
            push_rect_outline(
                builder,
                origin,
                x,
                y,
                width,
                height,
                stroke,
                radius,
                scale(color),
                clip,
            );
            false
        }
        WidgetPart::Line {
            points,
            stroke,
            color,
        } => {
            if points.len() < 2 {
                return false;
            }
            let points = points
                .into_iter()
                .map(|(px, py)| return Point::new(origin.x + px, origin.y + py))
                .collect();
            // Widget outlines are crisp: round joins via capsule segments,
            // butt caps so a check mark's tips stay square.
            builder.push_stroke(
                points,
                stroke,
                LineCap::Round,
                scale(color),
                clip,
                Point::ZERO,
            );
            false
        }
        WidgetPart::Label {
            text: content,
            size,
            color,
            padding,
            center_y,
        } => {
            let line_height = text_measure_height(text, &content, size);
            let baseline = origin.y + center_y - line_height / 2.0;
            builder.push_glyphs(
                text,
                &content,
                size,
                origin.x + padding,
                baseline,
                scale(color),
                clip,
            );
            false
        }
    };
}

/// The shaped height of a line, for vertical centering.
fn text_measure_height(text: &mut nui_text::TextSystem, content: &str, size: f32) -> f32 {
    return text.shape(content, size).height.max(size * 1.2);
}

/// Strokes a rectangle outline as four polyline segments (the capsule
/// pipeline has no closed-ring primitive; four segments with round caps
/// meet at the corners).
#[allow(clippy::too_many_arguments)]
fn push_rect_outline(
    builder: &mut SceneBuilder,
    origin: Point,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke: f32,
    radius: f32,
    color: Color,
    clip: Option<ClipDraw>,
) {
    if width <= 0.0 || height <= 0.0 || stroke <= 0.0 {
        return;
    }
    let inset = stroke / 2.0;
    let left = origin.x + x + inset;
    let top = origin.y + y + inset;
    let right = origin.x + x + width - inset;
    let bottom = origin.y + y + height - inset;
    let corner = radius.clamp(0.0, (right - left).min(bottom - top) / 2.0);
    let mut points: Vec<Point> = Vec::new();
    // Top-left corner arc: a quarter circle approximated by three points
    // (the capsule pipeline renders joins, so coarse is fine).
    if corner > 0.0 {
        points.push(Point::new(left + corner, top));
        points.push(Point::new(right - corner, top));
        push_corner(
            &mut points,
            right - corner,
            top + corner,
            corner,
            -90.0,
            0.0,
        );
        points.push(Point::new(right, bottom - corner));
        push_corner(
            &mut points,
            right - corner,
            bottom - corner,
            corner,
            0.0,
            90.0,
        );
        points.push(Point::new(left + corner, bottom));
        push_corner(
            &mut points,
            left + corner,
            bottom - corner,
            corner,
            90.0,
            180.0,
        );
        points.push(Point::new(left, top + corner));
        push_corner(
            &mut points,
            left + corner,
            top + corner,
            corner,
            180.0,
            270.0,
        );
        points.push(Point::new(left + corner, top));
    } else {
        points.extend([
            Point::new(left, top),
            Point::new(right, top),
            Point::new(right, bottom),
            Point::new(left, bottom),
            Point::new(left, top),
        ]);
    }
    builder.push_stroke(points, stroke, LineCap::Round, color, clip, Point::ZERO);
}

/// Appends `steps` points along an arc centered at `(cx, cy)`, from
/// `start_deg` to `end_deg`.
fn push_corner(
    points: &mut Vec<Point>,
    cx: f32,
    cy: f32,
    radius: f32,
    start_deg: f32,
    end_deg: f32,
) {
    const STEPS: usize = 4;
    for step in 0..=STEPS {
        let t = step as f32 / STEPS as f32;
        let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
        points.push(Point::new(
            cx + radius * angle.cos(),
            cy + radius * angle.sin(),
        ));
    }
}

/// Reads the stroke style: `stroke.width` (dp) is the gate — no width, no
/// stroke. The cap is `cap`, maps `"round"`/`"butt"` (default round), and
/// the color comes from `color` then `stroke.color` (white default) with
/// the element's opacity folded in.
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
    use crate::testkit::{build, widget, window_tree};
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
    fn a_hidden_element_is_not_painted_and_takes_its_subtree_with_it() {
        let mut tree = ElementTree::new();
        let mut hidden = sized_element(10.0, 10.0);
        hidden.set("visible", Value::Bool(false));
        let hidden_id = tree.insert(hidden);
        // A visible child under a hidden parent: nothing under a hidden
        // node is reachable, so the subtree goes too.
        let child = tree.insert(sized_element(10.0, 10.0));
        tree.append_child(hidden_id, child);
        let shown = tree.insert(sized_element(10.0, 10.0));
        for id in [hidden_id, shown] {
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
        assert_eq!(scene.sources, vec![shown]);
    }

    #[test]
    fn a_hidden_label_never_reaches_the_text_system() {
        // Text paints from `x`/`y` and never consults `width`/`height`, so
        // it is exactly the element class a zero-box pruning rule would
        // have missed. The walk has to stop before the shaping call, which
        // is why this asserts on `texts` and not on a rect.
        let mut tree = ElementTree::new();
        let mut hidden = Element::new("Text", None);
        hidden.set("content", Value::String("hidden".to_string()));
        hidden.set("width", Value::Length(nui_core::Length::Dp(80.0)));
        hidden.set("height", Value::Length(nui_core::Length::Dp(20.0)));
        hidden.set("visible", Value::Bool(false));
        let hidden_id = tree.insert(hidden);
        tree.push_root(hidden_id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        assert!(scene.texts.is_empty(), "no glyph quads: {:?}", scene.texts);
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
    fn wave_points_stay_in_the_envelope_and_mirror_symmetrically() {
        let bounds = Rect::new(Point::ZERO, Size::new(90.0, 60.0));
        let wave = wave_points(bounds, 10.0, 2.0, 0.0, 30, false);
        assert_eq!(wave.len(), 31);
        for point in &wave {
            assert!(
                (point.y - 30.0).abs() <= 10.0 + 1e-3,
                "harmonics must not exceed the amplitude"
            );
        }
        let flipped = wave_points(bounds, 10.0, 2.0, 0.0, 30, true);
        for (main, twin) in wave.iter().zip(&flipped) {
            assert_eq!(main.x, twin.x);
            assert!(
                (main.y + twin.y - 60.0).abs() < 1e-3,
                "mirror twins reflect about the middle"
            );
        }
        // Full levels draw a flat line exactly `amplitude` above the middle.
        let flat = level_points(bounds, 15.0, &[1.0, 1.0], 30, false);
        assert!(
            flat.iter()
                .all(|point| return (point.y - 15.0).abs() < 1e-3)
        );
    }

    #[test]
    fn waveline_element_becomes_one_or_two_strokes() {
        let build = |mirror: bool| {
            let mut tree = ElementTree::new();
            let mut wave = Element::new("Waveline", None);
            wave.set("x", Value::Float(5.0));
            wave.set("width", Value::Float(100.0));
            wave.set("height", Value::Float(60.0));
            wave.set("amplitude", Value::Float(10.0));
            wave.set("stroke.width", Value::Float(2.0));
            wave.set("color", Value::Color(Color::from_rgb8(255, 0, 0)));
            if mirror {
                wave.set("mirror", Value::Bool(true));
            }
            let id = tree.insert(wave);
            tree.push_root(id);
            return SceneBuilder::build_with_context(
                &tree,
                &mut nui_text::TextSystem::with_embedded_font(),
                SceneContext {
                    focused: None,
                    image_keys: &HashMap::new(),
                },
            );
        };
        let single = build(false);
        assert_eq!(single.polylines.len(), 1);
        // Points are element-local, offset by the origin (5, 0); the wave
        // starts somewhere inside the amplitude envelope around the middle.
        let first = single.polylines[0].points[0];
        assert_eq!(first.x, 5.0);
        assert!((first.y - 30.0).abs() <= 10.0 + 1e-3, "inside the envelope");
        assert_eq!(single.polylines[0].points.last().unwrap().x, 105.0);
        let mirrored = build(true);
        assert_eq!(mirrored.polylines.len(), 2, "mirror adds the twin");
    }

    #[test]
    fn levels_parse_clamps_and_skip_garbage() {
        assert_eq!(parse_levels("0.5 2 -0.3 bad 1"), vec![0.5, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn waveline_without_width_or_amplitude_is_skipped() {
        let mut tree = ElementTree::new();
        let mut no_width = Element::new("Waveline", None);
        no_width.set("height", Value::Float(60.0));
        no_width.set("amplitude", Value::Float(10.0));
        no_width.set("stroke.width", Value::Float(2.0));
        let no_width_id = tree.insert(no_width);
        tree.push_root(no_width_id);
        let mut zero_amplitude = Element::new("Waveline", None);
        zero_amplitude.set("width", Value::Float(100.0));
        zero_amplitude.set("height", Value::Float(60.0));
        zero_amplitude.set("amplitude", Value::Float(0.0));
        zero_amplitude.set("stroke.width", Value::Float(2.0));
        let zero_amplitude_id = tree.insert(zero_amplitude);
        tree.push_root(zero_amplitude_id);
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

    #[test]
    fn path_element_flattens_into_fill_and_strokes() {
        let mut tree = ElementTree::new();
        let mut path = Element::new("Path", None);
        path.set("x", Value::Float(5.0));
        path.set("y", Value::Float(6.0));
        path.set("d", Value::String("M 0 0 L 48 0 L 24 42 Z".to_string()));
        path.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        path.set("stroke.width", Value::Float(2.0));
        path.set("stroke.color", Value::Color(Color::from_rgb8(0, 0, 255)));
        path.set("opacity", Value::Float(0.5));
        let id = tree.insert(path);
        tree.push_root(id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        // Fill: one closed ring, offset by the origin, opacity folded in.
        assert_eq!(scene.paths.len(), 1);
        let fill = &scene.paths[0];
        assert_eq!(fill.loops.len(), 1);
        assert_eq!(fill.loops[0][0], Point::new(5.0, 6.0));
        assert_eq!(fill.loops[0][1], Point::new(53.0, 6.0));
        assert!((fill.fill.alpha() - 0.5).abs() < 1e-6, "opacity folded");
        // Stroke: the same ring goes through the polyline pipeline.
        assert_eq!(scene.polylines.len(), 1);
        assert_eq!(
            scene.polylines[0].points.len(),
            4,
            "closed ring of 3 points"
        );
        assert_eq!(scene.polylines[0].width, 2.0);
    }

    #[test]
    fn path_without_fill_or_stroke_paints_nothing() {
        let mut tree = ElementTree::new();
        let mut no_d = Element::new("Path", None);
        no_d.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        let no_d_id = tree.insert(no_d);
        tree.push_root(no_d_id);
        let mut broken_d = Element::new("Path", None);
        broken_d.set("d", Value::String("M 0 0 X 5".to_string()));
        broken_d.set("fill", Value::Color(Color::from_rgb8(255, 0, 0)));
        let broken_id = tree.insert(broken_d);
        tree.push_root(broken_id);
        let mut stroke_only = Element::new("Path", None);
        stroke_only.set("d", Value::String("M 0 0 L 10 0".to_string()));
        stroke_only.set("stroke.width", Value::Float(2.0));
        let stroke_id = tree.insert(stroke_only);
        tree.push_root(stroke_id);
        let scene = SceneBuilder::build_with_context(
            &tree,
            &mut nui_text::TextSystem::with_embedded_font(),
            SceneContext {
                focused: None,
                image_keys: &HashMap::new(),
            },
        );
        // No fill anywhere; only the stroke-only path produces a polyline.
        assert_eq!(scene.paths.len(), 0);
        assert_eq!(scene.polylines.len(), 1);
    }

    #[test]
    fn an_overlay_element_paints_after_the_content_it_covers() {
        // A plain rect declared *after* an overlay must still end up
        // beneath it: the overlay is hoisted, not merely later in the
        // document.
        let mut overlay = widget("Rectangle", 100.0, 50.0);
        overlay.set("overlay", Value::Bool(true));
        overlay.set("fill", Value::Color(nui_core::Color::from_rgb8(255, 0, 0)));
        let mut plain = widget("Rectangle", 100.0, 50.0);
        plain.set("fill", Value::Color(nui_core::Color::from_rgb8(0, 255, 0)));
        let tree = window_tree(vec![overlay, plain]);
        let scene = build(&tree);
        // Two rects, and the overlay's is last: the renderer paints rects
        // in list order, so last = topmost.
        assert_eq!(scene.rects.len(), 2);
        assert_eq!(scene.rects[0].fill, nui_core::Color::from_rgb8(0, 255, 0));
        assert_eq!(scene.rects[1].fill, nui_core::Color::from_rgb8(255, 0, 0));
        assert!(scene.overlays.is_empty(), "flattened at build time");
    }

    #[test]
    fn a_closed_dialog_paints_nothing_at_all() {
        let mut dialog = widget("Dialog", 240.0, 160.0);
        dialog.set("open", Value::Bool(false));
        let mut plain = widget("Rectangle", 100.0, 50.0);
        plain.set("fill", Value::Color(nui_core::Color::from_rgb8(0, 255, 0)));
        let tree = window_tree(vec![dialog, plain]);
        let scene = build(&tree);
        assert_eq!(scene.rects.len(), 1, "only the plain rect remains");
        assert_eq!(scene.rects[0].fill, nui_core::Color::from_rgb8(0, 255, 0));
    }

    #[test]
    fn an_overlay_inside_a_layer_still_reaches_the_front() {
        // The M9 layer path walks a subtree; an overlay in there must be
        // hoisted to the top of that sub-scene, not swallowed by it.
        let mut layer = widget("Rectangle", 200.0, 100.0);
        layer.set("layer.blur", Value::Float(2.0));
        let mut dialog = widget("Dialog", 120.0, 80.0);
        dialog.set("open", Value::Bool(true));
        let mut tree = ElementTree::new();
        let mut root = Element::new("Column", None);
        root.set("width", Value::Float(400.0));
        root.set("height", Value::Float(300.0));
        let root_id = tree.insert(root);
        tree.push_root(root_id);
        let layer_id = tree.insert(layer);
        tree.append_child(root_id, layer_id);
        let dialog_id = tree.insert(dialog);
        tree.append_child(layer_id, dialog_id);
        let scene = build(&tree);
        // One M9 layer, whose own scene holds the dialog's draws at the
        // tail (backdrop + panel) rather than losing them.
        assert_eq!(scene.layers.len(), 1);
        let inner = &scene.layers[0].scene;
        assert_eq!(inner.rects.len(), 2, "backdrop + panel: {:?}", inner.rects);
    }

    #[test]
    fn a_multi_line_field_draws_its_wrapped_lines_stacked() {
        // A field 100dp wide holding 31 chars: at 16dp the text cannot fit
        // on one line, so the glyphs must arrive at more than one `y`. This
        // is the draw-side half of the wrap — `nui-layout` decides the box,
        // this decides that the painted lines follow the same breaks.
        let content = "alpha beta gamma delta epsilon";
        let mut field = widget("TextInput", 100.0, 60.0);
        field.set("text", Value::String(content.to_string()));
        field.set("multiline", Value::Bool(true));
        field.text_input = Some(nui_runtime::TextInputState::from_text(content));
        let tree = window_tree(vec![field]);
        let scene = build(&tree);

        assert!(!scene.texts.is_empty(), "the content reaches the scene");
        let top = scene
            .texts
            .iter()
            .map(|quad| return quad.origin.y)
            .fold(f32::INFINITY, f32::min);
        let bottom = scene
            .texts
            .iter()
            .map(|quad| return quad.origin.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            bottom - top > 12.0,
            "glyphs on more than one line: spread {top}..{bottom}"
        );
        // ... and they stay inside the box, inset by the field's own 8dp.
        for quad in &scene.texts {
            assert!(
                quad.origin.x >= 7.5 && quad.origin.y >= 7.5,
                "drawn inside the inset: {:?}",
                quad.origin
            );
            assert!(
                quad.clip.is_some(),
                "a wrapped field clips its overflow at its own edge"
            );
        }
    }

    #[test]
    fn a_password_field_draws_every_char_as_the_same_glyph() {
        let content = "secret";
        let field = |password: bool| {
            let mut element = widget("TextInput", 200.0, 40.0);
            element.set("text", Value::String(content.to_string()));
            element.set("password", Value::Bool(password));
            element.text_input = Some(nui_runtime::TextInputState::from_text(content));
            return element;
        };
        let scene = build(&window_tree(vec![field(true)]));

        assert_eq!(scene.texts.len(), 6, "one glyph per character");
        let masks: std::collections::HashSet<(u32, u32, u32, u32)> =
            scene.texts.iter().map(|quad| return quad.mask).collect();
        assert_eq!(masks.len(), 1, "every char is the mask glyph");

        // The same field without the flag draws the real characters, which
        // is what makes the assertion above about masking and not about a
        // font that happens to have one glyph.
        let scene = build(&window_tree(vec![field(false)]));
        let masks: std::collections::HashSet<(u32, u32, u32, u32)> =
            scene.texts.iter().map(|quad| return quad.mask).collect();
        assert!(masks.len() > 1, "the plain field shows distinct glyphs");
    }

    #[test]
    fn a_required_label_draws_its_marker_after_the_text() {
        let mut label = widget("Text", 60.0, 20.0);
        label.set("content", Value::String("Name".to_string()));
        label.set("for", Value::String("field".to_string()));
        label.set("required", Value::Bool(true));
        let tree = window_tree(vec![label]);
        let scene = build(&tree);

        // Four letters plus the asterisk.
        assert_eq!(scene.texts.len(), 5);
        let marker = scene
            .texts
            .iter()
            .max_by(|left, right| return left.origin.x.total_cmp(&right.origin.x))
            .expect("glyphs");
        assert_eq!(
            marker.color, REQUIRED_COLOR,
            "the marker is the trailing glyph, in the form-marker red"
        );
        assert_eq!(
            scene
                .texts
                .iter()
                .filter(|quad| return quad.color == REQUIRED_COLOR)
                .count(),
            1,
            "exactly one marker"
        );
    }
}
