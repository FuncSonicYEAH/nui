//! Dynamic value: the unified carrier for property storage and binding
//! evaluation.

use std::fmt;

use crate::color::Color;
use crate::duration::Duration;
use crate::error::{Error, Result};
use crate::length::Length;

/// Runtime representation of nui-lang values.
///
/// Mirrors the compile-time type system:
/// `Bool/Int/Float/String/Color/Length/Duration/Enum/Model`. `Model` holds
/// the index of a host-registered model in the runtime engine's model table
/// (M4 `For` iterables); every match on `Value` across the workspace avoids
/// wildcard arms, so new variants are forced through by the compiler.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Boolean value.
    Bool(bool),
    /// 64-bit integer.
    Int(i64),
    /// 64-bit float.
    Float(f64),
    /// UTF-8 string.
    String(String),
    /// Color.
    Color(Color),
    /// Length (dp / % / auto).
    Length(Length),
    /// Duration (milliseconds).
    Duration(Duration),
    /// Enum variant name (`bold`, `ease-out`). Variants are referenced by
    /// name in nui-lang and resolved against the component registry at
    /// instantiation time.
    Enum(String),
    /// Reference to a model in the runtime engine's model table (M4): the
    /// payload is the model index handed out by the engine. A component
    /// property declared without default seeds the
    /// [`UNSET_MODEL`](Self::UNSET_MODEL) sentinel.
    Model(u32),
}

impl Value {
    /// Sentinel model index for "declared but not yet host-assigned".
    pub const UNSET_MODEL: u32 = u32::MAX;

    /// Type name (for diagnostics; matches the nui-lang type keywords).
    pub fn type_name(&self) -> &'static str {
        return match self {
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::Color(_) => "Color",
            Value::Length(_) => "Length",
            Value::Duration(_) => "Duration",
            Value::Enum(_) => "Enum",
            Value::Model(_) => "Model",
        };
    }

    /// Reads a boolean; returns [`Error::TypeMismatch`] on a wrong type.
    pub fn as_bool(&self) -> Result<bool> {
        return match self {
            Value::Bool(value) => Ok(*value),
            other => Err(Error::TypeMismatch {
                expected: "Bool",
                found: other.type_name(),
            }),
        };
    }

    /// Reads a number: `Int` and `Float` unify into f64 (the numeric channel
    /// for binding expressions); other types return [`Error::TypeMismatch`].
    ///
    /// # Examples
    /// ```
    /// use nui_core::Value;
    /// assert_eq!(Value::from(3i64).as_f64().unwrap(), 3.0);
    /// assert!(Value::from("text").as_f64().is_err());
    /// ```
    pub fn as_f64(&self) -> Result<f64> {
        return match self {
            Value::Int(value) => Ok(*value as f64),
            Value::Float(value) => Ok(*value),
            other => Err(Error::TypeMismatch {
                expected: "number",
                found: other.type_name(),
            }),
        };
    }

    /// Reads a string slice; errors on a wrong type.
    pub fn as_str(&self) -> Result<&str> {
        return match self {
            Value::String(text) => Ok(text.as_str()),
            other => Err(Error::TypeMismatch {
                expected: "String",
                found: other.type_name(),
            }),
        };
    }

    /// Reads a color; errors on a wrong type.
    pub fn as_color(&self) -> Result<Color> {
        return match self {
            Value::Color(color) => Ok(*color),
            other => Err(Error::TypeMismatch {
                expected: "Color",
                found: other.type_name(),
            }),
        };
    }

    /// Reads a length; errors on a wrong type.
    pub fn as_length(&self) -> Result<Length> {
        return match self {
            Value::Length(length) => Ok(*length),
            other => Err(Error::TypeMismatch {
                expected: "Length",
                found: other.type_name(),
            }),
        };
    }

    /// Reads a duration; errors on a wrong type.
    pub fn as_duration(&self) -> Result<Duration> {
        return match self {
            Value::Duration(duration) => Ok(*duration),
            other => Err(Error::TypeMismatch {
                expected: "Duration",
                found: other.type_name(),
            }),
        };
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        return Value::Bool(value);
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        return Value::Int(value);
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        return Value::Float(value);
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        return Value::String(value);
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        return Value::String(value.to_string());
    }
}

