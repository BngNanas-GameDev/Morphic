use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use morphic::dyn_fn::boxed_async_fn;
use morphic::{CallCtx, Gc, Value};
use std::collections::HashMap;
use std::future::Future;
use std::hint::black_box;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    fn nop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, nop, nop, nop);
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

enum Op {
    Double,
}

impl Op {
    async fn run(&self, x: i32) -> i32 {
        match self {
            Op::Double => x * 2,
        }
    }
}

fn bench_list_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_push_10k");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("morphic_list", |b| {
        b.iter(|| {
            let list = Value::list();
            for i in 0..10_000i64 {
                list.list_push(Value::int(black_box(i))).unwrap();
            }
            black_box(list.list_len().unwrap())
        })
    });
    group.bench_function("vec_push", |b| {
        b.iter(|| {
            let mut v: Vec<u64> = Vec::new();
            for i in 0..10_000u64 {
                v.push(black_box(i));
            }
            black_box(v.len())
        })
    });
    group.finish();
}

fn bench_map_lookup(c: &mut Criterion) {
    let map = Value::map();
    for i in 0..1000 {
        map.map_insert(Value::str(&format!("k{i:04}")), Value::int(i))
            .unwrap();
    }
    let mut native: HashMap<String, i64> = HashMap::new();
    for i in 0..1000 {
        native.insert(format!("k{i:04}"), i);
    }
    let mut group = c.benchmark_group("map_lookup_1k");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("morphic_map", |b| {
        b.iter(|| black_box(map.map_get_str(black_box("k0500")).unwrap()))
    });
    group.bench_function("morphic_map_reused_key", |b| {
        let key = Value::str("k0500");
        b.iter(|| black_box(map.map_get(black_box(&key)).unwrap()))
    });
    group.bench_function("hashmap", |b| {
        b.iter(|| black_box(native.get(black_box("k0500")).unwrap()))
    });
    group.finish();
}

fn bench_map_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("map_build_1k");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("morphic_map", |b| {
        b.iter(|| {
            let map = Value::map();
            for i in 0..1000i64 {
                map.map_insert(Value::int(black_box(i)), Value::int(i))
                    .unwrap();
            }
            black_box(map.map_len().unwrap())
        })
    });
    group.bench_function("hashmap", |b| {
        b.iter(|| {
            let mut native: HashMap<i64, i64> = HashMap::new();
            for i in 0..1000i64 {
                native.insert(black_box(i), i);
            }
            black_box(native.len())
        })
    });
    group.finish();
}

fn bench_proto_chain(c: &mut Criterion) {
    let root = Value::object();
    root.obj_set("x", Value::int(42)).unwrap();
    let mut child = root.clone();
    for _ in 0..8 {
        let next = Value::object();
        next.obj_set_proto(&child).unwrap();
        child = next;
    }
    let leaf = child;
    let own = Value::object();
    own.obj_set("x", Value::int(42)).unwrap();
    let mut group = c.benchmark_group("obj_get");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function(BenchmarkId::new("proto_chain", "depth8"), |b| {
        b.iter(|| black_box(leaf.obj_get(black_box("x")).unwrap()))
    });
    group.bench_function(BenchmarkId::new("own_property", "depth0"), |b| {
        b.iter(|| black_box(own.obj_get(black_box("x")).unwrap()))
    });
    group.finish();
}

fn bench_native_call(c: &mut Criterion) {
    use morphic::NativeFn;
    let f = Value::native(NativeFn::new("one", |_ctx, _args| Ok(Value::int(1))));
    let ctx = CallCtx::new();
    let direct = || Value::int(1);
    let mut group = c.benchmark_group("trivial_call");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("morphic_call_with", |b| {
        b.iter(|| black_box(f.call_with(&ctx, black_box(&[])).unwrap()))
    });
    group.bench_function("direct_closure", |b| b.iter(|| black_box(direct())));
    group.finish();
}

fn bench_gc_collect(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_100_cells");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("gc_new_plus_collect", |b| {
        b.iter(|| {
            let cells: Vec<Gc<Vec<i64>>> = (0..100)
                .map(|_| Gc::new(black_box(vec![1i64; 8])))
                .collect();
            drop(cells);
            morphic::gc::collect();
        })
    });
    group.bench_function("rc_new_plus_drop", |b| {
        b.iter(|| {
            let cells: Vec<std::rc::Rc<Vec<i64>>> = (0..100)
                .map(|_| std::rc::Rc::new(black_box(vec![1i64; 8])))
                .collect();
            drop(cells);
        })
    });
    group.finish();
}

fn bench_boxed_async(c: &mut Criterion) {
    let f = boxed_async_fn(|x: i32| async move { x * 2 });
    let op = Op::Double;
    let mut group = c.benchmark_group("async_invoke");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("boxed_async_fn", |b| {
        b.iter(|| {
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            let mut fut = f.invoke(black_box(21));
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => black_box(v),
                Poll::Pending => unreachable!(),
            }
        })
    });
    group.bench_function("enum_dispatch", |b| {
        b.iter(|| {
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            let fut = op.run(black_box(21));
            let mut fut = std::pin::pin!(fut);
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => black_box(v),
                Poll::Pending => unreachable!(),
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_list_push,
    bench_map_lookup,
    bench_map_build,
    bench_proto_chain,
    bench_native_call,
    bench_gc_collect,
    bench_boxed_async,
);
criterion_main!(benches);
