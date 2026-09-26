//! Error type for [`morphic`](crate).

use core::fmt;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Everything that can go wrong when working with [`Value`](crate::Value)s
/// and dynamic calls.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// A value that is not callable was called.
    NotCallable,
    /// A value had the wrong shape for the requested operation.
    TypeMismatch {
        /// The operation that failed, e.g. `"Value::as_int"`.
        context: &'static str,
        /// The shape the operation required.
        expected: &'static str,
        /// The shape the value actually had.
        found: &'static str,
    },
    /// A list index was out of bounds.
    IndexOutOfBounds {
        /// The requested index.
        index: usize,
        /// The length of the list.
        len: usize,
    },
    /// A number could not be represented without changing its value.
    ///
    /// Always a narrowing failure, so `requested` is a `u64` and `max` is the
    /// largest representable value *for the direction that failed*. Converting
    /// a `u64` above [`i64::MAX`] into a [`Value::Int`](crate::Value::Int) is
    /// the motivating case: a `From<u64> for Value` that cast instead would
    /// turn `u64::MAX` into `-1` and report no error at all.
    OutOfRange {
        /// The value that was offered.
        requested: u64,
        /// The largest value that can be represented.
        max: u64,
    },
    /// A negative integer was read into an unsigned type.
    ///
    /// Distinct from [`Error::OutOfRange`] because the value is the wrong sign
    /// rather than too large, and a caller usually wants to treat the two
    /// differently: a negative length is malformed input, an over-wide one is
    /// merely unrepresentable.
    NegativeToUnsigned {
        /// The operation that failed, e.g. `"parse_len"`.
        context: &'static str,
        /// The negative value that was found.
        found: i64,
    },
    /// A map or object did not contain the requested key.
    ///
    /// Only ever returned when the walk actually completed. A prototype that
    /// could not be read reports [`Error::BorrowConflict`] instead.
    KeyNotFound,
    /// A prototype chain exceeded [`MAX_PROTO_DEPTH`](crate::MAX_PROTO_DEPTH),
    /// which means it loops.
    ProtoCycle {
        /// The configured limit.
        limit: usize,
    },
    /// A `GcRefCell` was already mutably borrowed, or already borrowed when a
    /// mutable borrow was attempted.
    BorrowConflict(&'static str),
    /// The call depth limit in the [`CallCtx`](crate::CallCtx) was reached.
    ///
    /// This is what stops a recursively calling graph of native functions from
    /// overflowing the native stack.
    RecursionLimit {
        /// The configured limit.
        limit: u32,
    },
    /// A key was already present, and the operation refused to overwrite it.
    DuplicateKey,
    /// A user type reported a failure of its own.
    User(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotCallable => f.write_str("value is not callable"),
            Error::TypeMismatch {
                context,
                expected,
                found,
            } => write!(f, "{context} expected {expected}, found {found}"),
            Error::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Error::OutOfRange { requested, max } => {
                write!(f, "{requested} is out of range, and the maximum is {max}")
            }
            Error::NegativeToUnsigned { context, found } => {
                write!(f, "{context} expected a non-negative int, found {found}")
            }
            Error::KeyNotFound => f.write_str("key not found"),
            Error::ProtoCycle { limit } => {
                write!(f, "prototype chain exceeded {limit} links, so it must loop")
            }
            Error::BorrowConflict(what) => write!(f, "borrow conflict while accessing {what}"),
            Error::RecursionLimit { limit } => {
                write!(f, "call recursion limit of {limit} reached")
            }
            Error::DuplicateKey => f.write_str("key already present"),
            Error::User(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}
