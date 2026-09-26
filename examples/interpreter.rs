//! A tiny plugin-style interpreter built on `morphic`.
//!
//! Run with `cargo run --example interpreter`.
//!
//! It exercises the three pieces together: a [`Registry`] of handlers chosen by
//! name at runtime, [`Value`] as the argument and result currency, and
//! prototype-chain defaults.

use morphic::{Finalize, Gc, GcRefCell, NativeFn, Registry, Trace, Value};

struct Inc;
struct Dec;
struct Dbl;
struct Triple;

/// A handler that owns traced state, to show the user-payload path.
#[derive(Trace, Finalize)]
struct Offset(i64);

struct Plugins {
    handlers: Registry,
}

impl Plugins {
    fn new() -> Self {
        let mut handlers = Registry::new();
        handlers.insert(Inc);
        handlers.insert(Dec);
        handlers.insert(Dbl);
        handlers.insert(Triple);
        handlers.insert(Offset(100));
        Self { handlers }
    }

    /// Remove a handler. A disabled instruction is genuinely absent, which is
    /// what `is_enabled` reports.
    fn disable<H: Send + Sync + 'static>(&mut self) {
        self.handlers.remove::<H>();
    }

    fn is_enabled<H: Send + Sync + 'static>(&self) -> bool {
        self.handlers.contains::<H>()
    }

    fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .handlers
            .type_ids()
            .filter_map(|t| {
                if t == std::any::TypeId::of::<Inc>() {
                    Some("inc")
                } else if t == std::any::TypeId::of::<Dec>() {
                    Some("dec")
                } else if t == std::any::TypeId::of::<Dbl>() {
                    Some("dbl")
                } else if t == std::any::TypeId::of::<Triple>() {
                    Some("triple")
                } else if t == std::any::TypeId::of::<Offset>() {
                    Some("offset")
                } else {
                    None
                }
            })
            .collect();
        names.sort_unstable();
        names
    }
}

/// Run one instruction. The shape of `args` is checked at the boundary, which
/// is the whole point of a dynamic layer: nothing forces the caller and the
/// handler to agree at compile time.
fn run(plugins: &Plugins, name: &str, args: &Value) -> Result<i64, String> {
    let nth = |i: usize| -> Result<i64, String> {
        args.with_list_ref(|l| {
            l.get(i)
                .and_then(Value::as_int)
                .ok_or_else(|| format!("argument {i} is not an int"))
        })
        .map_err(|e: morphic::Error| e.to_string())?
    };

    let last = nth(2).unwrap_or(0);
    match name {
        "inc" if plugins.is_enabled::<Inc>() => Ok(last + 1),
        "dec" if plugins.is_enabled::<Dec>() => Ok(last - 1),
        "dbl" if plugins.is_enabled::<Dbl>() => Ok(last * 2),
        "triple" if plugins.is_enabled::<Triple>() => Ok(last * 3),
        "offset" => plugins
            .handlers
            .get::<Offset>()
            .map(|Offset(k)| last + k)
            .ok_or_else(|| "no offset handler registered".to_owned()),
        other => Err(format!("unknown instruction `{other}`")),
    }
}

fn main() {
    let mut plugins = Plugins::new();
    println!("all registered:  {:?}", plugins.names());

    // A handler that is not in the registry simply is not there. `Dec` is
    // removed, so the `dec` instruction below reports it as missing.
    plugins.disable::<Dec>();
    println!("after disable:   {:?}", plugins.names());

    let args = Value::list_from([Value::int(1), Value::int(2), Value::int(3)]);
    let program = Value::list_from(
        ["inc", "dec", "dbl", "triple", "offset", "nope"]
            .iter()
            .map(|n| Value::str(n))
            .collect::<Vec<_>>(),
    );

    for name in program
        .with_list_ref(|p| {
            p.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap()
    {
        match run(&plugins, &name, &args) {
            Ok(v) => {
                println!("{name:>7} -> {v}");
                args.list_push(Value::int(v)).unwrap();
            }
            Err(m) => println!("{name:>7} !! {m}"),
        }
    }

    println!("log: {args}");

    // A native function stored in a `Value` and called later. The body moves
    // the log in, which is what keeps it alive across a collection.
    let sum = Value::native(NativeFn::new("sum", {
        let log = args.clone();
        move |_ctx, _a| {
            let total = log
                .with_list_ref(|l| l.iter().filter_map(Value::as_int).sum::<i64>())
                .map_err(|e| morphic::Error::User(e.to_string()))?;
            Ok(Value::int(total))
        }
    }));

    // One context for the whole program. The recursion limit is per-thread, so
    // sharing it is what makes the limit hold across nested calls.
    let ctx = morphic::CallCtx::new();

    morphic::gc::collect();
    println!("sum of the log: {}", sum.call_with(&ctx, &[]).unwrap());

    // A user payload, mutated from a native function through a captured cell.
    let counter = Gc::new(GcRefCell::new(0i64));
    let tick = Value::native(NativeFn::from_user_mut(
        "tick",
        counter.clone(),
        |mut c, _ctx, _a| {
            *c += 1;
            Ok(Value::int(*c))
        },
    ));
    for _ in 0..3 {
        tick.call_with(&ctx, &[]).unwrap();
    }
    println!("ticks: {}", counter.borrow());

    // A cyclic value prints without hanging, because printing is depth-bounded.
    let node = Value::object_of_class("Node");
    node.obj_set("self", node.clone()).unwrap();
    println!("cyclic: {node}");

    // A callable object, with a default inherited from a prototype.
    let base = Value::object_of_class("Greeter");
    base.obj_set(
        morphic::CALL_PROP,
        Value::native(NativeFn::new("__call", |_ctx, _a| Ok(Value::str("hello")))),
    )
    .unwrap();

    let polite = Value::object_of_class("PoliteGreeter");
    polite.obj_set_proto(&base).unwrap();
    polite
        .obj_set(
            morphic::CALL_PROP,
            Value::native(NativeFn::new("__call", |_ctx, _a| {
                Ok(Value::str("hello, nice to meet you"))
            })),
        )
        .unwrap();

    println!("inherited call: {}", base.call_with(&ctx, &[]).unwrap());
    println!("overridden call: {}", polite.call_with(&ctx, &[]).unwrap());
}
