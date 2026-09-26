# morphic

Dynamism for Rust: a garbage-collected dynamic value type, object-safe callables for traits Rust cannot put behind a `dyn`, and a runtime type registry.

```toml
[dependencies]
morphic = "0.1"
```

```rust
use morphic::{CallCtx, FromValue, NativeFn, Value};

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
let ctx = CallCtx::new();
assert_eq!(add.call_with(&ctx, &[Value::int(2), Value::int(3)]).unwrap().as_int(), Some(5));
```

There is no `Value::call(&self, args)`. It existed in 0.1.0 and was removed: it built a
fresh `CallCtx` per invocation, so a body re-entering through it reset the depth counter and
overflowed the native stack instead of returning `Error::RecursionLimit`.

Run `cargo run --example interpreter` for a plugin-style interpreter that uses all three modules together.

## Why this exists

Rust has a garbage collector problem, but not the one people usually mean. Two decisions were made long ago and neither is reversible:

**Nothing is in the language.** Manishearth's survey of safe GC designs in Rust opens with *"Why are GCs in Rust hard? In one word: Rooting."* Rust has no concept of "directly on the stack", so nothing distinguishes a root from a heap-held pointer. Every design in the space is a different answer to that one question.

**The project has never accepted a GC RFC.** [RFC 256](https://github.com/rust-lang/rfcs/blob/master/text/0256-remove-refcounting-gc-of-t.md) removed the pre-1.0 `@T` refcounting collector in 2014, noting that "the majority of the Rust core team still believe that there are use cases that would be well handled by a proper tracing garbage collector". Twelve years later there is still no proposal. The `rust-lang/rfcs` tree contains no GC or dynamism RFC at all.

So the collector here is **borrowed, not invented**. `morphic` builds on [`boa_gc`](https://crates.io/crates/boa_gc), the mark-sweep collector behind the Boa JavaScript engine and the most widely deployed GC written in Rust. What is new here is the layer above it.

## What is new here

### A closed-enum `Value` with a real `Eq`

No `dyn` is needed for data, so every match stays exhaustive and needs no vtable.

The interesting part is equality. `Value` keys a map, which requires a *reflexive* equality, and `f64` normally rules that out because `NaN != NaN`. So floats compare **by bit pattern**, which preserves reflexivity and makes `Value` a real `Eq`.

`Int` and `Float` are deliberately **not** cross-compared. An earlier version compared them numerically, which is not transitive: `Int(2^53+1) == Float(2^53)` and `Int(2^53) == Float(2^53)` but the two `Int`s differ, so `ValueMap` collapsed three inserts into two and destroyed a value. Use `Value::as_float`, which widens an `Int` on read without making them equal.

`List`, `Map`, `Object`, `Native` and `User` compare by *identity*. Structural equality on a cyclic graph is not decidable, so structural comparison is deliberately not offered — use `ptr_eq` when you mean "the same cell".

This is the friction that has kept the ecosystem from converging on a dynamic value type. `serde_json::Value` has 1.3B downloads but is serialization-shaped and *owns*; `valuable` has 292M and is object-safe and visitor-based but *borrows*; `anymap` is still `1.0.0-beta.2` after twelve years; `serde-value` has been stuck at 0.7.0 since 2020. Nobody has won.

### A body may hold `Gc` cells, and that leaks

A `Value::User` payload is a bare `GcErased`, and a closure body is an opaque `Rc<dyn Fn ..>`. Neither can be introspected to discover which cells it holds.

The consequence is not the one you might expect. `boa_gc` 0.22 is **not** a pure tracer: `GcHeader::is_rooted` is `non_root_count < ref_count`, and `Trace::trace_non_roots` is what raises `non_root_count`. A `Gc` handle living in ordinary Rust memory — a closure environment, a stack slot — is therefore never marked non-root, so it is a **permanent root**. A cell captured by a body cannot be freed while the body is alive.

That makes it sound, and it also makes it leak: when the `NativeFn` is swept its `Rc<dyn Fn ..>` is dropped under a guard, and `Gc::drop` only runs `Finalize` when `finalizer_safe()`. The refcount is never decremented, so the captured cell stays rooted forever.

In short: **a `Gc` captured by a body is safe but never reclaimed.** This is a property of `boa_gc`'s refcount model, not of the `Trace` contract, and it may change in a future `boa_gc`.

```rust
use morphic::{CallCtx, Gc, NativeFn, Value};

let cell = Gc::new(vec![10i64, 20, 30]);
let sum = Value::native(NativeFn::new("sum", {
    let cell = cell.clone();
    move |_ctx, _args| Ok(Value::int(cell.iter().sum::<i64>()))
}));

drop(cell);
morphic::gc::collect();

// Sound: the cell is still usable even though every strong handle is gone.
assert_eq!(sum.call_with(&CallCtx::new(), &[]).unwrap().as_int(), Some(60));
```

`NativeFn::from_user` and `from_user_mut` move the data cell into the body in one step, so it cannot be forgotten. If you need captured cells actually collected, use [`gc-arena`](https://crates.io/crates/gc-arena), whose mutation-XOR-collection design has no rooting problem at all.

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

`TypeId -> Box<dyn Any + Send + Sync>`, which is the trick `anymap` plays. It is here so users do not need a second crate. `try_insert` refuses to replace and `try_get` names the missing `TypeId`, where `anymap` — still `1.0.0-beta.2` — returns a bare `Option` from everything.

To be precise: **two of its eleven methods report.** `insert`, `get`, `get_mut`, `remove` and `contains` all return `Option`, because "absent" is the overwhelmingly common answer and a `Result` there would be noise.

This is the complement to [`inventory`](https://crates.io/crates/inventory) rather than a replacement: `inventory` and `linkme` register at link time, which is right when the type set is closed at compile time. Use a `Registry` when the set really is open at runtime.

## Bounds you should know about

Three limits exist because the alternative was a hang or a wrong answer, not because of caution.

- **Prototype chains are bounded at 64 links** (`MAX_PROTO_DEPTH`), reported as `Error::ProtoCycle`. Prototype links are writable, so `a.obj_set_proto(&b); b.obj_set_proto(&a)` is legal from safe code and every reader would otherwise spin forever at constant memory — which no allocator-based watchdog can catch. A 64-deep *acyclic* chain still resolves.
- **Recursion is bounded at 256 per thread**, reported as `Error::RecursionLimit`. The counter is a thread-local, not a `CallCtx` field, on purpose: a body that constructs a fresh `CallCtx` and recurses through that must not get a fresh budget. The trade-off is that two `CallCtx`s on one thread share one budget.
- **Printing is bounded in depth** at 64, not in width. A 2000-property object renders 2000 entries.

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
| [`boa_gc`](https://crates.io/crates/boa_gc) | mark-sweep + weak refs + ephemerons | **refcount hybrid** | What this crate uses. 5M+ downloads. `GcHeader::is_rooted` is `non_root_count < ref_count`, so a `Gc` outside the heap is a permanent root. That is what makes an untraced body sound — and what makes it leak. |
| [`cgc`](https://github.com/playXE/cgc) | concurrent, moving | explicit barriers | The only moving collector in Rust, and the only one that is `unsafe` throughout. Dead. |

`gc-arena`'s design is the one worth studying: it is documented to an unusually high standard, and its central constraint — *mutation XOR collection* — is the ergonomic tax that the other two designs pay in a different currency (refcount traffic, and rooting ceremony respectively).

## Two design notes worth knowing

**Every derived type gets an empty `Drop` impl.** `#[derive(Trace)]` in `boa_gc` generates `impl Drop` that does nothing, in order to make `unsafe impl Drop` a compile error. This is not an accident: a destructor on a collected type can stash itself into a long-lived reference during collection and produce a dangling reference, which is the single hardest problem in the whole space. The cost is that you cannot partially move out of a `Value` or an `Object` — clone instead. The sharper consequence: **you cannot implement `Drop` for any `Trace` type at all.** The escape hatch is `#[boa_gc(unsafe_no_drop)]`.

**`#[derive(Trace)]` requires a direct `boa_gc` dependency.** The derive expands to `::boa_gc::…`, and `morphic` re-exports the derive macros but not the crate under that name. So a crate that defines its own traced payload needs:

```toml
[dependencies]
morphic = "0.1"
boa_gc = "0.22"   # the derives expand to ::boa_gc::Trace / ::boa_gc::Finalize
```

```rust
use morphic::{Finalize, Trace, Value};

#[derive(Trace, Finalize)]
struct Point {
    x: f64,
    y: f64,
}

let v = Value::user(Point { x: 1.0, y: 2.0 });
assert_eq!(v.as_user::<Point>().unwrap().x, 1.0);
```

## Status

Early, and specific about what that means. **87 tests** (52 value, 11 registry, 10 dyn_fn, 8 convert, 6 doctests), `cargo clippy --all-targets -- -D warnings` clean, `cargo doc` clean under `-D warnings`.

An adversarial audit found 14 bugs in 0.1.0, three of them critical. All critical and major ones are fixed, each with a regression test:

- A prototype cycle made **seven** public entry points loop forever at constant memory. Now `Error::ProtoCycle`.
- A prototype walk reported an unreadable ancestor as `KeyNotFound` — a key that existed simply "wasn't there". Now `Error::BorrowConflict`.
- The recursion limit was defeated by one method call, then by a fresh `CallCtx` inside a body. The counter is now a thread-local.
- Call depth leaked on panic, so 256 panics made every later call return a spurious `RecursionLimit`.
- `proto()` and `is_callable()` panicked on safe input; they were the only panics reachable from the public API.
- `Int`/`Float` equality was not transitive, so `ValueMap` silently destroyed a value. Cross-kind equality is gone and `Value` is a real `Eq`.

That audit also found that the crate's headline claim was false. 0.1.0 shipped an "explicit capture list" justified as preventing a use-after-free; it prevented nothing, because a `Gc` in ordinary Rust memory is already a permanent root, and the *actual* failure was a 100% permanent leak that the capture list could not fix. The list is deleted and the leak is documented instead.

**Not fixed** (known, not hidden): dead variants `Error::Arity` and `RegistryError::NotSendSync`; `From<u64> for Value` truncates; `usize` conversion reports `expected: "i64"`; `Object::keys()` is own-only while `get` walks the chain; `list_pop` on empty reports a hard-coded index; `ValueMap` insert is O(n) so building is O(n²); no `list_set`/`list_insert`/`list_remove`.

**Unverified:** MSRV is claimed as 1.91 (inherited from `boa_gc` 0.22) but only the CI job can confirm it. The statement that `rust-lang/rfcs` contains no GC or dynamism RFC was true when written and needs re-checking before each release.

## Licence

MIT OR Apache-2.0.

## Further reading

- [A Tour of Safe Tracing GC Designs in Rust](https://manishearth.github.io/blog/2021/04/05/a-tour-of-safe-tracing-gc-designs-in-rust/) — the single best survey in existence. Read this first.
- [Techniques for Safe Garbage Collection in Rust](https://kyju.org/blog/rust-safe-garbage-collection/) — the clearest statement of what safety costs: lifetime branding, invariance, `MustNotImplDrop`, mutation-XOR-collection.
- [Designing a GC in Rust](https://manishearth.github.io/blog/2015/09/01/designing-a-gc-in-rust/) — the original `rust-gc` design, including why lints and stack scanning were rejected.
- [Dyn you have idea for `dyn`?](https://smallcultfollowing.com/babysteps/blog/2025/03/25/dyn-you-have-idea-for-dyn/) — Niko Matsakis on why `dyn Trait` is dissatisfying.
- [Garbage Collection for Rust: The Finalizer Frontier](https://arxiv.org/abs/2504.01841) — Hughes & Tratt, the current academic frontier (DOI 10.1145/3763179).
