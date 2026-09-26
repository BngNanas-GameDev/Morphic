use morphic::dyn_fn::{
    BoxFuture, DynFn, DynFnMut, DynFnOnce, boxed_async, boxed_async_fn, boxed_async_fn_mut,
    boxed_fn, boxed_fn_mut,
};
use std::future::Future;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_waker() -> Waker {
    fn waker_clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    fn waker_nop(_: *const ()) {}
    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(waker_clone, waker_nop, waker_nop, waker_nop);
    // SAFETY: the vtable above never dereferences the data pointer.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

/// Drive a future to completion on the current thread.
///
/// The futures in this file never park, so a poll loop is enough and the crate
/// needs no async runtime as a dev-dependency.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

#[test]
fn a_plain_fn_is_usable_through_all_three_traits() {
    let mut f = |x: i32| x + 1;
    assert_eq!(DynFn::invoke(&f, 1), 2);
    assert_eq!(DynFnMut::invoke_mut(&mut f, 2), 3);
    assert_eq!(DynFnOnce::<i32, i32>::invoke_once(f, 3), 4);
}

#[test]
fn a_boxed_fn_is_callable_by_reference() {
    let f: Box<dyn DynFn<i32, i32> + Send + Sync> = boxed_fn(|x: i32| x * 2);
    assert_eq!(f.invoke(21), 42);
    assert_eq!(f.invoke(0), 0);
}

#[test]
fn a_boxed_fn_mut_keeps_its_state() {
    use std::sync::{Arc, Mutex};

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut f: Box<dyn DynFnMut<i32, i32> + Send> = {
        let seen = seen.clone();
        boxed_fn_mut(move |x: i32| {
            seen.lock().unwrap().push(x);
            x * 2
        })
    };
    assert_eq!(f.invoke_mut(1), 2);
    assert_eq!(f.invoke_mut(2), 4);
    assert_eq!(f.invoke_mut(3), 6);
    assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3]);
}

#[test]
fn boxed_async_fn_runs_to_completion() {
    let double: Box<dyn DynFn<i32, BoxFuture<'static, i32>> + Send + Sync> =
        boxed_async_fn(|x: i32| async move { x * 2 });
    assert_eq!(block_on(double.invoke(21)), 42);
}

#[test]
fn boxed_async_fn_mut_runs_and_keeps_state() {
    use std::sync::{Arc, Mutex};

    // `async move` copies a `Copy` capture into the future, so shared
    // accumulation has to go through a cell rather than a plain `i64`.
    let acc = Arc::new(Mutex::new(0i64));
    let mut add: Box<dyn DynFnMut<i64, BoxFuture<'static, i64>> + Send> = {
        let acc = acc.clone();
        boxed_async_fn_mut(move |n: i64| {
            let acc = acc.clone();
            async move {
                let mut guard = acc.lock().unwrap();
                *guard += n;
                *guard
            }
        })
    };
    assert_eq!(block_on(add.invoke_mut(1)), 1);
    assert_eq!(block_on(add.invoke_mut(2)), 3);
    assert_eq!(block_on(add.invoke_mut(3)), 6);
    assert_eq!(*acc.lock().unwrap(), 6);
}

#[test]
fn heterogeneous_handlers_share_one_erased_type() {
    let handlers: Vec<Box<dyn DynFn<i32, BoxFuture<'static, i32>> + Send + Sync>> = vec![
        boxed_async_fn(|x: i32| async move { x + 1 }),
        boxed_async_fn(|x: i32| async move { x * 10 }),
        boxed_async_fn(|x: i32| async move { -x }),
    ];

    let results: Vec<i32> = handlers.iter().map(|h| block_on(h.invoke(5))).collect();
    assert_eq!(results, vec![6, 50, -5]);
}

#[test]
fn boxed_async_erases_an_anonymous_future_type() {
    let fut: BoxFuture<'static, u32> = boxed_async(async { 7u32 });
    assert_eq!(block_on(fut), 7);
}

#[test]
fn a_zero_argument_callable_needs_an_explicit_argument_type() {
    let f: Box<dyn DynFn<(), String> + Send + Sync> =
        boxed_fn::<(), String, _>(|_: ()| "morphic".to_owned());
    assert_eq!(f.invoke(()), "morphic");

    let g: Box<dyn DynFn<(), BoxFuture<'static, String>> + Send + Sync> =
        boxed_async_fn::<(), String, _, _>(|_: ()| async { "morphic".to_owned() });
    assert_eq!(block_on(g.invoke(())), "morphic");
}

#[test]
fn dyn_fn_once_works_on_a_concrete_type() {
    let owned = String::from("consumed");
    let consume = move |_: ()| owned.len();
    assert_eq!(DynFnOnce::<(), usize>::invoke_once(consume, ()), 8);
}

#[test]
fn the_return_type_may_be_any_shape() {
    let unit: Box<dyn DynFn<i32, ()> + Send + Sync> = boxed_fn(|_| ());
    unit.invoke(1);

    let owned: Box<dyn DynFn<i32, Vec<u8>> + Send + Sync> =
        boxed_fn(|x: i32| vec![0u8; x as usize]);
    assert_eq!(owned.invoke(3).len(), 3);

    let pair: Box<dyn DynFn<(i32, i32), (i32, i32)> + Send + Sync> = boxed_fn(|(a, b)| (b, a));
    assert_eq!(pair.invoke((1, 2)), (2, 1));
}
