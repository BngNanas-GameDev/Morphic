use morphic::{Registry, RegistryError};
use std::any::TypeId;

#[test]
fn insert_and_get_by_type() {
    let mut r = Registry::new();
    assert!(r.is_empty());

    r.insert(42u32);
    r.insert("hello".to_owned());
    r.insert(vec![1u8, 2, 3]);

    assert_eq!(r.len(), 3);
    assert_eq!(r.get::<u32>(), Some(&42));
    assert_eq!(r.get::<String>().map(String::as_str), Some("hello"));
    assert_eq!(r.get::<Vec<u8>>().map(|v| v.len()), Some(3));
    assert!(r.contains::<u32>());
    assert!(!r.contains::<i32>());
}

#[test]
fn insert_replaces_and_returns_the_old_value() {
    let mut r = Registry::new();
    assert_eq!(r.insert(1u32), None);
    assert_eq!(r.insert(2u32), Some(1));
    assert_eq!(r.len(), 1);
    assert_eq!(r.get::<u32>(), Some(&2));
}

#[test]
fn try_insert_refuses_to_replace() {
    let mut r = Registry::new();
    r.try_insert(1u32).unwrap();
    assert_eq!(
        r.try_insert(2u32),
        Err(RegistryError::AlreadyPresent(TypeId::of::<u32>()))
    );
    assert_eq!(r.get::<u32>(), Some(&1));
}

#[test]
fn get_mut_allows_in_place_mutation() {
    let mut r = Registry::new();
    r.insert(vec![1u8, 2]);
    r.get_mut::<Vec<u8>>().unwrap().push(3);
    assert_eq!(r.get::<Vec<u8>>().unwrap().len(), 3);
}

#[test]
fn try_get_reports_the_missing_type() {
    let r = Registry::new();
    assert_eq!(
        r.try_get::<u32>(),
        Err(RegistryError::NotFound(TypeId::of::<u32>()))
    );
    assert!(
        r.try_get::<u32>()
            .unwrap_err()
            .to_string()
            .contains("no value")
    );
}

#[test]
fn remove_takes_the_value_out() {
    let mut r = Registry::new();
    r.insert(7u64);
    assert_eq!(r.remove::<u64>(), Some(7));
    assert_eq!(r.remove::<u64>(), None);
    assert!(r.is_empty());
}

#[test]
fn type_ids_and_typed_iteration() {
    let mut r = Registry::new();
    r.insert(1u32);
    r.insert(2u64);
    r.insert("s".to_owned());
    r.insert(vec![0u8]);

    assert_eq!(r.type_ids().count(), 4);
    assert!(r.type_ids().any(|t| t == TypeId::of::<u32>()));
    assert!(r.type_ids().any(|t| t == TypeId::of::<String>()));

    assert_eq!(r.values::<u32>().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(r.values::<u64>().copied().collect::<Vec<_>>(), vec![2]);
    assert_eq!(r.values::<f64>().count(), 0, "absent types yield nothing");
}

#[test]
fn only_one_value_per_type_can_be_registered() {
    let mut r = Registry::new();
    r.insert(1u32);
    r.insert(2u32);
    assert_eq!(r.len(), 1, "the type is the key, not the value");
    assert_eq!(r.get::<u32>(), Some(&2));
}

#[test]
fn clear_empties_the_registry() {
    let mut r = Registry::new();
    r.insert(1u32);
    r.insert(2u64);
    r.clear();
    assert!(r.is_empty());
    assert_eq!(r.len(), 0);
}

#[test]
fn zero_sized_types_are_distinct_keys() {
    let mut r = Registry::new();
    r.insert(());
    assert!(r.contains::<()>());
    assert!(!r.contains::<u8>());
    assert_eq!(r.len(), 1);
}

#[test]
fn two_registries_are_independent() {
    let mut a = Registry::new();
    let mut b = Registry::new();
    a.insert(1u32);
    assert!(!b.contains::<u32>());
    b.insert(2u32);
    assert_eq!(a.get::<u32>(), Some(&1));
    assert_eq!(b.get::<u32>(), Some(&2));
}
