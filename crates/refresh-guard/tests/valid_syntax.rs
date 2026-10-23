//! Valid syntax regressions: parser normalization must never panic.
use refresh_guard::{Class, FileSnapshot, Guard};

#[test]
fn valid_rust_syntax_survives_admission_and_both_classifiers() {
    let cases = [
        "use std::io::Write as _; fn f() -> i32 { 1 }",
        "use std::io::{Read as _, Write as _}; fn f() -> i32 { 1 }",
        "use std::io::Write as r#type; fn r#match() -> i32 { 1 }",
        "fn f(#[allow(unused)] r#type: i32) -> i32 { 1 }",
        "fn f(_: impl Copy) -> i32 { 1 }",
        "fn f() -> i32 { macro_rules! inner { () => { 1 }; } inner!() }",
        "fn f() -> i32 { let _ = r###\"raw string\"###; 1 }",
        "fn f<'a>(_: &'a str) -> i32 { 'r#label: loop { break 'r#label 1; } }",
        "#[allow(dead_code)] fn f() -> i32 { 1 }",
        "struct S; impl S { async fn f(#[allow(unused)] _: impl Copy) -> i32 { 1 } }",
    ];
    for before in cases {
        let after = before.replace("1", "2");
        let before = [FileSnapshot::new("main.rs", before.as_bytes())];
        let after = [FileSnapshot::new("main.rs", after.as_bytes())];
        let mut guard = Guard::new();
        let _ = guard.admit(&before, &after);
        let class = guard.classify(&before, &after);
        assert!(!matches!(class, Class::Stock { .. }), "{class:?}");
        let _ = guard.classify_with_async_certificates(&before, &after);
    }
}

#[test]
fn underscore_import_does_not_shadow_standard_macros() {
    let before = [FileSnapshot::new(
        "main.rs",
        b"use std::io::Write as _; fn f() { println!(\"one\"); }".to_vec(),
    )];
    let after = [FileSnapshot::new(
        "main.rs",
        b"use std::io::Write as _; fn f() { println!(\"two\"); }".to_vec(),
    )];
    assert!(matches!(
        Guard::new().classify(&before, &after),
        Class::LiveEligible { .. }
    ));
}

#[test]
fn raw_macro_import_alias_still_shadows_the_allowlist() {
    let before = [FileSnapshot::new(
        "main.rs",
        b"use custom::mac as r#println; fn f() { println!(\"one\"); }".to_vec(),
    )];
    let after = [FileSnapshot::new(
        "main.rs",
        b"use custom::mac as r#println; fn f() { println!(\"two\"); }".to_vec(),
    )];
    assert!(matches!(
        Guard::new().classify(&before, &after),
        Class::Restart { .. }
    ));
}

#[test]
fn unqualified_ast_stringification_is_found_through_wrappers() {
    use refresh_guard::requires_stock_stringification as blocked;
    for source in [
        "fn f() { assert!(x+y==0); }",
        "fn f() { std::assert!(true); }",
        "fn f() { core::debug_assert!(true); }",
        "macro_rules! wrap { ($e:expr) => { assert!($e) } }",
        "fn f() { r#assert!(true); }",
        "use std::assert as renamed; fn f() { renamed!(false); }",
        "use core::{debug_assert as renamed}; fn f() { renamed!(false); }",
    ] {
        assert!(blocked(source.as_bytes()), "{source}");
    }
    for source in [
        "fn f() { stringify!(x+y); dbg!(x+y); assert_eq!(1, 1); }",
        "fn f() { let _ = \"assert!(x)\"; /* debug_assert!(false) */ }",
        "fn f() { assert_ne!(1, 2); debug_assert_eq!(1, 1); }",
    ] {
        assert!(!blocked(source.as_bytes()), "{source}");
    }
}

#[test]
fn async_macro_arguments_are_distinct_from_outer_async_and_token_text() {
    use refresh_guard::has_async_macro_arguments as needs_stock;
    for source in [
        "fn f() { vec![block_on(async { 1 }); 3]; }",
        "fn f() { println!(\"{:?}\", vec![block_on(async { 1 }), block_on(async { 2 })]); }",
        "fn f() { std::vec!{block_on(async move { 1 })}; }",
    ] {
        assert!(needs_stock(source.as_bytes()), "{source}");
    }
    for source in [
        "fn f() { block_on(async { println!(\"x\"); }); }",
        "fn f() { println!(\"async\"); /* async */ }",
        "fn f() { println!(\"{}\", r#async); }",
    ] {
        assert!(!needs_stock(source.as_bytes()), "{source}");
    }
}
