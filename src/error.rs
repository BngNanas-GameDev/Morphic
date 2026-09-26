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
    /// A native function was called with the wrong number of arguments.
    Arity {
        /// Name of the native function.
        name: String,
        /// Argument count the function accepts.
        expected: usize,
        /// Argument count that was supplied.
        found: usize,
    },
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
    /// A map or object did not contain the requested key.
    KeyNotFound,
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
            Error::Arity {
                name,
                expected,
                found,
            } => write!(f, "`{name}` expected {expected} argument(s), got {found}"),
            Error::TypeMismatch {
                context,
                expected,
                found,
            } => write!(f, "{context} expected {expected}, found {found}"),
            Error::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Error::KeyNotFound => f.write_str("key not found"),
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
