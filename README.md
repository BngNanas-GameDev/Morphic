# morphic

Dynamism for Rust: a garbage-collected dynamic value type, object-safe callables for traits Rust cannot put behind a `dyn`, and a runtime type registry.

```toml
[dependencies]
morphic = "0.1"
```

```rust
use morphic::{FromValue, NativeFn, Value};

// Any shape, chosen at runtime, collected when unreachable.
let config = Value::map_from([
    ("retries", Value::int(3)),
    ("label", Value::str("primary")),
]);
assert_eq!(config.map_get(&Value::str("retries")).unwrap().as_int(), Some(3));

// Cycles are ordinary, not a leak.
let list = Value::list();
list.list_push(Value::int(1)).unwrap();
list.list_push(list.clone()).unwrap();
assert!(list.list_get(1).unwrap().ptr_eq(&list));

// Behaviour crosses a `dyn` boundary with no proc macro.
let add = Value::native(NativeFn::new("add", |_ctx, args| {
    let a = i64::from_value(args.first().unwrap_or(&Value::Nil), "add")?;
    let b = i64::from_value(args.get(1).unwrap_or(&Value::Nil), "add")?;
    Ok(Value::int(a + b))
}));
assert_eq!(add.call(&[Value::int(2), Value::int(3)]).unwrap().as_int(), Some(5));
```

Run `cargo run --example interpreter` for a plugin-style interpreter that uses all three modules together.

## Why this exists

Rust has a garbage collector problem, but not the one people usually mean. Two decisions were made long ago and neither is reversible:

**Nothing is in the language.** Manishearth's survey of safe GC designs in Rust opens with *"Why are GCs in Rust hard? In one word: Rooting."* Rust has no concept of "directly on the stack", so nothing distinguishes a root from a heap-held pointer. Every design in the space is a different answer to that one question.

