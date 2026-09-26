use morphic::{
    CALL_PROP, CallCtx, Error, Finalize, FromValue, Gc, GcRefCell, MAX_PROTO_DEPTH, NativeFn,
    Object, Trace, Value, ValueMap,
};

/// `Value::call` was deliberately removed from the public API: it built a fresh
/// `CallCtx` per invocation, so a body re-entering through it reset the depth
/// counter and defeated the recursion guard. Tests opt back in through this
/// shim, which keeps the removal visible rather than papering over it.
trait CallExt {
    fn call(&self, args: &[Value]) -> morphic::Result<Value>;
}

impl CallExt for Value {
    fn call(&self, args: &[Value]) -> morphic::Result<Value> {
        self.call_with(&CallCtx::new(), args)
    }
}

// ---------------------------------------------------------------- scalars

#[test]
fn scalar_roundtrip() {
    assert_eq!(Value::int(7).as_int(), Some(7));
    assert_eq!(Value::float(1.5).as_float(), Some(1.5));
    assert_eq!(Value::str("hi").as_str(), Some("hi"));
    assert_eq!(Value::bool(true).as_bool(), Some(true));
    assert_eq!(Value::char('x').as_char(), Some('x'));
    assert_eq!(Value::bytes(vec![1u8, 2]).as_bytes(), Some(&[1u8, 2][..]));
}

#[test]
fn int_widens_to_float_but_not_the_reverse() {
    assert_eq!(Value::int(3).as_float(), Some(3.0));
    assert_eq!(Value::float(3.0).as_int(), None);
}

#[test]
fn type_names_cover_every_shape() {
    assert_eq!(Value::Nil.type_name(), "nil");
    assert_eq!(Value::bool(true).type_name(), "bool");
    assert_eq!(Value::int(0).type_name(), "int");
    assert_eq!(Value::float(0.0).type_name(), "float");
    assert_eq!(Value::char('a').type_name(), "char");
    assert_eq!(Value::str("").type_name(), "str");
    assert_eq!(Value::bytes(vec![]).type_name(), "bytes");
    assert_eq!(Value::list().type_name(), "list");
    assert_eq!(Value::map().type_name(), "map");
    assert_eq!(Value::object().type_name(), "object");
    assert_eq!(Value::NIL, Value::Nil);
    assert!(Value::Nil.is_nil());
}

// ------------------------------------------------------------------ lists

#[test]
fn list_is_a_mutable_sequence() {
    let list = Value::list();
    for i in 0..5 {
        list.list_push(Value::int(i)).unwrap();
    }
    assert_eq!(list.list_len().unwrap(), 5);
    assert_eq!(list.list_get(3).unwrap().as_int(), Some(3));
    assert_eq!(list.list_pop().unwrap().as_int(), Some(4));
    assert_eq!(list.list_len().unwrap(), 4);
}

#[test]
fn list_index_out_of_bounds_names_the_index_and_length() {
    let list = Value::list_from([Value::int(1)]);
    assert_eq!(
        list.list_get(9).unwrap_err(),
        Error::IndexOutOfBounds { index: 9, len: 1 }
    );
}

#[test]
fn popping_an_empty_list_is_an_error_not_a_panic() {
    assert!(Value::list().list_pop().is_err());
}

// ------------------------------------------------------------ cycles, GC

#[test]
fn self_referential_list_survives_collection() {
    let list = Value::list();
    list.list_push(Value::int(1)).unwrap();
    list.list_push(list.clone()).unwrap();

    morphic::gc::collect();

    assert!(list.list_get(1).unwrap().ptr_eq(&list));
    assert_eq!(list.list_get(0).unwrap().as_int(), Some(1));
}

#[test]
fn cyclic_object_graph_survives_collection() {
    let parent = Value::object();
    let child = Value::object();
    parent.obj_set("child", child.clone()).unwrap();
    child.obj_set("parent", parent.clone()).unwrap();

    morphic::gc::collect();

    let back = parent.obj_get("child").unwrap();
    assert!(back.ptr_eq(&child));
    let up = back.obj_get("parent").unwrap();
    assert!(up.ptr_eq(&parent));
}

