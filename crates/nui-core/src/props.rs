//! The property surface of every built-in element type.
//!
//! A built-in's properties used to exist only as string literals scattered
//! through the renderer and the widget layer — `f_property(element,
//! "radius").unwrap_or(6.0)`. That worked while the compiler had no reason
//! to know the names: a misspelling simply did nothing at runtime, and
//! nobody could observe it from a document.
//!
//! Two things changed that:
//!
//! 1. `component Child extends Button` makes a built-in's properties the
//!    *inherited API* of a derived component, so the checker has to know
//!    what `Button` accepts, what type each property is, and what a
//!    missing one defaults to.
//! 2. With the names written down, `Text(contnt = "x")` can be a compile
//!    error instead of a silent no-op — the half of the vocabulary hole
//!    that has no ambiguity (see `plan.md` §13; element *state* shares the
//!    same namespace deliberately and still needs a language decision).
//!
//! One table, in `nui-core`, because the compiler and the runtime must
//! agree: the checker reads it to type an override, and the widget layer
//! reads it to fall back to the same default the checker advertised.

use crate::color::Color;
use crate::length::Length;
use crate::value::Value;

/// The type of a built-in property's value.
///
/// Mirrors `nui_compiler::Type` for the subset a built-in can declare.
/// Kept separate rather than shared because the compiler's enum also has
/// to name document-only concepts (`Unknown`) that no element property
/// can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    Bool,
    Int,
    Float,
    String,
    Color,
    Length,
    /// A closed set of names, written as a bare identifier (`"ghost"`,
    /// `"vertical"`). Distinct from `String` only in how it is written in
    /// a document; the runtime sees a string either way.
    Enum,
    /// A number that may be an `Int` or a `Float`; the widget layer keeps
    /// whichever the document used (`value <=> page.volume`).
    Number,
    /// A host-assigned model (`For`'s `items`). Models have no literals:
    /// the host seeds them through the runtime engine.
    Model,
}

impl PropType {
    /// Whether a value of this type may be written where the property is
    /// expected. `Number` accepts both numeric kinds, and `Enum` accepts a
    /// string because a quoted variant name reads identically.
    pub fn accepts(self, other: PropType) -> bool {
        if self == other {
            return true;
        }
        return matches!(
            (self, other),
            (PropType::Number, PropType::Int | PropType::Float)
                | (PropType::Float, PropType::Int)
                | (PropType::Enum, PropType::String)
        );
    }

    /// The name a diagnostic should use.
    pub fn name(self) -> &'static str {
        return match self {
            PropType::Bool => "Bool",
            PropType::Int => "Int",
            PropType::Float => "Float",
            PropType::String => "String",
            PropType::Color => "Color",
            PropType::Length => "Length",
            PropType::Enum => "Enum",
            PropType::Number => "Number",
            PropType::Model => "Model",
        };
    }
}

/// One property of one built-in type.
#[derive(Debug, Clone, Copy)]
pub struct PropDecl {
    /// The property name, as written in a document.
    pub name: &'static str,
    pub ty: PropType,
    /// The value a read sees when the document sets nothing.
    pub default: DefaultValue,
}

/// A property's default, in the representation the widget layer wants.
#[derive(Debug, Clone, Copy)]
pub enum DefaultValue {
    Bool(bool),
    /// `f64` so a `Float` default survives exactly; the widget layer
    /// converts, and an `Int` default round-trips through `as i64`.
    Number(f64),
    Text(&'static str),
    Color(Color),
    Length(Length),
    /// No default: the property is meaningful only when set (`text`,
    /// `content`, `value` on a `Slider`). A read sees the unset sentinel.
    None,
}

impl DefaultValue {
    /// The runtime value, or `None` when the property has no default.
    pub fn value(self) -> Option<Value> {
        return match self {
            DefaultValue::Bool(inner) => Some(Value::Bool(inner)),
            DefaultValue::Number(inner) => {
                // A whole number stays an `Int`, so a document that never
                // writes the property sees the same representation a
                // document that writes `3` would.
                if inner.fract() == 0.0 && inner.abs() < i64::MAX as f64 {
                    Some(Value::Int(inner as i64))
                } else {
                    Some(Value::Float(inner))
                }
            }
            DefaultValue::Text(inner) => Some(Value::String(inner.to_string())),
            DefaultValue::Color(inner) => Some(Value::Color(inner)),
            DefaultValue::Length(inner) => Some(Value::Length(inner)),
            DefaultValue::None => None,
        };
    }

