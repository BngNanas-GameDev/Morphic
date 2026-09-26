use morphic::{
    CALL_PROP, CallCtx, Error, Finalize, FromValue, Gc, GcRefCell, NativeFn, Object, Trace, Value,
    ValueMap,
};

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
    let rendered = format!("{list}");
    assert!(rendered.contains("..."), "{rendered}");
}

#[test]
fn a_cyclic_object_prints_within_the_depth_budget() {
    let node = Value::object_of_class("Node");
    node.obj_set("self", node.clone()).unwrap();
    let rendered = format!("{node}");
    assert!(rendered.starts_with("Node {"), "{rendered}");
    assert!(rendered.contains("..."), "{rendered}");
    assert!(
        rendered.len() < 4096,
        "printing must be bounded, got {} chars",
        rendered.len()
    );
}

#[test]
fn a_cyclic_map_prints_within_the_depth_budget() {
    let map = Value::map();
    map.map_insert(Value::str("self"), map.clone()).unwrap();
    let rendered = format!("{map}");
    assert!(rendered.contains("..."), "{rendered}");
    assert!(rendered.len() < 4096, "{}", rendered.len());
}

#[test]
fn a_cyclic_object_behind_a_list_also_terminates() {
    let list = Value::list();
    let node = Value::object_of_class("Node");
    node.obj_set("parent", list.clone()).unwrap();
    list.list_push(node.clone()).unwrap();
    assert!(format!("{list}").contains("..."));
    assert!(format!("{node}").contains("..."));
}

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
    assert_eq!(map.map_len().unwrap(), 1);
    assert_eq!(map.map_get(&Value::str("k")).unwrap().as_int(), Some(1));
    assert_eq!(map.map_get(&Value::str("nope")), Err(Error::KeyNotFound));
    assert_eq!(
        map.map_remove(&Value::str("k")).unwrap().unwrap().as_int(),
        Some(1)
    );
    assert!(map.map_remove(&Value::str("k")).unwrap().is_none());
    assert_eq!(map.map_len().unwrap(), 0);
}

#[test]
fn float_equality_is_total_so_it_can_be_a_key() {
    let nan = Value::float(f64::NAN);
    assert_eq!(nan, nan.clone(), "reflexivity must hold for map keys");
    assert_eq!(Value::float(0.0), Value::float(0.0));
    assert_ne!(
        Value::float(0.0),
        Value::float(-0.0),
        "bit pattern, not numeric"
    );

    let mut m = ValueMap::new();
    m.insert(nan.clone(), Value::str("first"));
    m.insert(nan.clone(), Value::str("second"));
    assert_eq!(m.len(), 1, "NaN must hit the same key");
    assert_eq!(m.get(&nan).unwrap().as_str(), Some("second"));
}

#[test]
fn int_and_float_are_the_same_key() {
    let mut m = ValueMap::new();
    m.insert(Value::int(1), Value::str("from int"));
    m.insert(Value::float(1.0), Value::str("from float"));
    assert_eq!(m.len(), 1);
    assert_eq!(m.get(&Value::int(1)).unwrap().as_str(), Some("from float"));
}

#[test]
fn containers_compare_by_identity_not_structure() {
    let a = Value::list_from([Value::int(1)]);
    let b = Value::list_from([Value::int(1)]);
    assert_eq!(a, a.clone(), "the same cell is equal to itself");
    assert_ne!(
        a, b,
        "two structurally identical cells are still different values"
    );
    assert!(!a.ptr_eq(&b));
    assert!(a.ptr_eq(&a.clone()));
}

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

    obj.obj_set("greeting", Value::str("hi")).unwrap();
    assert_eq!(obj.obj_get("greeting").unwrap().as_str(), Some("hi"));
    assert_eq!(
        proto.obj_get("greeting").unwrap().as_str(),
        Some("hello"),
        "writing to the child must not touch the parent"
    );
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
    assert_eq!(top.with_object_ref(|o| o.chain_len()).unwrap(), 3);
    assert!(top.proto().is_some());
    assert!(Value::int(0).proto().is_none());
}

#[test]
fn attaching_a_non_object_prototype_is_rejected() {
    assert!(Value::object().obj_set_proto(&Value::int(1)).is_err());
}

#[test]
fn object_default_is_empty() {
    let o = Object::new();
    assert!(o.is_empty());
    assert_eq!(o.len(), 0);
    assert!(o.class().is_none());
    assert!(o.proto().is_none());
}

#[test]
fn object_clear_drops_own_properties() {
    let mut o = Object::new();
    o.set("a", Value::int(1));
    o.set("b", Value::int(2));
    assert_eq!(o.len(), 2);
    o.clear();
    assert!(o.is_empty());
}

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
fn a_captured_cell_outlives_every_other_handle() {
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
}

