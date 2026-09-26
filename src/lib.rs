//! Dynamism for Rust.
//!
//! `morphic` gives a Rust program the three things it structurally lacks: a
//! dynamically typed [`Value`], a garbage-collected object graph, and
//! object-safe callables for trait signatures Rust cannot put behind a `dyn`.
//!
//! ```
//! use morphic::{CallCtx, FromValue, NativeFn, Value};
//!
//! // Any shape, chosen at runtime, collected when unreachable.
//! let config = Value::map_from([
//!     ("retries", Value::int(3)),
//!     ("label", Value::str("primary")),
//! ]);
//! assert_eq!(config.map_get(&Value::str("retries")).unwrap().as_int(), Some(3));
//!
//! // Cycles are ordinary, not a leak.
//! let list = Value::list();
//! list.list_push(Value::int(1)).unwrap();
//! list.list_push(list.clone()).unwrap();
//! assert!(list.list_get(1).unwrap().ptr_eq(&list));
//!
//! // Behaviour crosses a `dyn` boundary with no proc macro.
//! let add = Value::native(NativeFn::new("add", |_ctx, args| {
//!     let a = i64::from_value(args.first().unwrap_or(&Value::Nil), "add")?;
//!     let b = i64::from_value(args.get(1).unwrap_or(&Value::Nil), "add")?;
//!     Ok(Value::int(a + b))
//! }));
//! let ctx = CallCtx::new();
//! assert_eq!(add.call_with(&ctx, &[Value::int(2), Value::int(3)]).unwrap().as_int(), Some(5));
//!
//! // A prototype cycle is rejected rather than followed forever.
//! let a = Value::object();
//! let b = Value::object();
//! a.obj_set_proto(&b).unwrap();
//! b.obj_set_proto(&a).unwrap();
//! assert!(a.obj_get("anything").is_err());
//! ```
//!
//! # Why this exists
//!
//! Rust has a garbage collector problem, but not the one people usually mean.
//! Two design decisions were made long ago and neither is reversible:
//!
//! * **Nothing is in the language.** Manishearth's survey of safe GC designs
//!   opens with "Why are GCs in Rust hard? In one word: Rooting." Rust has no
//!   concept of "directly on the stack", so nothing distinguishes a root from a
//!   heap-held pointer. Every design below is a different answer to that one
//!   question.
//! * **The project has never accepted a GC RFC that adds one.** RFC 256 removed
//!   the pre-1.0 `@T` refcounting collector in 2014, noting that "the majority
//!   of the Rust core team still believe that there are use cases that would be
//!   well handled by a proper tracing garbage collector". As of 2026 there is
//!   still no proposal to add one.
//!
//! So the collector here is borrowed, not invented. `morphic` builds on
//! [`boa_gc`], the mark-sweep collector behind the Boa JavaScript engine, which
//! is the most widely deployed GC written in Rust. Note that 0.22 is a
//! **refcount hybrid**, not a pure tracer — see [`NativeFn`] for why that
//! matters here. What is new is the layer above it.
//!
//! # What is new here
//!
//! * **A closed-enum [`Value`] with a total equality.** No `dyn` is needed for
//!   data, so every match stays exhaustive. Floats compare by bit pattern, so
//!   `Value` can key a map without `Eq`/`Hash`, which is the friction that has
//!   kept the ecosystem from converging on a dynamic value type
//!   ([`serde_json::Value`] owns, [`valuable`] borrows, and nothing has won).
//! * **An explicit capture list for native functions.** See [`NativeFn`] for
//!   why this is a soundness requirement and not a convenience.
//! * **Object-safe callables for `async fn`.** See [`dyn_fn`] for the
//!   dyn-compatibility rules this works around, and for what it deliberately
//!   does not solve.
//!
//! # What this is not
//!
//! * **Not concurrent.** [`Gc`] is `!Send + !Sync`; the heap is thread-local.
//! * **Not moving.** There is no compaction, because a moving collector needs
//!   interior pointers in the `noalias`-controlled stack.
//! * **Not `no_std`.** It needs an allocator and a thread.
//! * **Not a language runtime.** There is no parser, no evaluator and no
//!   object system beyond properties and prototypes. If you want those, embed
//!   `rquickjs`, `mlua` or `deno_core` — they have battle-tested collectors,
//!   and using one is almost always the right call.
//!
//! # Comparison
//!
//! | Crate | Collector | Notes |
//! |---|---|---|
//! | `gc` | mark-sweep, every handle is a root | Ergonomic, but pays refcount traffic on every write and is `!Send`. Stalled since 2021. |
//! | `gc-arena` | incremental, non-rooted handles | Provably safe and zero-cost, but every access goes through `arena.mutate(..)` and `Collect` cannot be derived for `async` state machines. |
//! | `boa_gc` | mark-sweep, weak refs and ephemerons | What this crate uses. 5M+ downloads, powering Boa. |
//!
//! [`valuable`]: https://crates.io/crates/valuable
//! [`serde_json::Value`]: https://crates.io/crates/serde_json

#![doc(html_root_url = "https://docs.rs/morphic/0.1.0")]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod convert;
pub mod dyn_fn;
pub mod error;
pub mod gc;
pub mod registry;
pub mod value;

pub use boa_gc::{Ephemeron, Finalize, Gc, GcErased, GcRef, GcRefCell, GcRefMut, Trace, WeakGc};
pub use convert::{FromValue, IntoValue, ValueUser};
pub use error::{Error, Result};
pub use registry::{Registry, RegistryError};
pub use value::{
    CALL_PROP, CallCtx, MAX_PRINT_DEPTH, MAX_PRINT_ENTRIES, MAX_PROTO_DEPTH, NativeFn,
    NativeFnBody, Object, Value, ValueMap,
};