#[test]
fn cycle_display_terminates() {
    let list = Value::list();
    list.list_push(list.clone()).unwrap();
    assert!(format!("{list}").contains("..."));
}

#[test]
fn a_cyclic_object_prints_within_the_depth_budget() {
    let node = Value::object_of_class("Node");
    node.obj_set("self", node.clone()).unwrap();
    let rendered = format!("{node}");
    assert!(rendered.starts_with("Node {"), "{rendered}");
    assert!(rendered.contains("..."), "{rendered}");
    assert!(rendered.len() < 4096, "{} chars", rendered.len());
}

#[test]
fn a_cyclic_map_prints_within_the_depth_budget() {
    let map = Value::map();
    map.map_insert(Value::str("self"), map.clone()).unwrap();
    assert!(format!("{map}").contains("..."));
}

// ------------------------------------------------------------------ maps

#[test]
fn map_preserves_insertion_order() {
    let mut m = ValueMap::new();
    m.insert(Value::str("z"), Value::int(1));
    m.insert(Value::str("a"), Value::int(2));
    m.insert(Value::str("z"), Value::int(3));

    assert_eq!(m.len(), 2);
    let keys: Vec<_> = m.keys().filter_map(Value::as_str).collect();
    assert_eq!(keys, vec!["z", "a"]);
    assert_eq!(m.get_str("z").unwrap().as_int(), Some(3));
    assert!(m.contains_key(&Value::str("a")));
}

#[test]
fn map_try_insert_refuses_duplicates() {
    let mut m = ValueMap::new();
    m.try_insert(Value::str("k"), Value::int(1)).unwrap();
    assert_eq!(
        m.try_insert(Value::str("k"), Value::int(2)),
        Err(Error::DuplicateKey)
    );
}

#[test]
fn map_remove_and_missing_key() {
    let map = Value::map_from([("k", Value::int(1))]);
    assert_eq!(map.map_get(&Value::str("k")).unwrap().as_int(), Some(1));
    assert_eq!(map.map_get(&Value::str("nope")), Err(Error::KeyNotFound));
    assert_eq!(
        map.map_remove(&Value::str("k")).unwrap().unwrap().as_int(),
        Some(1)
    );
    assert_eq!(map.map_len().unwrap(), 0);
}

// B7 regression: cross-kind numeric equality was not transitive, so three
// inserts collapsed to two and one value was silently destroyed.
#[test]
fn int_and_float_are_distinct_keys_so_equality_stays_transitive() {
    assert_ne!(Value::int(1), Value::float(1.0));
    assert_eq!(Value::int(1), Value::int(1));
    assert_eq!(Value::float(1.0), Value::float(1.0));

    let mut m = ValueMap::new();
    m.insert(Value::int(1), Value::str("int"));
    m.insert(Value::float(1.0), Value::str("float"));
    assert_eq!(m.len(), 2, "numerically equal values must not collide");
    assert_eq!(m.get(&Value::int(1)).unwrap().as_str(), Some("int"));
    assert_eq!(m.get(&Value::float(1.0)).unwrap().as_str(), Some("float"));
}

#[test]
fn value_equality_is_transitive_across_the_2_53_boundary() {
    let a = Value::int(((1u64 << 53) + 1) as i64);
    let b = Value::int((1u64 << 53) as i64);
    let c = Value::float((1u64 << 53) as f64);

    assert!(a == c || a != c);
    assert!(b == c || b != c);
    // The property that actually matters: never a == c && c == b && a != b.
    assert!(
        !((a == c) && (c == b) && (a != b)),
        "equality is transitive"
    );

    let mut m = ValueMap::new();
    m.insert(a.clone(), Value::str("A"));
    m.insert(c.clone(), Value::str("C"));
    m.insert(b.clone(), Value::str("B"));
    assert_eq!(m.len(), 3, "no value may be lost");
}

#[test]
fn float_equality_is_reflexive_by_bit_pattern() {
    let nan = Value::float(f64::NAN);
    assert_eq!(nan, nan.clone());
    assert_eq!(Value::float(0.0), Value::float(0.0));
    assert_ne!(Value::float(0.0), Value::float(-0.0));

    let mut m = ValueMap::new();
    m.insert(nan.clone(), Value::str("first"));
    m.insert(nan.clone(), Value::str("second"));
    assert_eq!(m.len(), 1, "NaN must hit the same key");
    assert_eq!(m.get(&nan).unwrap().as_str(), Some("second"));
}

