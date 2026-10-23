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
            "async fn a() { 1; } fn b() { println!(\"old\"); }",
            "async fn a() { 2; } fn b() { println!(\"new\"); }",
        ),
    ] {
        assert!(!matches!(classify(old, new).0, Class::LiveEligible { .. }));
    }
}
