//! The nui-lang type system used by the checker.

use std::fmt;

/// Compile-time types of nui-lang.
///
/// `Unknown` marks expressions whose type could not be determined (access
/// into `parent`, or results of already-reported errors); operations on it
/// stay silent so one error does not cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    /// Boolean.
    Bool,
    /// 64-bit integer.
    Int,
    /// 64-bit float.
    Float,
    /// UTF-8 string.
    String,
    /// Color.
    Color,
    /// Length (dp / % / auto).
    Length,
    /// Duration (milliseconds).
    Duration,
    /// Enum variant name (`bold`, `ease-out`). Variants carry their name as
    /// a string at runtime, so `String` and `Enum` unify (a quoted variant
    /// name is accepted where an enum is expected).
    Enum,
    /// Reference to a host-registered model (`For` iterables, M4). Models
    /// have no literals; the host assigns them through the runtime engine.
    Model,
    /// Undetermined; suppresses further type errors.
    Unknown,
}

impl Type {
    /// Maps a type keyword to its type; `None` for unknown names.
    pub fn from_name(name: &str) -> Option<Type> {
        return match name {
            "Bool" => Some(Type::Bool),
            "Int" => Some(Type::Int),
            "Float" => Some(Type::Float),
            "String" => Some(Type::String),
            "Color" => Some(Type::Color),
            "Length" => Some(Type::Length),
            "Duration" => Some(Type::Duration),
            "Enum" => Some(Type::Enum),
            "Model" => Some(Type::Model),
            _ => None,
        };
    }

    /// The type's nui-lang name.
    pub fn name(&self) -> &'static str {
        return match self {
            Type::Bool => "Bool",
            Type::Int => "Int",
            Type::Float => "Float",
            Type::String => "String",
            Type::Color => "Color",
            Type::Length => "Length",
            Type::Duration => "Duration",
            Type::Enum => "Enum",
            Type::Model => "Model",
            Type::Unknown => "?",
        };
    }

    /// Whether the type participates in numeric arithmetic.
    pub fn is_numeric(&self) -> bool {
        return matches!(self, Type::Int | Type::Float | Type::Unknown);
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(formatter, "{}", self.name());
    }
}

/// Unifies two types for arithmetic, comparisons, and ternary branches.
///
/// `Unknown` unifies with anything (the other side wins); `Int` and `Float`
/// unify to `Float`; identical types unify to themselves. Returns `None`
/// when the types are incompatible.
pub fn unify(left: Type, right: Type) -> Option<Type> {
    if left == Type::Unknown {
        return Some(right);
    }
    if right == Type::Unknown {
        return Some(left);
    }
    if left == right {
        return Some(left);
    }
    if matches!(
        (left, right),
        (Type::Int, Type::Float) | (Type::Float, Type::Int)
    ) {
        return Some(Type::Float);
    }
    if matches!(
        (left, right),
        (Type::Enum, Type::String) | (Type::String, Type::Enum)
    ) {
        return Some(Type::Enum);
    }
    return None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_matches_keywords() {
        assert_eq!(Type::from_name("Int"), Some(Type::Int));
        assert_eq!(Type::from_name("Duration"), Some(Type::Duration));
        assert_eq!(Type::from_name("Model"), Some(Type::Model));
        assert_eq!(Type::from_name("Widget"), None);
    }

    #[test]
    fn unify_numeric_promotes_to_float() {
        assert_eq!(unify(Type::Int, Type::Float), Some(Type::Float));
        assert_eq!(unify(Type::Int, Type::Int), Some(Type::Int));
    }

    #[test]
    fn unify_unknown_absorbs() {
        assert_eq!(unify(Type::Unknown, Type::Color), Some(Type::Color));
        assert_eq!(unify(Type::Length, Type::Unknown), Some(Type::Length));
    }

    #[test]
    fn unify_rejects_incompatible() {
        assert_eq!(unify(Type::Bool, Type::Int), None);
        assert_eq!(unify(Type::String, Type::Color), None);
    }

    #[test]
    fn unify_enum_accepts_string_variant_names() {
        assert_eq!(unify(Type::Enum, Type::String), Some(Type::Enum));
        assert_eq!(unify(Type::Enum, Type::Enum), Some(Type::Enum));
        assert_eq!(unify(Type::Enum, Type::Int), None);
    }
}
