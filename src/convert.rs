//! Conversions between Rust types and [`Value`].

use std::any::TypeId;

use boa_gc::Trace;

use crate::error::{Error, Result};
use crate::value::Value;

/// Types that can become a [`Value`].
///
/// Blanket-implemented for the scalar shapes, so `Value::from(1i64)` and
/// `Into::<Value>::into("hi")` both work. Implement it for your own type to
/// make it first-class.
pub trait IntoValue {
    /// Perform the conversion.
    fn into_value(self) -> Value;
}

/// Types that can be recovered from a [`Value`].
///
/// Deliberately separate from [`IntoValue`]: converting `f64` into a `Value`
/// should always succeed, while reading one back as `i64` may legitimately
/// fail. A single `From` pair could not express that asymmetry.
pub trait FromValue: Sized {
    /// Attempt the conversion, naming `context` in any type error.
    fn from_value(value: &Value, context: &'static str) -> Result<Self>;
}

macro_rules! into_value_int {
    ($($t:ty),* $(,)?) => {$(
        impl IntoValue for $t {
            fn into_value(self) -> Value {
                Value::Int(self as i64)
            }
        }

        impl FromValue for $t {
            fn from_value(value: &Value, context: &'static str) -> Result<Self> {
                match value {
                    Value::Int(i) => <$t>::try_from(*i).map_err(|_| Error::TypeMismatch {
                        context,
                        expected: stringify!($t),
                        found: "out-of-range int",
                    }),
                    other => Err(Error::TypeMismatch {
                        context,
                        expected: stringify!($t),
                        found: other.type_name(),
                    }),
                }
            }
        }
    )*};
}

into_value_int!(i8, i16, i32, i64, u8, u16, u32);

macro_rules! from_value_lossy_int {
    ($($t:ty),* $(,)?) => {$(
        impl FromValue for $t {
            fn from_value(value: &Value, context: &'static str) -> Result<Self> {
                <i64 as FromValue>::from_value(value, context).map(|i| i as $t)
            }
        }
    )*};
}

from_value_lossy_int!(isize, usize);

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::Float(self as f64)
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Value {
        Value::Float(self)
    }
}

impl FromValue for f32 {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        f64::from_value(value, context).map(|f| f as f32)
    }
}

impl FromValue for f64 {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        value.as_float().ok_or(Error::TypeMismatch {
            context,
            expected: "float",
            found: value.type_name(),
        })
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl FromValue for bool {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        value.as_bool().ok_or(Error::TypeMismatch {
            context,
            expected: "bool",
            found: value.type_name(),
        })
    }
}

impl IntoValue for char {
    fn into_value(self) -> Value {
        Value::Char(self)
    }
}

impl FromValue for char {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        value.as_char().ok_or(Error::TypeMismatch {
            context,
            expected: "char",
            found: value.type_name(),
        })
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::string(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::str(self)
    }
}

impl FromValue for String {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or(Error::TypeMismatch {
                context,
                expected: "str",
                found: value.type_name(),
            })
    }
}

impl FromValue for TypeId {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        value.as_type_id().ok_or(Error::TypeMismatch {
            context,
            expected: "type",
            found: value.type_name(),
        })
    }
}

impl<T: Trace + 'static> IntoValue for ValueUser<T> {
    fn into_value(self) -> Value {
        Value::user(self.0)
    }
}

/// A traced payload on its way into a [`Value`], so user types get
/// [`IntoValue`] without wrapping every call site.
pub struct ValueUser<T: Trace + 'static>(pub T);

impl<T: Trace + 'static> ValueUser<T> {
    /// Wrap a traced payload.
    pub fn new(v: T) -> Self {
        Self(v)
    }

    /// Recover the payload.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::Int(v as i64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<char> for Value {
    fn from(v: char) -> Self {
        Value::Char(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::str(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::string(v)
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::bytes(v)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::list_from(v)
    }
}

impl<V: IntoValue> From<Option<V>> for Value {
    fn from(v: Option<V>) -> Self {
        match v {
            Some(v) => v.into_value(),
            None => Value::Nil,
        }
    }
}
