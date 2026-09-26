//! Garbage-collection controls.
//!
//! The heap is a [`boa_gc`](https://crates.io/crates/boa_gc) mark-sweep
//! collector. It is **thread-local**: [`Gc`](crate::Gc) is `!Send + !Sync`, and
//! a value allocated on one thread cannot be moved to another. Every safe Rust
//! GC in existence has the same constraint, and RustPython is the cautionary
//! tale — making even a refcount-and-cycle collector correct across threads
//! required a stop-the-world barrier at bytecode safepoints.
//!
//! There is no concurrency here, deliberately. `cgc` is the only moving,
//! concurrent collector ever written in Rust; its documentation states that
//! write barriers "must be inserted before any store operation into heap value
//! otherwise this may lead to UB or segfault", and it has been unmaintained
//! since 2020.

use boa_gc::force_collect as boa_force_collect;

/// Run a collection immediately.
///
/// The collector also runs on its own allocation heuristic, so calling this is
/// only necessary when you need reclamation at a specific point — after a
/// known batch of churn, or before measuring memory.
pub fn collect() {
    boa_force_collect();
}

/// Whether it is currently safe to run a [`Finalize`](crate::Finalize)
/// implementation.
///
/// Returns `false` while a finalizer that is itself being collected is on the
/// stack. Do not allocate in that state.
pub fn finalizer_safe() -> bool {
    boa_gc::finalizer_safe()
}