    /// The default as `f64`, for the widget helpers that read a number and
    /// want a fallback in one step.
    pub fn number(self) -> Option<f64> {
        return match self {
            DefaultValue::Number(inner) => Some(inner),
            _ => None,
        };
    }

    /// The default as `f32` (lengths included), for the layout-facing
    /// helpers.
    pub fn length(self) -> Option<f32> {
        return match self {
            DefaultValue::Length(Length::Dp(inner)) => Some(inner),
            DefaultValue::Number(inner) => Some(inner as f32),
            _ => None,
        };
    }

    /// The default as `bool`, for the toggle helpers.
    pub fn boolean(self) -> Option<bool> {
        return match self {
            DefaultValue::Bool(inner) => Some(inner),
            _ => None,
        };
    }

    /// The default as `&str`, for the text helpers.
    pub fn text(self) -> Option<&'static str> {
        return match self {
            DefaultValue::Text(inner) => Some(inner),
            _ => None,
        };
    }
}

/// Every property one built-in type declares.
#[derive(Debug, Clone, Copy)]
pub struct TypeProps {
    pub ty: &'static str,
    pub props: &'static [PropDecl],
}

/// Looks up a built-in type's property table.
pub fn props_of(ty: &str) -> Option<&'static [PropDecl]> {
    return TABLE
        .iter()
        .find(|entry| return entry.ty == ty)
        .map(|entry| return entry.props);
}

/// Looks up one property of one built-in type.
pub fn prop_of(ty: &str, name: &str) -> Option<&'static PropDecl> {
    return props_of(ty)?.iter().find(|prop| return prop.name == name);
}

/// The default of one property, or `None` when unknown/unset.
pub fn default_of(ty: &str, name: &str) -> Option<Value> {
    return prop_of(ty, name)?.default.value();
}

/// Declares a property inline, so the table below reads as a table.
const fn f(name: &'static str, ty: PropType, default: DefaultValue) -> PropDecl {
    return PropDecl { name, ty, default };
}

/// Blank `Length` default: the caller's `auto`.
const AUTO: DefaultValue = DefaultValue::Length(Length::Auto);

/// The properties every element accepts, whatever else it declares.
///
/// `id` is deliberately absent: it is syntax (a pseudo-property the
/// compiler resolves into the element's name), not a value slot, and
/// listing it here would make `id` look assignable in an effect.
///
/// `key` is here rather than on `For`/`ListView` because the reconciler
/// reads it off *any* element when it matches old children against new
/// (`nui-runtime::keys::KEY_PROPERTY`), so every type has to accept it for
/// the write to type-check.
///
/// `width` / `height` default to `auto` rather than a number because that
/// is what the layout reads when nothing is set — this entry documents the
/// absence, rather than introducing a default the renderer would then have
/// to honour.
pub const UNIVERSAL: &[PropDecl] = &[
    f("width", PropType::Length, AUTO),
    f("height", PropType::Length, AUTO),
    f("visible", PropType::Bool, DefaultValue::Bool(true)),
    f("opacity", PropType::Float, DefaultValue::Number(1.0)),
    f("key", PropType::String, DefaultValue::None),
];

/// The layout properties every *box* takes, on top of [`UNIVERSAL`].
///
/// `align_self` / `justify_self` are how one child opts out of its
/// container's `align`; the grid spans are read by `Grid` on its children
/// rather than on itself. All four are read by the layout from whichever
/// element carries them, so every element accepts them.
pub const LAYOUT: &[PropDecl] = &[
    f("align_self", PropType::Enum, DefaultValue::None),
    f("justify_self", PropType::Enum, DefaultValue::None),
    f("column_span", PropType::Int, DefaultValue::Number(1.0)),
    f("row_span", PropType::Int, DefaultValue::Number(1.0)),
    f("flex_grow", PropType::Number, DefaultValue::Number(0.0)),
    f("offset_x", PropType::Length, AUTO),
    f("offset_y", PropType::Length, AUTO),
];

