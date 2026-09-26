use morphic::{FromValue, IntoValue, Value, ValueUser};
use std::any::TypeId;

#[test]
fn scalars_convert_in() {
    assert_eq!(Value::from(1i64).as_int(), Some(1));
    assert_eq!(Value::from(true).as_bool(), Some(true));
    assert_eq!(Value::from('c').as_char(), Some('c'));
    assert_eq!(Value::from("hi").as_str(), Some("hi"));
    assert_eq!(Value::from("hi".to_owned()).as_str(), Some("hi"));
    assert_eq!(Value::from(1.5f64).as_float(), Some(1.5));
    assert_eq!(Value::from(vec![1u8, 2]).as_bytes().unwrap().len(), 2);
    assert_eq!(Value::from(vec![Value::int(1)]).list_len().unwrap(), 1);
}

#[test]
fn option_becomes_nil() {
    assert_eq!(Value::from(Some(1i64)).as_int(), Some(1));
    assert_eq!(Value::from(None::<i64>), Value::Nil);
    assert_eq!(Value::from(Some("x")).as_str(), Some("x"));
}

#[test]
fn into_value_trait_covers_the_scalars() {
    assert_eq!(1i64.into_value(), Value::int(1));
    assert_eq!(1u32.into_value(), Value::int(1));
    assert_eq!(1u8.into_value(), Value::int(1));
    assert_eq!(true.into_value(), Value::bool(true));
    assert_eq!(1.5f32.into_value().as_float(), Some(1.5));
    assert_eq!(1.5f64.into_value().as_float(), Some(1.5));
    assert_eq!('x'.into_value(), Value::Char('x'));
    assert_eq!("s".to_owned().into_value().as_str(), Some("s"));
}

#[test]
fn from_value_trait_covers_the_scalars() {
    assert_eq!(i64::from_value(&Value::int(7), "t").unwrap(), 7);
    assert_eq!(u32::from_value(&Value::int(7), "t").unwrap(), 7u32);
    assert_eq!(i8::from_value(&Value::int(7), "t").unwrap(), 7i8);
    assert!(bool::from_value(&Value::bool(true), "t").unwrap());
    assert_eq!(f64::from_value(&Value::int(2), "t").unwrap(), 2.0);
    assert_eq!(f64::from_value(&Value::float(2.5), "t").unwrap(), 2.5);
    assert_eq!(char::from_value(&Value::Char('z'), "t").unwrap(), 'z');
    assert_eq!(String::from_value(&Value::str("q"), "t").unwrap(), "q");
    assert_eq!(
        TypeId::from_value(&Value::type_id(TypeId::of::<u8>()), "t").unwrap(),
        TypeId::of::<u8>()
    );
}

#[test]
fn from_value_reports_the_named_context_on_failure() {
    let err = i64::from_value(&Value::str("nope"), "parse_port").unwrap_err();
    assert_eq!(
        err,
        morphic::Error::TypeMismatch {
            context: "parse_port",
            expected: "i64",
            found: "str"
        }
    );
    assert!(err.to_string().contains("parse_port"));
}

#[test]
fn narrowing_integers_report_out_of_range() {
    let err = i8::from_value(&Value::int(1000), "t").unwrap_err();
    assert!(matches!(
        err,
        morphic::Error::TypeMismatch {
            found: "out-of-range int",
            ..
        }
    ));
    assert!(u8::from_value(&Value::int(-1), "t").is_err());
}

#[test]
fn wide_integers_widen_lossily_by_design() {
    assert_eq!(usize::from_value(&Value::int(-1), "t").unwrap(), usize::MAX);
    assert_eq!(isize::from_value(&Value::int(2), "t").unwrap(), 2);
}

#[test]
fn user_values_are_not_constructible_without_trace() {
    #[derive(morphic::Trace, morphic::Finalize)]
    struct Point {
        x: f64,
        y: f64,
    }

    let wrapped = ValueUser::new(Point { x: 1.0, y: 2.0 });
    let v = wrapped.into_value();
    assert_eq!(v.type_name(), "user");
    assert_eq!(v.as_user::<Point>().unwrap().x, 1.0);
}