#[test]
fn value_is_a_real_eq() {
    fn assert_eq_trait<T: Eq>() {}
    assert_eq_trait::<Value>();
}

#[test]
fn containers_compare_by_identity_not_structure() {
    let a = Value::list_from([Value::int(1)]);
    let b = Value::list_from([Value::int(1)]);
    assert_eq!(a, a.clone());
    assert_ne!(a, b);
    assert!(!a.ptr_eq(&b));
    assert!(a.ptr_eq(&a.clone()));
}

// ------------------------------------------------------------- prototypes

#[test]
fn object_inherits_through_the_prototype_chain() {
    let proto = Value::object();
    proto.obj_set("greeting", Value::str("hello")).unwrap();

    let obj = Value::object();
    obj.obj_set_proto(&proto).unwrap();

    assert_eq!(obj.obj_get("greeting").unwrap().as_str(), Some("hello"));
    assert!(obj.obj_has("greeting").unwrap());
    assert!(
        obj.with_object_ref(|o| o.get_local("greeting").is_none())
            .unwrap()
    );
    assert_eq!(proto.with_object_ref(|o| o.len()).unwrap(), 1);
}

#[test]
fn shadow_removal_restores_the_inherited_value() {
    let proto = Value::object();
    proto.obj_set("x", Value::int(1)).unwrap();
    let obj = Value::object();
    obj.obj_set_proto(&proto).unwrap();

    obj.obj_set("x", Value::int(2)).unwrap();
    assert_eq!(obj.obj_get("x").unwrap().as_int(), Some(2));
    obj.obj_remove("x").unwrap();
    assert_eq!(obj.obj_get("x").unwrap().as_int(), Some(1));
}

#[test]
fn object_reports_its_class_and_keys() {
    let obj = Value::object_of_class("Point");
    obj.obj_set("x", Value::int(1)).unwrap();
    obj.obj_set("y", Value::int(2)).unwrap();

    let (class, keys, len) = obj
        .with_object_ref(|o| {
            (
                o.class().map(str::to_owned),
                o.keys().into_iter().map(str::to_owned).collect::<Vec<_>>(),
                o.len(),
            )
        })
        .unwrap();
    assert_eq!(class.as_deref(), Some("Point"));
    assert_eq!(keys, vec!["x", "y"]);
    assert_eq!(len, 2);
}

#[test]
fn object_chain_length_is_reported() {
    let base = Value::object();
    let mid = Value::object();
    let top = Value::object();
    mid.obj_set_proto(&base).unwrap();
    top.obj_set_proto(&mid).unwrap();
    assert_eq!(top.with_object_ref(|o| o.chain_len()).unwrap().unwrap(), 3);
    assert!(top.proto().is_some());
    assert!(Value::int(0).proto().is_none());
}

#[test]
fn attaching_a_non_object_prototype_is_rejected() {
    assert!(Value::object().obj_set_proto(&Value::int(1)).is_err());
}

// B1 regression: a prototype cycle used to spin forever at constant memory.
// Every one of these entry points hung; now all of them terminate.
#[test]
fn a_two_cycle_prototype_terminates_instead_of_hanging() {
    let a = Value::object();
    let b = Value::object();
    a.obj_set_proto(&b).unwrap();
    b.obj_set_proto(&a).unwrap();

    assert_eq!(
        a.obj_get("absent"),
        Err(Error::ProtoCycle {
            limit: MAX_PROTO_DEPTH
        })
    );
    assert_eq!(
        a.obj_has("absent"),
        Err(Error::ProtoCycle {
            limit: MAX_PROTO_DEPTH
        })
    );
    assert!(a.with_object_ref(|o| o.has("absent")).unwrap().is_err());
    assert!(a.with_object_ref(|o| o.chain_len()).unwrap().is_err());
    assert!(matches!(a.as_callable(), Err(Error::ProtoCycle { .. })));
    assert!(a.call(&[]).is_err());
    assert!(!a.is_callable(), "a cyclic object cannot be callable");
}

