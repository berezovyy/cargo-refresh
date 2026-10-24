use refresh_guard::{Class, FileSnapshot, Guard, Reason};
fn classify(old: &str, new: &str) -> (Class, Vec<refresh_guard::ChangedFn>) {
    let mut guard = Guard::default();
    guard.classify_with_async_certificates(
        &[FileSnapshot::new("src/lib.rs", old)],
        &[FileSnapshot::new("src/lib.rs", new)],
    )
}
#[test]
fn strict_string_guard_is_only_deferred_with_complete_owner_list() {
    let old = "async fn a() { tick().await; println!(\"old\"); } async fn b() { tick().await; println!(\"old\"); }";
    let new = old.replace("old", "new");
    assert!(matches!(
        Guard::default().classify(
            &[FileSnapshot::new("src/lib.rs", old)],
            &[FileSnapshot::new("src/lib.rs", new.as_str())]
        ),
        Class::Restart {
            reason: Reason::Async { .. }
        }
    ));
    let (class, owners) = classify(old, &new);
    assert!(matches!(class, Class::LiveEligible { .. }));
    assert_eq!(
        owners.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        vec!["::a", "::b"]
    );
}
#[test]
fn certificates_never_relax_structure_signatures_or_nonasync_hazards() {
    for (old, new) in [
        ("async fn a() { 1; }", "async fn a(x: u64) { 2; }"),
        (
            "struct A(u8); async fn a() { 1; }",
            "struct A(u64); async fn a() { 2; }",
        ),
        (
            "async fn a() { 1; } fn b() { opaque!(\"old\"); }",
            "async fn a() { 2; } fn b() { opaque!(\"new\"); }",
        ),
    ] {
        assert!(!matches!(classify(old, new).0, Class::LiveEligible { .. }));
    }
}

#[test]
fn unique_async_methods_require_declaration_location_coverage() {
    let old = "struct Service;\nimpl ExternalTrait for Service {\n #[instrument(skip(self))]\n async fn call(&self) { tick().await; println!(\"old\"); }\n}";
    let new = old.replace("old", "new");
    let (class, owners) = classify(old, &new);
    assert!(matches!(class, Class::LiveEligible { .. }), "{class:?}");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].name, "@line:3-4::call");
    assert!(matches!(
        Guard::default().classify(
            &[FileSnapshot::new("src/lib.rs", old)],
            &[FileSnapshot::new("src/lib.rs", new.as_str())]
        ),
        Class::Restart { .. }
    ));
}
#[test]
fn ambiguous_methods_and_method_header_changes_keep_restart() {
    let old = "struct A; struct B; impl A { async fn call(&self) { 1; } } impl B { async fn call(&self) { 2; } }";
    assert!(!matches!(
        classify(old, &old.replace("1;", "3;")).0,
        Class::LiveEligible { .. }
    ));
    let old = "struct A; impl A { async fn call(&self) { 1; } }";
    assert!(!matches!(
        classify(old, &old.replace("&self", "&mut self")).0,
        Class::LiveEligible { .. }
    ));
    assert!(!matches!(
        classify(old, &old.replace("struct A;", "struct A(u64);")).0,
        Class::LiveEligible { .. }
    ));
}

#[test]
fn attribute_coverage_is_bounded_by_the_unique_method_header() {
    let old = "struct A;\nimpl A {\n #[instrument(\n fields(values = [1, 2])\n )]\n pub(crate) async fn call(&self) { 1; }\n}";
    let (class, owners) = classify(old, &old.replace("1;", "3;"));
    assert!(matches!(class, Class::LiveEligible { .. }));
    assert_eq!(owners[0].name, "@line:3-6::call");
}

#[test]
fn certificate_deferral_preserves_the_closed_expression_macro_policy() {
    let old = "async fn a() { 1; } fn b() { println!(\"old\"); }";
    let new = old.replace("1;", "2;").replace("old", "new");
    let (class, owners) = classify(old, &new);
    assert!(matches!(class, Class::LiveEligible { .. }), "{class:?}");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].name, "::a");

    for hazard in [
        "opaque!(1)",
        "println!(\"{}\", || 1)",
        "vec![async { fn local() {} 1 }]",
    ] {
        let before = format!("async fn a() {{ 1; }} fn b() {{ {hazard}; }}");
        let after = before.replace('1', "2");
        assert!(!matches!(
            classify(&before, &after).0,
            Class::LiveEligible { .. }
        ));
    }
}

#[test]
fn vec_argument_coroutines_require_complete_compiler_coverage() {
    let old = "fn evaluate() { println!(\"{:?}\", vec![block_on(async { 1i64 }); 3]); }";
    let new = "fn evaluate() { println!(\"{:?}\", vec![block_on(async { 2i64 }), block_on(async { 3i64 })]); }";
    assert!(matches!(
        Guard::default().classify(
            &[FileSnapshot::new("src/lib.rs", old)],
            &[FileSnapshot::new("src/lib.rs", new)]
        ),
        Class::Restart { .. }
    ));
    let (class, owners) = classify(old, new);
    assert!(matches!(class, Class::LiveEligible { .. }), "{class:?}");
    assert_eq!(
        owners.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        vec!["::evaluate"]
    );

    // The additional pass only relaxes parsed standard vec arguments. All other
    // hazards in either revision still prevent certificate deferral.
    for (old, new) in [
        (
            "fn f() { block_on(async { 1 }); }",
            "fn f() { block_on(async { 2 }); }",
        ),
        (
            "fn f() { println!(\"{}\", block_on(async { 1 })); }",
            "fn f() { println!(\"{}\", block_on(async { 2 })); }",
        ),
        (
            "fn f() { vec![async { opaque!(1) }]; }",
            "fn f() { vec![async { opaque!(2) }]; }",
        ),
        (
            "fn f() { vec![async { (|| 1)() }]; }",
            "fn f() { vec![async { (|| 2)() }]; }",
        ),
        (
            "macro_rules! vec { ($e:expr) => { $e }; } fn f() { vec!(async { 1 }); }",
            "macro_rules! vec { ($e:expr) => { $e }; } fn f() { vec!(async { 2 }); }",
        ),
    ] {
        assert!(
            !matches!(classify(old, new).0, Class::LiveEligible { .. }),
            "{old}"
        );
    }
}