#[test]
fn an_uncaptured_cell_does_not_keep_its_target_alive() {
    let cell = Gc::new(vec![1i64, 2, 3]);
    let len = Value::native(NativeFn::new("len", {
        let cell = cell.clone();
        move |_ctx, _args| Ok(Value::int(cell.len() as i64))
    }));

    drop(cell);
    morphic::gc::collect();

    assert_eq!(len.as_native().unwrap().capture_count(), 0);
    assert_eq!(len.call(&[]).unwrap().as_int(), Some(3));
}

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
    assert_eq!(bump.as_native().unwrap().capture_count(), 1);
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

    assert!(obj.is_callable());
    assert_eq!(obj.call(&[]).unwrap().as_str(), Some("from proto"));
}

#[test]
fn a_child_object_can_override_inherited_call() {
    let proto = Value::object();
    proto
        .obj_set(
            CALL_PROP,
            Value::native(NativeFn::new("__call", |_ctx, _a| Ok(Value::str("proto")))),
        )
        .unwrap();

    let obj = Value::object();
    obj.obj_set_proto(&proto).unwrap();
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
    assert!(Value::object().as_callable().is_none());
}

#[test]
fn infinite_native_recursion_is_bounded_not_a_stack_overflow() {
    let slot: Gc<GcRefCell<Option<Value>>> = Gc::new(GcRefCell::new(None));
    let bomb = Value::native(
        NativeFn::new("bomb", {
            let slot = slot.clone();
            move |ctx, _args| {
                let callee = slot
                    .borrow()
                    .clone()
                    .ok_or_else(|| Error::User("callee not installed".into()))?;
                callee.call_with(ctx, &[])
            }
        })
        .capture(slot.clone()),
    );
    *slot.borrow_mut() = Some(bomb.clone());

    let mut ctx = CallCtx::new();
    match bomb.call_with(&mut ctx, &[]) {
        Err(Error::RecursionLimit { limit }) => assert_eq!(limit, CallCtx::DEFAULT_MAX_DEPTH),
        other => panic!("expected a recursion limit, got {other:?}"),
    }
    assert_eq!(
        ctx.depth(),
        0,
        "the depth counter must unwind after the failure"
    );
}

#[test]
fn a_custom_recursion_limit_is_honoured() {
    let slot: Gc<GcRefCell<Option<Value>>> = Gc::new(GcRefCell::new(None));
    let bomb = Value::native(
        NativeFn::new("bomb", {
            let slot = slot.clone();
            move |ctx, _args| {
                let callee = slot.borrow().clone().unwrap();
                callee.call_with(ctx, &[])
            }
        })
        .capture(slot.clone()),
    );
    *slot.borrow_mut() = Some(bomb.clone());

    let mut ctx = CallCtx::with_max_depth(4);
    assert_eq!(ctx.max_depth(), 4);
    assert_eq!(
        bomb.call_with(&mut ctx, &[]),
        Err(Error::RecursionLimit { limit: 4 })
    );
}

#[test]
fn call_depth_is_tracked_while_nested() {
    let mut ctx = CallCtx::new();
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
    assert_eq!(outer.call_with(&mut ctx, &[]).unwrap().as_int(), Some(2));
    assert_eq!(ctx.depth(), 0);
}

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
    assert_eq!(
        Value::int(1).with_map(|_| ()).unwrap_err(),
        Error::TypeMismatch {
            context: "Value::with_map",
            expected: "map",
            found: "int"
        }
    );
    assert_eq!(
        Value::int(1).with_object(|_| ()).unwrap_err(),
        Error::TypeMismatch {
            context: "Value::with_object",
            expected: "object",
            found: "int"
        }
    );
}

#[test]
fn type_values_round_trip() {
    let tid = std::any::TypeId::of::<u32>();
    let v = Value::type_id(tid);
    assert_eq!(v.as_type_id(), Some(tid));
    assert_eq!(Value::int(0).as_type_id(), None);
    assert_eq!(Value::int(0).user_type_id(), None);
}

#[test]
fn bulk_accessors_beat_the_convenience_wrappers() {
    let list = Value::list_from([Value::int(1), Value::int(2)]);
    let sum = list.with_list_ref(|l| l.iter().filter_map(Value::as_int).sum::<i64>());
    assert_eq!(sum.unwrap(), 3);

    let doubled = list.with_list(|l| {
        for v in l.iter_mut() {
            *v = Value::int(v.as_int().unwrap_or(0) * 2);
        }
    });
    assert!(doubled.is_ok());
    assert_eq!(list.list_get(0).unwrap().as_int(), Some(2));
}