#[test]
fn a_self_cycle_prototype_terminates() {
    let a = Value::object();
    a.obj_set_proto(&a).unwrap();
    assert!(a.obj_get("absent").is_err());
}

#[test]
fn a_long_but_acyclic_chain_still_resolves() {
    let mut tail = Value::object();
    tail.obj_set("needle", Value::int(42)).unwrap();
    for _ in 0..(MAX_PROTO_DEPTH - 2) {
        let next = Value::object();
        next.obj_set_proto(&tail).unwrap();
        tail = next;
    }
    assert_eq!(tail.obj_get("needle").unwrap().as_int(), Some(42));
}

#[test]
fn object_default_and_clear() {
    let mut o = Object::new();
    o.set("a", Value::int(1));
    o.set("b", Value::int(2));
    assert_eq!(o.len(), 2);
    o.clear();
    assert!(o.is_empty());
}

// B3 regression: an unreadable prototype used to be reported as "absent".
#[test]
fn a_borrowed_prototype_is_not_reported_as_absent() {
    let proto = Value::object();
    proto.obj_set("greeting", Value::str("hello")).unwrap();
    let child = Value::object();
    child.obj_set_proto(&proto).unwrap();

    assert_eq!(child.obj_get("greeting").unwrap().as_str(), Some("hello"));

    proto
        .with_object(|_| {
            assert_eq!(
                child.obj_get("greeting"),
                Err(Error::BorrowConflict("a prototype")),
                "must not be misreported as KeyNotFound"
            );
            assert_eq!(
                child.obj_has("greeting"),
                Err(Error::BorrowConflict("a prototype"))
            );
        })
        .unwrap();

    assert_eq!(child.obj_get("greeting").unwrap().as_str(), Some("hello"));
}

#[test]
fn chain_len_does_not_undercount_when_a_link_is_borrowed() {
    let base = Value::object();
    let mid = Value::object();
    let top = Value::object();
    mid.obj_set_proto(&base).unwrap();
    top.obj_set_proto(&mid).unwrap();

    assert_eq!(top.with_object_ref(|o| o.chain_len()).unwrap().unwrap(), 3);

    mid.with_object(|_| {
        let n = top.with_object_ref(|o| o.chain_len()).unwrap();
        assert_eq!(n, Err(Error::BorrowConflict("a prototype")));
    })
    .unwrap();
}

#[test]
fn a_borrowed_callable_object_is_not_reported_as_not_callable() {
    let obj = Value::object_of_class("Adder");
    obj.obj_set(
        CALL_PROP,
        Value::native(NativeFn::new("__call", |_ctx, args| {
            Ok(Value::int(
                args.first().and_then(Value::as_int).unwrap_or(0) * 2,
            ))
        })),
    )
    .unwrap();

    assert!(obj.is_callable());
    assert_eq!(obj.call(&[Value::int(21)]).unwrap().as_int(), Some(42));

    obj.with_object(|_| {
        assert_eq!(
            obj.as_callable().err(),
            Some(Error::BorrowConflict("an object")),
            "must not be misreported as NotCallable"
        );
    })
    .unwrap();
}

// B6 regression: these two used to panic on safe input.
#[test]
fn accessors_do_not_panic_on_a_mutably_borrowed_object() {
    let obj = Value::object();
    obj.with_object(|o| o.set_proto(gc_proto())).unwrap();
    obj.with_object(|_| {
        assert!(obj.proto().is_none(), "returns None, not a panic");
        assert!(!obj.is_callable(), "returns false, not a panic");
    })
    .unwrap();
}

fn gc_proto() -> Gc<GcRefCell<Object>> {
    Gc::new(GcRefCell::new(Object::new()))
}

// -------------------------------------------------------------- functions

#[test]
fn native_functions_receive_arguments() {
    let add = Value::native(NativeFn::new("add", |_ctx, args| {
        let mut total = 0i64;
        for a in args {
            total += i64::from_value(a, "add")?;
        }
        Ok(Value::int(total))
    }));

    assert_eq!(
        add.call(&[Value::int(1), Value::int(2)]).unwrap().as_int(),
        Some(3)
    );
    assert_eq!(add.call(&[]).unwrap().as_int(), Some(0));
    assert!(add.is_callable());
    assert_eq!(add.as_native().unwrap().name(), "add");
}