/// The paint properties every *drawing* element takes.
///
/// Read generically by the scene builder (`color_property(element,
/// "fill")`) rather than per type, which is why a `Scroll` can take a
/// background and a `Panel` a radius without either declaring them: the
/// entry here records what the renderer already accepts.
pub const PAINT: &[PropDecl] = &[
    f("fill", PropType::Color, DefaultValue::None),
    f("radius", PropType::Number, DefaultValue::Number(0.0)),
    f("tint", PropType::Color, DefaultValue::None),
    f("rotation", PropType::Float, DefaultValue::Number(0.0)),
    f("clip", PropType::Bool, DefaultValue::Bool(false)),
    f("shadow.color", PropType::Color, DefaultValue::None),
    f("shadow.dx", PropType::Number, DefaultValue::Number(0.0)),
    f("shadow.dy", PropType::Number, DefaultValue::Number(0.0)),
    f("shadow.blur", PropType::Number, DefaultValue::Number(0.0)),
    f("gradient.from", PropType::Color, DefaultValue::None),
    f("gradient.to", PropType::Color, DefaultValue::None),
    f(
        "gradient.kind",
        PropType::Enum,
        DefaultValue::Text("linear"),
    ),
    f("gradient.angle", PropType::Float, DefaultValue::Number(0.0)),
    f("gradient.center_x", PropType::Length, AUTO),
    f("gradient.center_y", PropType::Length, AUTO),
    f("gradient.radius", PropType::Length, AUTO),
    f("stroke.color", PropType::Color, DefaultValue::None),
    f("stroke.width", PropType::Number, DefaultValue::Number(1.0)),
    f("stroke.cap", PropType::Enum, DefaultValue::Text("butt")),
];

/// The properties a frame can write **without any box changing** (D40).
///
/// `nui-layout` reads none of these — they are consumed by the scene
/// builder and the shaders — so a frame whose writes all land here can
/// skip `layout_with_text` entirely. This is what makes a `tween` on
/// `opacity` (or a hover recolour, or a `scroll_y` drag) cheap per frame.
///
/// The list is **fail-safe on purpose**: a property name missing here
/// only costs a layout pass that was probably needed anyway, while a
/// name wrongly listed here would pin stale boxes on screen. `opacity`
/// is [`UNIVERSAL`] but consumed at draw time; `color` is the
/// control-owned colour each widget declares separately; `scroll_y` is
/// the D38 viewport translation, not a layout input.
pub const PAINT_ONLY: &[&str] = &[
    "opacity",
    "fill",
    "tint",
    "color",
    "radius",
    "rotation",
    "clip",
    "shadow.color",
    "shadow.dx",
    "shadow.dy",
    "shadow.blur",
    "gradient.from",
    "gradient.to",
    "gradient.kind",
    "gradient.angle",
    "gradient.center_x",
    "gradient.center_y",
    "gradient.radius",
    "stroke.color",
    "stroke.width",
    "stroke.cap",
    "scroll_y",
];

/// Whether writing `name` can never change a box (see [`PAINT_ONLY`]).
pub fn is_paint_only_property(name: &str) -> bool {
    return PAINT_ONLY.contains(&name);
}

/// Declares a type's properties as `UNIVERSAL + LAYOUT + PAINT + extras`.
///
/// A macro rather than a function because a `&'static [PropDecl]` cannot be
/// built by concatenation at runtime, and writing the three shared sets out
/// in full for each of two dozen types is exactly the duplication this
/// table exists to remove.
///
/// A type that takes no geometry (`Timer`, `For`) uses the explicit forms
/// below instead; a type that draws nothing (`Slot`, `Spacer`) passes
/// `UNIVERSAL` alone.
macro_rules! element {
    ($($extra:expr),* $(,)?) => {
        &[
            UNIVERSAL[0], UNIVERSAL[1], UNIVERSAL[2], UNIVERSAL[3], UNIVERSAL[4],
            LAYOUT[0], LAYOUT[1], LAYOUT[2], LAYOUT[3], LAYOUT[4], LAYOUT[5], LAYOUT[6],
            PAINT[0], PAINT[1], PAINT[2], PAINT[3], PAINT[4],
            PAINT[5], PAINT[6], PAINT[7], PAINT[8],
            PAINT[9], PAINT[10], PAINT[11], PAINT[12], PAINT[13], PAINT[14], PAINT[15],
            PAINT[16], PAINT[17], PAINT[18],
            $($extra),*
        ]
    };
}