impl From<Color> for Value {
    fn from(value: Color) -> Self {
        return Value::Color(value);
    }
}

impl From<Length> for Value {
    fn from(value: Length) -> Self {
        return Value::Length(value);
    }
}

impl From<Duration> for Value {
    fn from(value: Duration) -> Self {
        return Value::Duration(value);
    }
}

impl fmt::Display for Value {
    /// String interpolation semantics: `String` renders without quotes,
    /// other types render in their literal form.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return match self {
            Value::Bool(value) => write!(formatter, "{value}"),
            Value::Int(value) => write!(formatter, "{value}"),
            Value::Float(value) => write!(formatter, "{value}"),
            Value::String(text) => write!(formatter, "{text}"),
            Value::Color(color) => write!(formatter, "{color}"),
            Value::Length(length) => write!(formatter, "{length}"),
            Value::Duration(duration) => write!(formatter, "{duration}"),
            Value::Enum(name) => write!(formatter, "{name}"),
            Value::Model(index) => write!(formatter, "<model {index}>"),
        };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn type_names_match_language_keywords() {
        assert_eq!(Value::from(true).type_name(), "Bool");
        assert_eq!(Value::from(1i64).type_name(), "Int");
        assert_eq!(Value::from(1.5f64).type_name(), "Float");
        assert_eq!(Value::from("s").type_name(), "String");
        assert_eq!(Value::from(Color::BLACK).type_name(), "Color");
        assert_eq!(Value::from(Length::auto()).type_name(), "Length");
        assert_eq!(Value::from(Duration::ZERO).type_name(), "Duration");
        assert_eq!(Value::Enum("bold".to_string()).type_name(), "Enum");
    }

    #[test]
    fn enum_variant_displays_as_bare_name() {
        assert_eq!(Value::Enum("ease-out".to_string()).to_string(), "ease-out");
        assert!(Value::Enum("bold".to_string()).as_bool().is_err());
    }

    #[test]
    fn as_f64_unifies_int_and_float() {
        assert_eq!(Value::from(3i64).as_f64().unwrap(), 3.0);
        assert_eq!(Value::from(2.5f64).as_f64().unwrap(), 2.5);
    }

    #[test]
    fn as_f64_reports_type_mismatch() {
        let error = Value::from(true).as_f64().unwrap_err();
        let Error::TypeMismatch { expected, found } = error else {
            panic!("expected TypeMismatch, got {error:?}");
        };
        assert_eq!((expected, found), ("number", "Bool"));
    }

    #[test]
    fn as_bool_and_as_str_succeed_on_match() {
        assert!(!Value::from(false).as_bool().unwrap());
        assert_eq!(Value::from("hello").as_str().unwrap(), "hello");
    }

    #[test]
    fn typed_accessors_roundtrip() {
        let color = Color::from_rgb8(1, 2, 3);
        assert_eq!(Value::from(color).as_color().unwrap(), color);
        assert_eq!(
            Value::from(Length::percent(25.0)).as_length().unwrap(),
            Length::percent(25.0)
        );
        assert_eq!(
            Value::from(Duration::from_millis(5.0))
                .as_duration()
                .unwrap(),
            Duration::from_millis(5.0)
        );
    }

    #[test]
    fn display_matches_interpolation_semantics() {
        assert_eq!(Value::from("raw").to_string(), "raw");
        assert_eq!(Value::from(42i64).to_string(), "42");
        assert_eq!(Value::from(1.5f64).to_string(), "1.5");
        assert_eq!(Value::from(true).to_string(), "true");
        assert_eq!(Value::from(Length::dp(10.0)).to_string(), "10dp");
        assert_eq!(
            Value::from(Duration::from_millis(200.0)).to_string(),
            "200ms"
        );
        assert_eq!(
            Value::from(Color::from_rgb8(255, 0, 0)).to_string(),
            "#ff0000ff"
        );
    }
}