#[test]
fn native_functions_report_argument_type_errors() {
    let add = Value::native(NativeFn::new("add", |_ctx, args| {
        let a = i64::from_value(args.first().unwrap_or(&Value::Nil), "add")?;
        let b = i64::from_value(args.get(1).unwrap_or(&Value::Nil), "add")?;
        Ok(Value::int(a + b))
    }));
    assert!(add.call(&[Value::str("x")]).is_err());
    assert!(add.call(&[]).is_err());
}

#[test]
fn objects_are_callable_through_the_call_property() {
    let callable = Value::object_of_class("Adder");
    callable
        .obj_set(
            CALL_PROP,
            Value::native(NativeFn::new("__call", |_ctx, args| {
                Ok(Value::int(
                    args.first().and_then(Value::as_int).unwrap_or(0) * 2,
                ))
            })),
        )
        .unwrap();

    assert!(callable.is_callable());
    assert_eq!(callable.call(&[Value::int(21)]).unwrap().as_int(), Some(42));
}

#[test]
fn a_callable_object_inherits_call_from_its_prototype() {
    let proto = Value::object();
    proto
        .obj_set(
            CALL_PROP,
            Value::native(NativeFn::new("__call", |_ctx, _a| {
                Ok(Value::str("from proto"))
            })),
        )
        .unwrap();

    let obj = Value::object();
    obj.obj_set_proto(&proto).unwrap();
    assert_eq!(obj.call(&[]).unwrap().as_str(), Some("from proto"));

    obj.obj_set(
        CALL_PROP,
        Value::native(NativeFn::new("__call", |_ctx, _a| Ok(Value::str("own")))),
    )
    .unwrap();
    assert_eq!(obj.call(&[]).unwrap().as_str(), Some("own"));
}

#[test]
fn non_callables_report_not_callable() {
    assert_eq!(Value::int(1).call(&[]), Err(Error::NotCallable));
    assert!(!Value::int(1).is_callable());
    assert_eq!(
        Value::object().as_callable().err(),
        Some(Error::NotCallable)
    );
}

// ---------------------------------------------------------------- user data

#[test]
fn user_payloads_round_trip_through_the_erased_heap() {
    #[derive(Debug, PartialEq, Trace, Finalize)]
    struct Config {
        retries: u32,
        name: String,
    }

    let v = Value::user(Config {
        retries: 3,
        name: "primary".into(),
    });

    let cfg = v.as_user::<Config>().unwrap();
    assert_eq!(cfg.retries, 3);
    assert_eq!(cfg.name, "primary");
    assert!(v.is_user::<Config>());
    assert!(!v.is_user::<u64>());
    assert!(v.as_user::<u64>().is_none());
    assert!(v.user_type_id().is_some());
}

#[test]
fn user_payloads_can_be_given_behaviour_safely() {
    #[derive(Trace, Finalize)]
    struct Counter(i64);

    let bump = Value::native(NativeFn::from_user(
        "bump",
        Gc::new(Counter(0)),
        |c, _ctx, args| {
            let n = args.first().and_then(Value::as_int).unwrap_or(1);
            Ok(Value::int(c.0 + n))
        },
    ));

    assert_eq!(bump.call(&[Value::int(5)]).unwrap().as_int(), Some(5));
    assert_eq!(
        bump.call(&[Value::int(3)]).unwrap().as_int(),
        Some(3),
        "from_user hands out shared access, so nothing accumulates"
    );
}

#[test]
fn a_mutable_user_cell_can_be_driven_by_a_native_function() {
    #[derive(Trace, Finalize, Default)]
    struct Counter(i64);

    let cell = Gc::new(GcRefCell::new(Counter::default()));
    let bump = Value::native(NativeFn::from_user_mut(
        "bump",
        cell.clone(),
        |mut c, _ctx, args| {
            c.0 += args.first().and_then(Value::as_int).unwrap_or(1);
            Ok(Value::int(c.0))
        },
    ));

    assert_eq!(bump.call(&[Value::int(1)]).unwrap().as_int(), Some(1));
    assert_eq!(bump.call(&[Value::int(10)]).unwrap().as_int(), Some(11));
    assert_eq!(cell.borrow().0, 11);
}

