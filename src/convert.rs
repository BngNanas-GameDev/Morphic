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
///
/// # `u64` is deliberately absent
///
/// The covered integers are `i8`–`i64` and `u8`–`u32`. `u64` is left out on
/// purpose: the top half of its range does not fit [`Value::Int`], and
/// `into_value` has no way to report that — an infallible conversion that
/// silently rewrites `u64::MAX` as `-1` is worse than no conversion at all.
/// Use [`Value::try_from`] instead, which returns [`Error::OutOfRange`] for
/// anything above `i64::MAX`.
///
/// Note that the conversion has to be *named* in the type position, as in
/// `let v = Value::try_from(n)?`. The usual `n.try_into()` spelling is
/// ambiguous here, because `u64: TryInto<u64>` already resolves and shadows
/// the `Value` target.
///
/// # Related traits
///
/// Reading a [`Value`] back out is [`FromValue`], which can fail and so is
/// deliberately a separate trait.
pub trait IntoValue {
    /// Perform the conversion.
    fn into_value(self) -> Value;
}

/// Types that can be recovered from a [`Value`].
///
/// Deliberately separate from [`IntoValue`]: converting `f64` into a `Value`
/// should always succeed, while reading one back as `i64` may legitimately
/// fail. A single `From` pair could not express that asymmetry.
///
/// # No conversion is lossy
///
/// Every implementation either produces the value the caller asked for or
/// returns an [`Error`]. In particular [`usize`] and [`isize`] report
/// [`Error::NegativeToUnsigned`] rather than wrapping a negative [`Value::Int`]
/// around to a huge length, and the type error names the type that was asked
/// for rather than the [`i64`] the implementation reads.
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

/// `usize` must not wrap a negative [`Value::Int`].
///
/// The obvious one-liner — read the [`i64`] and `as usize` — turns `-1` into
/// `usize::MAX`, which is the most dangerous possible result for the values
/// this type is read from: a length, an offset, an index. There is no
/// defensible reading of a negative length.
impl FromValue for usize {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        let raw = match value {
            Value::Int(i) => *i,
            other => {
                return Err(Error::TypeMismatch {
                    context,
                    expected: "usize",
                    found: other.type_name(),
                });
            }
        };
        usize::try_from(raw).map_err(|_| Error::NegativeToUnsigned {
            context,
            found: raw,
        })
    }
}

/// `isize` keeps the sign, but still refuses to truncate.
///
/// [`i64::try_from`] is not involved: on any target this crate supports
/// `isize` is at least 64 bits wide, so the conversion is a lossless
/// reinterpretation. The `try_from` is there so a hypothetical narrower
/// target reports [`Error::OutOfRange`] instead of silently dropping the top
/// bits.
impl FromValue for isize {
    fn from_value(value: &Value, context: &'static str) -> Result<Self> {
        let raw = match value {
            Value::Int(i) => *i,
            other => {
                return Err(Error::TypeMismatch {
                    context,
                    expected: "isize",
                    found: other.type_name(),
                });
            }
        };
        isize::try_from(raw).map_err(|_| Error::OutOfRange {
            requested: raw.unsigned_abs(),
            max: isize::MAX as u64,
        })
    }
}

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

/// A `u64` becomes a [`Value::Int`] only if it fits.
///
/// There is deliberately no `From<u64> for Value`. The infallible version
/// existed and cast with `as i64`, so `u64::MAX` became `-1` with no error —
/// and the crate's flagship use case is parsing config, where a large
/// identifier becomes a wrong value rather than a failure. Absence of the
/// `From` impl is enforced by the compiler, not by convention: `core` blanket-
/// implements `TryFrom<U> for T` wherever `U: Into<T>`, so a `From<u64>` and
/// this `TryFrom<u64>` cannot both exist.
///
/// ```
/// use morphic::{Error, Value};
///
/// assert_eq!(Value::try_from(7u64).unwrap(), Value::int(7));
///
/// assert_eq!(
///     Value::try_from(u64::MAX),
///     Err(Error::OutOfRange { requested: u64::MAX, max: i64::MAX as u64 })
/// );
/// ```
impl TryFrom<u64> for Value {
    type Error = Error;

    fn try_from(v: u64) -> Result<Self> {
        match i64::try_from(v) {
            Ok(narrowed) => Ok(Value::Int(narrowed)),
            Err(_) => Err(Error::OutOfRange {
                requested: v,
                max: i64::MAX as u64,
            }),
        }
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