**The project has never accepted a GC RFC.** [RFC 256](https://github.com/rust-lang/rfcs/blob/master/text/0256-remove-refcounting-gc-of-t.md) removed the pre-1.0 `@T` refcounting collector in 2014, noting that "the majority of the Rust core team still believe that there are use cases that would be well handled by a proper tracing garbage collector". Twelve years later there is still no proposal. The `rust-lang/rfcs` tree contains no GC or dynamism RFC at all.

So the collector here is **borrowed, not invented**. `morphic` builds on [`boa_gc`](https://crates.io/crates/boa_gc), the mark-sweep collector behind the Boa JavaScript engine and the most widely deployed GC written in Rust. What is new here is the layer above it.

## What is new here

### A closed-enum `Value` with a total equality

No `dyn` is needed for data, so every match stays exhaustive and needs no vtable.

The interesting part is equality. `Value` can key a map, which requires a *total* equality, and `f64` normally rules that out because `NaN != NaN`. So floats compare **by bit pattern**, which preserves reflexivity, and `Int`/`Float` pairs compare numerically, so `1` and `1.0` are the same key. Mixed numeric transitivity keeps the usual float wart (`Int(2^53 + 1) == Float(2^53)` but `!= Int(2^53)`), which is why `Value` is not `Eq`.

`List`, `Map`, `Object`, `Native` and `User` compare by *identity*. Structural equality on a cyclic graph is not decidable, so structural comparison is deliberately not offered — use `ptr_eq` when you mean "the same cell".

This is the friction that has kept the ecosystem from converging on a dynamic value type. `serde_json::Value` has 1.3B downloads but is serialization-shaped and *owns*; `valuable` has 292M and is object-safe and visitor-based but *borrows*; `anymap` is still `1.0.0-beta.2` after twelve years; `serde-value` has been stuck at 0.7.0 since 2020. Nobody has won.

### An explicit capture list for native functions

A `Value::User` payload is a bare `GcErased`, and a closure body is an opaque `Rc<dyn Fn ..>`. Neither can be introspected to discover which `Gc` cells it holds. A closure that captures a `Gc` and does not register it will have that cell collected while still reachable — a use-after-free, not a leak.

`NativeFn` therefore traces *only* its declared capture list, and the constructor API forces you to populate it:

```rust
use morphic::{Gc, NativeFn, Value};

let cell = Gc::new(vec![10i64, 20, 30]);
let sum = Value::native(
    NativeFn::new("sum", {
        let cell = cell.clone();
        move |_ctx, _args| Ok(Value::int(cell.iter().sum::<i64>()))
    })
    .capture(cell.clone()),
);

drop(cell);
morphic::gc::collect();
assert_eq!(sum.call(&[]).unwrap().as_int(), Some(60));
```

`NativeFn::capture_value` is the shorthand for the common case, and `NativeFn::from_user` / `from_user_mut` capture and register in one step, so the data and the rooting can never drift apart. This is, as far as I know, a novel answer in safe Rust: the equivalent of a write barrier, for opaque callbacks.

### Object-safe callables for `async fn`

Per the [Rust Reference](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility), a dyn-compatible trait may not have generic methods, may not return `impl Trait`, and may not be an `async fn`. So this does not compile:

```compile_fail
trait Task {
    type Out;
    async fn run(&mut self) -> Self::Out;
}
fn run_all(tasks: &mut [&mut dyn Task<Out = u32>]) {}
```

`morphic::dyn_fn` moves the un-dyn-able part into a type parameter, where it costs nothing at dispatch time:

```rust
use morphic::dyn_fn::{DynFn, DynFnMut, BoxFuture, boxed_async_fn, boxed_async_fn_mut};

let double: Box<dyn DynFn<i32, BoxFuture<'static, i32>> + Send + Sync> =
    boxed_async_fn(|x: i32| async move { x * 2 });
// await double.invoke(21) == 42

let mut add: Box<dyn DynFnMut<i64, BoxFuture<'static, i64>> + Send> =
    boxed_async_fn_mut(/* per-handler state */);
```

Stable, no proc macro, no `async_trait` box.

**What it does not do** is invent a vtable for *your* trait. For a trait of your own you still need one `Dyn`-suffixed wrapper per signature, or a proc macro. Upstream's real answer, [return type notation (RFC 3654)](https://rust-lang.github.io/rfcs/3654-return-type-notation.html), is still nightly-only, blocked on TAIT and the next-generation trait solver.

### A `Registry` that reports why

`TypeId -> Box<dyn Any + Send + Sync>`, which is the trick `anymap` plays. It is here so users do not need a second crate, and so lookups return a typed error instead of an opaque `None`. `try_insert` refuses to replace; `anymap` never escaped beta.

This is the complement to [`inventory`](https://crates.io/crates/inventory) rather than a replacement: `inventory` and `linkme` register at link time, which is right when the type set is closed at compile time. Use a `Registry` when the set really is open at runtime.

## What this is not

- **Not concurrent.** `Gc` is `!Send + !Sync`; the heap is thread-local. Every safe Rust GC in the ecosystem is single-threaded. RustPython needed a full stop-the-world barrier at bytecode safepoints just to make a *refcount* cycle detector correct.
- **Not moving.** No compaction, because a moving collector needs interior pointers in the `noalias`-controlled stack. The only prior art is [`cgc`](https://github.com/playXE/cgc), whose docs state that write barriers "must be inserted before any store operation into heap value otherwise this may lead to UB or segfault" and that `Heap::get` is a case where "It's UB to access gc'ed value". Unmaintained since 2020, 16k downloads. A safe moving collector is not buildable.
- **Not `no_std`.** Needs an allocator and a thread.
- **Not a language runtime.** No parser, no evaluator, no object system beyond properties and prototypes. If you want those, embed `rquickjs`, `mlua` or `deno_core` — they have battle-tested collectors, and using one is almost always the right call.

## Comparison

| Crate | Collector | Rooting | Notes |
|---|---|---|---|
| [`gc`](https://crates.io/crates/gc) | mark-sweep, refcount hybrid | every `Gc` is a root | Most ergonomic, but pays refcount traffic on every write and is `!Send`. Last release 2025, 22 open issues. |
| [`gc-arena`](https://crates.io/crates/gc-arena) | incremental, cycle-collecting | non-rooted handles | Provably safe, zero-cost handles, used by Ruffle. But every access goes through `arena.mutate(..)`, and `Collect` cannot be derived for `async` state machines. |
| [`boa_gc`](https://crates.io/crates/boa_gc) | mark-sweep + weak refs + ephemerons | rooted handles | What this crate uses. 5M+ downloads. |
| [`cgc`](https://github.com/playXE/cgc) | concurrent, moving | explicit barriers | The only moving collector in Rust, and the only one that is `unsafe` throughout. Dead. |

`gc-arena`'s design is the one worth studying: it is documented to an unusually high standard, and its central constraint — *mutation XOR collection* — is the ergonomic tax that the other two designs pay in a different currency (refcount traffic, and rooting ceremony respectively).

## Two design notes worth knowing

**Every derived type gets an empty `Drop` impl.** `#[derive(Trace)]` in `boa_gc` generates `impl Drop` that does nothing, in order to make `unsafe impl Drop` a compile error. This is not an accident: a destructor on a collected type can stash itself into a long-lived reference during collection and produce a dangling reference, which is the single hardest problem in the whole space. The cost is that you cannot partially move out of a `Value` or an `Object` — clone instead.

**`#[derive(Trace)]` does not imply `Finalize`.** `Trace` has `Finalize` as a supertrait, and the derive only generates the former, so user payloads need both:

```rust
use morphic::{Finalize, Trace, Value};

#[derive(Trace, Finalize)]
struct Point { x: f64, y: f64 }

let v = Value::user(Point { x: 1.0, y: 2.0 });
assert_eq!(v.as_user::<Point>().unwrap().x, 1.0);
```

## Status

Early. 80 tests, clippy clean, `missing_docs` enforced, MSRV 1.91 (inherited from `boa_gc` 0.22).

Not yet done, in rough priority order: benchmarks; `no_std` support behind a feature flag; weak references exposed at the `Value` level; a `serde` bridge; and a decision about whether to attempt a concurrent collector at all (the honest answer from the `cgc` evidence is probably no).

## Licence

MIT OR Apache-2.0.

## Further reading

- [A Tour of Safe Tracing GC Designs in Rust](https://manishearth.github.io/blog/2021/04/05/a-tour-of-safe-tracing-gc-designs-in-rust/) — the single best survey in existence. Read this first.
- [Techniques for Safe Garbage Collection in Rust](https://kyju.org/blog/rust-safe-garbage-collection/) — the clearest statement of what safety costs: lifetime branding, invariance, `MustNotImplDrop`, mutation-XOR-collection.
- [Designing a GC in Rust](https://manishearth.github.io/blog/2015/09/01/designing-a-gc-in-rust/) — the original `rust-gc` design, including why lints and stack scanning were rejected.
- [Dyn you have idea for `dyn`?](https://smallcultfollowing.com/babysteps/blog/2025/03/25/dyn-you-have-idea-for-dyn/) — Niko Matsakis on why `dyn Trait` is dissatisfying.
- [Garbage Collection for Rust: The Finalizer Frontier](https://arxiv.org/abs/2504.01841) — Hughes & Tratt, the current academic frontier (DOI 10.1145/3763179).