#[test]
fn user_payloads_survive_collection_through_their_behaviour() {
    #[derive(Trace, Finalize)]
    struct Bag(Vec<Value>);

    let bag = Value::native(NativeFn::from_user_mut(
        "push",
        Gc::new(GcRefCell::new(Bag(Vec::new()))),
        |mut b, _ctx, args| {
            b.0.push(args.first().cloned().unwrap_or(Value::Nil));
            Ok(Value::int(b.0.len() as i64))
        },
    ));

    assert_eq!(bag.call(&[Value::int(1)]).unwrap().as_int(), Some(1));
    morphic::gc::collect();
    assert_eq!(bag.call(&[Value::int(2)]).unwrap().as_int(), Some(2));
}

// B2: a `Gc` captured by a body is sound (the collector roots it by refcount)
// but is never reclaimed. This test pins the documented behaviour so a future
// `boa_gc` change is caught.
#[test]
fn a_cell_captured_by_a_body_is_safe_but_never_reclaimed() {
    let cell = Gc::new(vec![10i64, 20, 30]);
    let sum = Value::native(NativeFn::new("sum", {
        let cell = cell.clone();
        move |_ctx, _args| Ok(Value::int(cell.iter().sum::<i64>()))
    }));

    drop(cell);
    morphic::gc::collect();

    assert_eq!(
        sum.call(&[]).unwrap().as_int(),
        Some(60),
        "sound: the captured cell is still usable after every strong handle is dropped"
    );
}

// ------------------------------------------------------------- recursion

#[test]
fn infinite_native_recursion_is_bounded_not_a_stack_overflow() {
    let slot: Gc<GcRefCell<Option<Value>>> = Gc::new(GcRefCell::new(None));
    let bomb = Value::native(NativeFn::new("bomb", {
        let slot = slot.clone();
        move |ctx, _args| {
            let callee = slot
                .borrow()
                .clone()
                .ok_or_else(|| Error::User("callee not installed".into()))?;
            callee.call_with(ctx, &[])
        }
    }));
    *slot.borrow_mut() = Some(bomb.clone());

    let ctx = CallCtx::new();
    match bomb.call_with(&ctx, &[]) {
        Err(Error::RecursionLimit { limit }) => assert_eq!(limit, CallCtx::DEFAULT_MAX_DEPTH),
        other => panic!("expected a recursion limit, got {other:?}"),
    }
    assert_eq!(ctx.depth(), 0, "the depth counter must unwind");
}

// B4 regression. The first attempt at this fix only removed `Value::call` on the
// theory that every nested call would then have to share one `CallCtx`. That was
// wrong: a body can always build a fresh `CallCtx::new()` and pass *that* down,
// which silently reset the budget and overflowed the stack. The depth counter
// therefore lives in a thread-local, which cannot be reset from inside a body.
#[test]
fn recursion_cannot_be_bypassed_by_a_fresh_context() {
    let slot: Gc<GcRefCell<Option<Value>>> = Gc::new(GcRefCell::new(None));
    let bomb = Value::native(NativeFn::new("bomb", {
        let slot = slot.clone();
        move |_ctx, _args| {
            // A brand-new context per level: the exact shape that used to
            // defeat the guard.
            let fresh = CallCtx::new();
            let callee = slot.borrow().clone().unwrap();
            callee.call_with(&fresh, &[])
        }
    }));
    *slot.borrow_mut() = Some(bomb.clone());

    match bomb.call(&[]) {
        Err(Error::RecursionLimit { limit }) => assert_eq!(limit, CallCtx::DEFAULT_MAX_DEPTH),
        other => panic!("expected RecursionLimit despite the fresh context, got {other:?}"),
    }
}

#[test]
fn a_custom_recursion_limit_is_honoured() {
    let slot: Gc<GcRefCell<Option<Value>>> = Gc::new(GcRefCell::new(None));
    let bomb = Value::native(NativeFn::new("bomb", {
        let slot = slot.clone();
        move |ctx, _args| {
            let callee = slot.borrow().clone().unwrap();
            callee.call_with(ctx, &[])
        }
    }));
    *slot.borrow_mut() = Some(bomb.clone());

    let ctx = CallCtx::with_max_depth(4);
    assert_eq!(ctx.max_depth(), 4);
    assert_eq!(
        bomb.call_with(&ctx, &[]),
        Err(Error::RecursionLimit { limit: 4 })
    );
}

