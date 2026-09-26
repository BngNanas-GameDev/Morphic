//! Object-safe callables for traits that Rust cannot make `dyn`-compatible.
//!
//! # The gap
//!
//! Per the Rust Reference, a dyn-compatible trait may not have generic
//! methods, may not return `impl Trait`, and may not be an `async fn` (an
//! `async fn` in a trait desugars to a return-position `impl Future`). So this
//! does not compile:
//!
//! ```compile_fail
//! # use std::future::Future;
//! trait Task {
//!     type Out;
//!     async fn run(&mut self) -> Self::Out;   // not dyn-compatible
//! }
//! fn run_all(tasks: &mut [&mut dyn Task<Out = u32>]) {}   // ERROR
//! ```
//!
//! The same Reference lists the available workarounds: `where Self: Sized`
//! (which removes the method from the vtable), `enum_dispatch` (a closed set
//! only), or a proc macro that generates a `Dyn`-suffixed newtype, which is
//! what [`dynosaur`](https://crates.io/crates/dynosaur) does. Upstream's real
//! answer, return type notation ([RFC 3654](https://rust-lang.github.io/rfcs/3654-return-type-notation.html)),
//! is still nightly-only.
//!
//! # The approach here
//!
//! Move the un-dyn-able part into a *type parameter* of the trait, where it
//! costs nothing at dispatch time:
//!
//! ```
//! use morphic::dyn_fn::{DynFnMut, boxed_async_fn_mut};
//!
//! # fn demo() {
//! let mut double = boxed_async_fn_mut(|x: i32| async move { x * 2 });
//! // `&mut self` survives a `Box<dyn ..>` boundary, so this is callable.
//! let _fut = double.invoke_mut(21);
//! # }
//! ```
//!
//! `DynFnMut<A, R>` has no generic *methods*, no `impl Trait` return and no
//! `async fn`, so `Box<dyn DynFnMut<A, R>>` is a legal trait object. Nothing
//! here needs a proc macro, and it works on stable.
//!
//! What it does not do is invent a vtable for *your* trait. For a trait of your
//! own you still need one `Dyn`-suffixed wrapper per signature, or a proc
//! macro. What this module removes is the need for an `async_trait` box on the
//! value layer, and the need to name a `Future` type at every call site.
//!
//! # The one thing it cannot do
//!
//! A trait object cannot have a by-value `self` method that you can actually
//! call. `Box<dyn Trait>` will not let you move the `dyn` out of the box, and
//! by-value dispatch needs exactly that. This is why `Box<dyn FnOnce()>` does
//! not exist in Rust - a language rule, not an oversight.
//!
//! So [`DynFnOnce`] is useful on a concrete type and useless behind a `Box`,
//! and there is deliberately no `boxed_fn_once`. For "call each handler at most
//! once, heterogeneously", keep the state in the closure and use `FnMut`,
//! which is what `async_trait` and
//! [dynosaur](https://crates.io/crates/dynosaur) do too.
//!
//! # One ergonomics wart
//!
//! `A` is the argument type, and `A = ()` means "called with one `()`", not
//! "called with no arguments". A zero-argument closure therefore has to be
//! written `|_: ()| ..`, and a type annotation alone will not make a bare
//! `|| ..` compile:
//!
//! ```compile_fail
//! # use morphic::dyn_fn::boxed_fn;
//! let f: Box<dyn morphic::dyn_fn::DynFn<(), u8> + Send + Sync> = boxed_fn(|| 1u8);
//! ```
//!
//! This is inherent to "one argument parameter, any type" and is why a real
//! language would have three traits rather than one generic over a tuple.

use std::future::Future;
use std::pin::Pin;

/// A heap-allocated, type-erased [`Future`], the only way a future can cross
/// a `dyn` boundary.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A callable that consumes `self`, like [`FnOnce`].
///
/// Not invocable behind a `Box<dyn ..>`; see the module docs.
pub trait DynFnOnce<A, R> {
    /// Invoke the callable. Corresponds to [`FnOnce::call_once`].
    fn invoke_once(self, arg: A) -> R;
}

/// A callable that mutates itself, like [`FnMut`].
///
/// Usable behind a `Box<dyn ..>`, which makes this the practical choice for
/// per-handler state.
pub trait DynFnMut<A, R> {
    /// Invoke the callable. Corresponds to [`FnMut::call_mut`].
    fn invoke_mut(&mut self, arg: A) -> R;
}

/// A callable that borrows itself, like [`Fn`].
pub trait DynFn<A, R> {
    /// Invoke the callable. Corresponds to [`Fn::call`].
    fn invoke(&self, arg: A) -> R;
}

impl<F, A, R> DynFnOnce<A, R> for F
where
    F: FnOnce(A) -> R,
{
    fn invoke_once(self, arg: A) -> R {
        self(arg)
    }
}

impl<F, A, R> DynFnMut<A, R> for F
where
    F: FnMut(A) -> R,
{
    fn invoke_mut(&mut self, arg: A) -> R {
        self(arg)
    }
}

impl<F, A, R> DynFn<A, R> for F
where
    F: Fn(A) -> R,
{
    fn invoke(&self, arg: A) -> R {
        self(arg)
    }
}

/// Erase a `Fn` into a `Send + Sync` trait object.
pub fn boxed_fn<A, R, F>(f: F) -> Box<dyn DynFn<A, R> + Send + Sync>
where
    F: Fn(A) -> R + Send + Sync + 'static,
    A: Send + 'static,
    R: Send + 'static,
{
    Box::new(f)
}

/// Erase a `FnMut` into a `Send` trait object.
pub fn boxed_fn_mut<A, R, F>(f: F) -> Box<dyn DynFnMut<A, R> + Send>
where
    F: FnMut(A) -> R + Send + 'static,
    A: Send + 'static,
    R: Send + 'static,
{
    Box::new(f)
}

/// Erase an `async fn`-shaped closure returning a [`BoxFuture`].
pub fn boxed_async_fn<A, T, F, Fut>(f: F) -> Box<dyn DynFn<A, BoxFuture<'static, T>> + Send + Sync>
where
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = T> + Send + 'static,
    A: Send + 'static,
    T: Send + 'static,
{
    Box::new(move |a| Box::pin(f(a)) as BoxFuture<'static, T>)
}

/// Erase an `async fn`-shaped mutating closure returning a [`BoxFuture`].
pub fn boxed_async_fn_mut<A, T, F, Fut>(
    mut f: F,
) -> Box<dyn DynFnMut<A, BoxFuture<'static, T>> + Send>
where
    F: FnMut(A) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    A: Send + 'static,
    T: Send + 'static,
{
    Box::new(move |a| {
        let fut = f(a);
        Box::pin(fut) as BoxFuture<'static, T>
    })
}

/// Build a [`BoxFuture`] from an `async` block, erasing the anonymous future
/// type it would otherwise give the compiler no way to name.
pub fn boxed_async<T, Fut>(fut: Fut) -> BoxFuture<'static, T>
where
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    Box::pin(fut)
}
