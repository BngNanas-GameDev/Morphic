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
fn narrow_platform_ints_reject_what_they_cannot_represent() {
    assert_eq!(isize::from_value(&Value::int(2), "t").unwrap(), 2);
    assert_eq!(isize::from_value(&Value::int(-2), "t").unwrap(), -2);
    assert_eq!(usize::from_value(&Value::int(2), "t").unwrap(), 2);

    // The bug: `-1 as usize` is `usize::MAX`, so a negative length became the
    // largest possible one, silently.
    assert!(matches!(
        usize::from_value(&Value::int(-1), "t"),
        Err(morphic::Error::NegativeToUnsigned {
            context: "t",
            found: -1
        })
    ));
    assert!(usize::from_value(&Value::int(i64::MIN), "t").is_err());
}

#[test]
fn platform_int_type_errors_name_the_type_that_was_asked_for() {
    let err = usize::from_value(&Value::str("nope"), "parse_len").unwrap_err();
    assert_eq!(
        err,
        morphic::Error::TypeMismatch {
            context: "parse_len",
            expected: "usize",
            found: "str"
        }
    );
    assert!(err.to_string().contains("parse_len"));

    match isize::from_value(&Value::bool(true), "parse_off") {
        Err(morphic::Error::TypeMismatch { expected, .. }) => assert_eq!(expected, "isize"),
        other => panic!("expected a TypeMismatch, got {other:?}"),
    }
}

#[test]
fn negative_to_unsigned_explains_itself() {
    let err = usize::from_value(&Value::int(-7), "parse_len").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("parse_len"), "{text}");
    assert!(text.contains("-7"), "{text}");
}

#[test]
fn u64_into_value_is_fallible_above_i64_max() {
    assert_eq!(<u64 as TryInto<Value>>::try_into(0).unwrap(), Value::int(0));
    assert_eq!(
        <u64 as TryInto<Value>>::try_into(i64::MAX as u64).unwrap(),
        Value::int(i64::MAX),
        "i64::MAX is representable, so it must succeed"
    );

    let err = <u64 as TryInto<Value>>::try_into(u64::MAX).unwrap_err();
    assert!(
        matches!(
            err,
            morphic::Error::OutOfRange {
                requested: u64::MAX,
                max,
            } if max == i64::MAX as u64
        ),
        "got {err:?}"
    );

    let just_over = i64::MAX as u64 + 1;
    assert_eq!(
        <u64 as TryInto<Value>>::try_into(just_over),
        Err(morphic::Error::OutOfRange {
            requested: just_over,
            max: i64::MAX as u64
        })
    );
    assert!(
        <u64 as TryInto<Value>>::try_into(just_over)
            .unwrap_err()
            .to_string()
            .contains("out of range")
    );
}

/// `From<u64> for Value` cast with `as i64`, so `u64::MAX` became `-1` with no
/// error. There is no trait-bound way to assert that an impl is *absent* from
/// inside a downstream crate, so this test compiles snippets against the built
/// `morphic` and requires the `u64` ones to fail: `Value::from(u64)` and
/// `u64.into()` must not resolve, while the same call on an `i64` must still
/// compile. That control case is what stops the test passing for the wrong
/// reason — a mislocated rlib would fail every snippet, not just these.
#[test]
fn there_is_no_infallible_path_from_u64_into_a_value() {
    let control = r#"
        fn main() {
            let _: morphic::Value = morphic::Value::from(1i64);
        }
    "#;
    let from_impl = r#"
        fn main() {
            let _: morphic::Value = morphic::Value::from(1u64);
        }
    "#;
    let into_trait = r#"
        fn main() {
            let v: morphic::Value = 1u64.into();
            let _ = v;
        }
    "#;
    // The value that used to be produced silently, named so the test reads as
    // the bug it is pinning down.
    assert_eq!(u64::MAX as i64, -1);

    compile_against_morphic("control_i64", control)
        .expect("the i64 control snippet must still compile");

    let diagnostics = compile_against_morphic("must_not_from", from_impl)
        .expect_err("Value::from(u64) must not exist");
    assert!(
        diagnostics.contains("E0277"),
        "expected a trait-bound failure, got:\n{diagnostics}"
    );

    let diagnostics = compile_against_morphic("must_not_into", into_trait)
        .expect_err("u64: Into<Value> must not hold");
    assert!(
        diagnostics.contains("E0277"),
        "expected a trait-bound failure, got:\n{diagnostics}"
    );
}

/// Compile `src` against the freshly built `morphic` rlib.
///
/// `Ok(())` when the snippet compiles, `Err(diagnostics)` when it does not.
///
/// The rlib is found next to this test binary, which is the directory cargo
/// linked it from, so a custom `CARGO_TARGET_DIR` or an explicit `--target`
/// needs no special handling.
fn compile_against_morphic(crate_name: &str, src: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let exe = std::env::current_exe().expect("test binary path");
    let deps = exe.parent().expect("deps directory").to_path_buf();

    let newest = |stem: &str| -> std::path::PathBuf {
        let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&deps)
            .expect("readable deps directory")
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                let name = path.file_name()?.to_str()?.to_owned();
                if !name.starts_with(&format!("lib{stem}-")) || !name.ends_with(".rlib") {
                    return None;
                }
                let modified = e.metadata().ok()?.modified().ok()?;
                Some((modified, path))
            })
            .collect();
        found.sort();
        found
            .pop()
            .unwrap_or_else(|| panic!("no lib{stem}-*.rlib in {}", deps.display()))
            .1
    };

    let out_dir = std::env::temp_dir().join(format!("morphic-compile-{crate_name}"));
    std::fs::create_dir_all(&out_dir).expect("temp out dir");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut child = std::process::Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-type=bin")
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--extern")
        .arg(format!("morphic={}", newest("morphic").display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rustc");

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(src.as_bytes())
        .expect("write snippet to rustc");

    let output = child.wait_with_output().expect("run rustc");
    let _ = std::fs::remove_dir_all(&out_dir);
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
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