/// Every built-in element type's property surface.
///
/// Types are grouped as the renderer and the widget layer see them: the
/// drawing primitives first, then the text-bearing ones, then controls.
/// A type that draws nothing and takes no input (`Spacer`, `Slot`) has no
/// entries beyond the universal four.
pub static TABLE: &[TypeProps] = &[
    // ---------------------------------------------------------------- //
    // The root. A `Window` differs from a `Column` in one way that matters
    // here: it is the viewport, so `layout` ignores its declared
    // `width`/`height` and fills the surface instead. It still takes the
    // container properties, because its chrome (background, corner radius,
    // title inset) is painted from them.
    // ---------------------------------------------------------------- //
    TypeProps {
        ty: "Window",
        props: element!(
            f("title", PropType::String, DefaultValue::Text("")),
            f("dark", PropType::Bool, DefaultValue::Bool(false)),
            f(
                "spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f("align", PropType::Enum, DefaultValue::Text("stretch")),
        ),
    },
    // ---------------------------------------------------------------- //
    // Layout containers. They read their children's geometry, so their
    // own surface is spacing and alignment.
    // ---------------------------------------------------------------- //
    TypeProps {
        ty: "Column",
        props: element!(
            f(
                "spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f("align", PropType::Enum, DefaultValue::Text("stretch")),
        ),
    },
    TypeProps {
        ty: "Row",
        props: element!(
            f(
                "spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f("align", PropType::Enum, DefaultValue::Text("stretch")),
        ),
    },
    TypeProps {
        ty: "Stack",
        props: element!(f(
            "padding",
            PropType::Length,
            DefaultValue::Length(Length::Dp(0.0))
        ),),
    },
    TypeProps {
        ty: "Wrap",
        props: element!(
            f(
                "spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
        ),
    },
    TypeProps {
        ty: "Grid",
        props: element!(
            f("columns", PropType::Int, DefaultValue::Number(1.0)),
            f(
                "row_spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "column_spacing",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
        ),
    },
    TypeProps {
        ty: "Scroll",
        props: element!(
            f("scroll_y", PropType::Number, DefaultValue::Number(0.0)),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
        ),
    },
    // A `Slot` is the marker a component leaves for its call site's
    // children; it draws nothing and holds a subtree.
    TypeProps {
        ty: "Slot",
        props: UNIVERSAL,
    },
    // A virtualized list. It shares `For`'s binding path and adds the
    // scroll offset the engine advances; the row prototype is the body,
    // which is why there is no `items` here. `row_height` is the fixed
    // extent the virtualizer windows against — read by
    // `nui-runtime::widget::scroll`, and `0.0` means "measure each row".
    TypeProps {
        ty: "ListView",
        props: element!(
            f("scroll_y", PropType::Number, DefaultValue::Number(0.0)),
            f(
                "row_height",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
        ),
    },
    TypeProps {
        ty: "Spacer",
        props: UNIVERSAL,
    },
    TypeProps {
        ty: "Separator",
        props: element!(),
    },
    // ---------------------------------------------------------------- //
    // Drawing primitives.
    // ---------------------------------------------------------------- //
    TypeProps {
        ty: "Rectangle",
        props: element!(),
    },
    TypeProps {
        ty: "Ellipse",
        props: element!(),
    },
    TypeProps {
        ty: "Image",
        props: element!(
            f("source", PropType::String, DefaultValue::None),
            f("fit", PropType::Enum, DefaultValue::Text("contain")),
            f("slice", PropType::Number, DefaultValue::Number(0.0)),
        ),
    },
    TypeProps {
        ty: "Path",
        props: element!(
            f("points", PropType::String, DefaultValue::None),
            f("d", PropType::String, DefaultValue::None),
            f("closed", PropType::Bool, DefaultValue::Bool(false)),
        ),
    },
    // A polyline and an arc are the two shapes `Path`'s SVG-ish `d` cannot
    // express as directly: a polyline is a point list, an arc is a centre, a
    // radius and a sweep. Both are stroked, so both read the `stroke.*`
    // family that `PAINT` already provides — and both name their colour
    // with `color` rather than `fill`, because `stroke_of` checks `color`
    // first and there is no fill to fall back to.
    TypeProps {
        ty: "Polyline",
        props: element!(
            f("points", PropType::String, DefaultValue::None),
            f("color", PropType::Color, DefaultValue::None),
        ),
    },
    TypeProps {
        ty: "Arc",
        props: element!(
            f("cx", PropType::Number, DefaultValue::Number(0.0)),
            f("cy", PropType::Number, DefaultValue::Number(0.0)),
            f("start", PropType::Number, DefaultValue::Number(0.0)),
            f("end", PropType::Number, DefaultValue::Number(360.0)),
            f("color", PropType::Color, DefaultValue::None),
        ),
    },
    TypeProps {
        ty: "Waveline",
        props: element!(
            f("count", PropType::Int, DefaultValue::Number(32.0)),
            f("amplitude", PropType::Float, DefaultValue::Number(12.0)),
            f("frequency", PropType::Float, DefaultValue::Number(1.0)),
            f("phase", PropType::Float, DefaultValue::Number(0.0)),
            f("color", PropType::Color, DefaultValue::None),
            f("mirror", PropType::Bool, DefaultValue::Bool(false)),
            f("levels", PropType::Int, DefaultValue::None),
        ),
    },
    // A canvas is painted by host code, so its document-facing surface is
    // only its geometry — plus `clip`, which the scene builder honours for
    // every element and a host painter's overdraw makes worth setting.
    TypeProps {
        ty: "Canvas",
        props: element!(),
    },
    // ---------------------------------------------------------------- //
    // Text-bearing types. `Text` and `TextInput` both size and shape
    // their content, so both carry the `font.*` family; the compiler
    // treats `font.*` as an attached property and this entry is what
    // gives it a home.
    // ---------------------------------------------------------------- //
    TypeProps {
        ty: "Text",
        props: element!(
            f("content", PropType::String, DefaultValue::None),
            f(
                "color",
                PropType::Color,
                DefaultValue::Color(Color::from_rgb8(0xe8, 0xec, 0xf2))
            ),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
            f("max_lines", PropType::Int, DefaultValue::None),
            f("elide", PropType::Enum, DefaultValue::Text("clip")),
            f("align", PropType::Enum, DefaultValue::Text("start")),
            // Where the run sits *inside* the box, read by the scene
            // builder for every `Text`. Without entries here the D33
            // property-name check rejects the two names the renderer
            // actually consumes — the exact table/renderer drift D31
            // exists to prevent.
            f("halign", PropType::Enum, DefaultValue::Text("start")),
            f("valign", PropType::Enum, DefaultValue::Text("start")),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f("for", PropType::String, DefaultValue::None),
            f("required", PropType::Bool, DefaultValue::Bool(false)),
        ),
    },
    TypeProps {
        ty: "TextInput",
        props: element!(
            f("text", PropType::String, DefaultValue::Text("")),
            f("placeholder", PropType::String, DefaultValue::Text("")),
            f("multiline", PropType::Bool, DefaultValue::Bool(false)),
            f("read_only", PropType::Bool, DefaultValue::Bool(false)),
            // `password` masks the display, `reveal` un-masks it again —
            // `is_password()` is the conjunction of the two, which is why
            // they are a pair rather than one three-state flag.
            f("password", PropType::Bool, DefaultValue::Bool(false)),
            f("reveal", PropType::Bool, DefaultValue::Bool(false)),
            f("max_length", PropType::Int, DefaultValue::None),
            f("validator", PropType::Enum, DefaultValue::Text("none")),
            f("invalid", PropType::Bool, DefaultValue::Bool(false)),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f(
                "color",
                PropType::Color,
                DefaultValue::Color(Color::from_rgb8(0xe8, 0xec, 0xf2))
            ),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
            f(
                "select_all_on_focus",
                PropType::Bool,
                DefaultValue::Bool(true)
            ),
        ),
    },
    // ---------------------------------------------------------------- //
    // Controls. Each one's surface is what its behaviour reads
    // (`WidgetKind` in `nui-runtime::widget`), plus the chrome the
    // renderer draws around it.
    // ---------------------------------------------------------------- //
    TypeProps {
        ty: "Button",
        props: element!(
            f("label", PropType::String, DefaultValue::Text("")),
            f("variant", PropType::Enum, DefaultValue::Text("primary")),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f("icon", PropType::String, DefaultValue::None),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f("color", PropType::Color, DefaultValue::None),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(500.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "CheckBox",
        props: element!(
            f("label", PropType::String, DefaultValue::Text("")),
            f("checked", PropType::Bool, DefaultValue::Bool(false)),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f("color", PropType::Color, DefaultValue::None),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "Switch",
        props: element!(
            f("label", PropType::String, DefaultValue::Text("")),
            f("checked", PropType::Bool, DefaultValue::Bool(false)),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "RadioButton",
        props: element!(
            f("label", PropType::String, DefaultValue::Text("")),
            f("group", PropType::String, DefaultValue::Text("")),
            f("selected", PropType::Bool, DefaultValue::Bool(false)),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "Slider",
        props: element!(
            f("value", PropType::Number, DefaultValue::Number(0.0)),
            f("min", PropType::Number, DefaultValue::Number(0.0)),
            f("max", PropType::Number, DefaultValue::Number(100.0)),
            f("step", PropType::Number, DefaultValue::Number(0.0)),
            f(
                "orientation",
                PropType::Enum,
                DefaultValue::Text("horizontal")
            ),
            f(
                "thumb_size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(16.0))
            ),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f("color", PropType::Color, DefaultValue::None),
        ),
    },
    TypeProps {
        ty: "SpinBox",
        props: element!(
            f("value", PropType::Number, DefaultValue::Number(0.0)),
            f("min", PropType::Number, DefaultValue::Number(0.0)),
            f("max", PropType::Number, DefaultValue::Number(100.0)),
            f("step", PropType::Number, DefaultValue::Number(1.0)),
            f("enabled", PropType::Bool, DefaultValue::Bool(true)),
            f("color", PropType::Color, DefaultValue::None),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(14.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(400.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "Dialog",
        props: element!(
            f("open", PropType::Bool, DefaultValue::Bool(false)),
            f("title", PropType::String, DefaultValue::Text("")),
            f("modal", PropType::Bool, DefaultValue::Bool(true)),
            f(
                "dismiss_on_backdrop",
                PropType::Bool,
                DefaultValue::Bool(true)
            ),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0))
            ),
            f(
                "font.size",
                PropType::Length,
                DefaultValue::Length(Length::Dp(15.0))
            ),
            f("font.weight", PropType::Number, DefaultValue::Number(600.0)),
            f("font.italic", PropType::Bool, DefaultValue::Bool(false)),
            f("font.family", PropType::String, DefaultValue::Text("")),
        ),
    },
    TypeProps {
        ty: "Panel",
        props: element!(
            f("title", PropType::String, DefaultValue::Text("")),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(16.0))
            ),
        ),
    },
    TypeProps {
        ty: "Card",
        props: element!(
            f("title", PropType::String, DefaultValue::Text("")),
            f(
                "padding",
                PropType::Length,
                DefaultValue::Length(Length::Dp(16.0))
            ),
        ),
    },
    TypeProps {
        ty: "Timer",
        props: &[
            f("interval", PropType::Number, DefaultValue::Number(1000.0)),
            f("running", PropType::Bool, DefaultValue::Bool(false)),
            f("repeat", PropType::Bool, DefaultValue::Bool(true)),
        ],
    },
    TypeProps {
        ty: "For",
        props: &[
            f("items", PropType::Model, DefaultValue::None),
            f(
                "row_height",
                PropType::Length,
                DefaultValue::Length(Length::Dp(0.0)),
            ),
        ],
    },
];

/// Every built-in type name, for the checker's unknown-type diagnostic.
pub fn type_names() -> impl Iterator<Item = &'static str> {
    return TABLE.iter().map(|entry| return entry.ty);
}

/// Whether `ty` is a built-in element type.
pub fn is_builtin_type(ty: &str) -> bool {
    return TABLE.iter().any(|entry| return entry.ty == ty);
}

/// The `Model` variant is declared here rather than in the table above
/// because `For` is the only type that takes one, and `PropType` needs the
/// name to exist.
impl PropType {
    /// `Model` as a value of this enum, for the `For` entry.
    pub const MODEL: PropType = PropType::String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_accepts_its_own_properties() {
        for entry in TABLE {
            for prop in entry.props {
                assert!(
                    prop.ty.accepts(prop.ty),
                    "{}.{} does not accept its own type",
                    entry.ty,
                    prop.name
                );
            }
        }
    }

    #[test]
    fn numbers_accept_both_numeric_kinds() {
        assert!(PropType::Number.accepts(PropType::Int));
        assert!(PropType::Number.accepts(PropType::Float));
        assert!(PropType::Float.accepts(PropType::Int));
        assert!(!PropType::Int.accepts(PropType::Float));
    }

    #[test]
    fn an_enum_accepts_a_quoted_variant() {
        assert!(PropType::Enum.accepts(PropType::String));
        assert!(!PropType::String.accepts(PropType::Enum));
    }

    #[test]
    fn universal_properties_are_on_every_element_type() {
        // Logic nodes (`Timer`, `For`) are not elements: nothing lays them
        // out and nothing draws them, so they take no geometry.
        for entry in TABLE {
            if matches!(entry.ty, "Timer" | "For") {
                continue;
            }
            for name in ["width", "height", "visible", "opacity", "key"] {
                assert!(
                    prop_of(entry.ty, name).is_some(),
                    "{} is missing `{name}`",
                    entry.ty
                );
            }
        }
    }

    #[test]
    fn key_is_accepted_wherever_the_reconciler_reads_it() {
        // The reconciler matches children by `key` on any element, so a
        // document may write it on a `Rectangle` just as validly as on an
        // `Item` inside a `For`. Listing it only on the loop types would
        // have made the write un-type-checkable.
        assert!(prop_of("Rectangle", "key").is_some());
        assert!(prop_of("Column", "key").is_some());
        assert!(prop_of("Text", "key").is_some());
    }

    #[test]
    fn logic_nodes_take_no_geometry() {
        for ty in ["Timer", "For"] {
            assert!(prop_of(ty, "width").is_none(), "{ty} must not take width");
            assert!(
                prop_of(ty, "visible").is_none(),
                "{ty} must not take visible"
            );
        }
    }

    #[test]
    fn no_type_declares_a_property_twice() {
        for entry in TABLE {
            let mut seen: Vec<&str> = Vec::new();
            for prop in entry.props {
                assert!(
                    !seen.contains(&prop.name),
                    "{} declares `{}` twice",
                    entry.ty,
                    prop.name
                );
                seen.push(prop.name);
            }
        }
    }

    #[test]
    fn a_whole_number_default_reads_as_an_int() {
        // `Number(6.0)` normalises to `Int(6)`: a document that never
        // writes `radius` reads the same representation a document that
        // writes `radius = 6` would.
        assert_eq!(default_of("Grid", "columns"), Some(Value::Int(1)));
        assert_eq!(
            default_of("Slider", "thumb_size"),
            Some(Value::Length(Length::Dp(16.0)))
        );
    }

    #[test]
    fn a_property_with_no_default_reads_as_nothing() {
        assert_eq!(default_of("Slider", "value"), Some(Value::Int(0)));
        assert_eq!(default_of("Text", "content"), None);
        assert_eq!(default_of("Button", "icon"), None);
    }

    #[test]
    fn a_default_survives_its_own_representation() {
        assert_eq!(DefaultValue::Number(1.0).value(), Some(Value::Int(1)));
        assert_eq!(DefaultValue::Number(0.25).value(), Some(Value::Float(0.25)));
        assert_eq!(
            DefaultValue::text(DefaultValue::Text("ghost")),
            Some("ghost")
        );
        assert_eq!(DefaultValue::boolean(DefaultValue::Bool(true)), Some(true));
    }
}