// B5 regression: depth used to leak on panic, so a later trivial call saw a
// spurious RecursionLimit.
#[test]
fn the_depth_counter_unwinds_even_when_a_body_panics() {
    let panicky = Value::native(NativeFn::new("boom", |_ctx, _a| {
        panic!("deliberate");
    }));
    let fine = Value::native(NativeFn::new("fine", |_ctx, _a| Ok(Value::int(7))));

    let ctx = CallCtx::new();
    for _ in 0..8 {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = panicky.call_with(&ctx, &[]);
        }));
    }

    assert_eq!(ctx.depth(), 0, "panics must not accumulate depth");
    assert_eq!(
        fine.call_with(&ctx, &[]).unwrap().as_int(),
        Some(7),
        "a trivial call must still succeed"
    );
}

#[test]
fn call_depth_is_tracked_while_nested() {
    let ctx = CallCtx::new();
    assert_eq!(ctx.depth(), 0);
    let inner = Value::native(NativeFn::new("inner", |ctx, _a| {
        assert_eq!(ctx.depth(), 2, "nested calls accumulate depth");
        Ok(Value::int(1))
    }));
    let outer = Value::native(NativeFn::new("outer", {
        let inner = inner.clone();
        move |ctx, _a| {
            assert_eq!(ctx.depth(), 1);
            inner
                .call_with(ctx, &[])
                .map(|v| Value::int(v.as_int().unwrap() + 1))
        }
    }));
    assert_eq!(outer.call_with(&ctx, &[]).unwrap().as_int(), Some(2));
    assert_eq!(ctx.depth(), 0);
}

// -------------------------------------------------------------- borrowing

#[test]
fn borrow_conflicts_are_reported_not_panicked() {
    let list = Value::list();
    list.with_list(|_| {
        assert_eq!(
            list.list_len(),
            Err(Error::BorrowConflict("a list")),
            "a shared read cannot be taken while a mutable borrow is live"
        );
    })
    .unwrap();
    assert_eq!(list.list_len().unwrap(), 0);
}

#[test]
fn borrow_conflicts_on_maps_and_objects_too() {
    let map = Value::map();
    map.with_map(|_| {
        assert_eq!(
            map.map_insert(Value::int(1), Value::int(1)),
            Err(Error::BorrowConflict("a map"))
        );
    })
    .unwrap();

    let obj = Value::object();
    obj.with_object(|_| {
        assert_eq!(
            obj.obj_set("k", Value::int(1)),
            Err(Error::BorrowConflict("an object"))
        );
    })
    .unwrap();
}

#[test]
fn mismatch_on_the_wrong_shape_is_descriptive() {
    assert_eq!(
        Value::int(1).with_list(|_| ()).unwrap_err(),
        Error::TypeMismatch {
            context: "Value::with_list",
            expected: "list",
            found: "int"
        }
    );
    assert!(Value::int(1).with_map(|_| ()).is_err());
    assert!(Value::int(1).with_object(|_| ()).is_err());
}

#[test]
fn bulk_accessors_beat_the_convenience_wrappers() {
    let list = Value::list_from([Value::int(1), Value::int(2)]);
    let sum = list.with_list_ref(|l| l.iter().filter_map(Value::as_int).sum::<i64>());
    assert_eq!(sum.unwrap(), 3);

    assert!(
        list.with_list(|l| l.iter_mut().for_each(|v| *v = Value::int(0)))
            .is_ok()
    );
    assert_eq!(list.list_get(0).unwrap().as_int(), Some(0));
}

#[test]
fn type_values_round_trip() {
    let tid = std::any::TypeId::of::<u32>();
    let v = Value::type_id(tid);
    assert_eq!(v.as_type_id(), Some(tid));
    assert_eq!(Value::int(0).as_type_id(), None);
    assert_eq!(Value::int(0).user_type_id(), None);
}
