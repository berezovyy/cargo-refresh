use refresh_guard::{
    Admission, AdmissionRule, Class, FileSnapshot, Guard, Reason, admit, classify,
};

fn file(path: &str, source: &str) -> FileSnapshot {
    FileSnapshot::new(path, source.as_bytes())
}

#[test]
fn admission_and_classification_share_analysis() {
    let before = [file("lib.rs", "fn n() -> u8 { 1 }")];
    let after = [file("lib.rs", "fn n() -> u8 { 2 }")];
    let mut guard = Guard::new();
    assert_eq!(guard.admit(&before, &after), Admission::Admit);
    assert_eq!(guard.cache_stats().misses, 2);
    let expected = guard.classify(&before, &after);
    assert!(matches!(expected, Class::LiveEligible { .. }));
    for _ in 0..10 {
        assert_eq!(guard.admit(&before, &after), Admission::Admit);
        assert_eq!(guard.classify(&before, &after), expected);
    }
    let stats = guard.cache_stats();
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 42);
    assert_eq!(stats.entries, 2);
}

#[test]
fn cache_key_contains_path_and_content() {
    let mut guard = Guard::new();
    for (path, source) in [
        ("a.rs", "fn n() {}"),
        ("b.rs", "fn n() {}"),
        ("a.rs", "fn m() {}"),
    ] {
        let snapshot = [file(path, source)];
        guard.classify(&snapshot, &snapshot);
    }
    assert_eq!(guard.cache_stats().misses, 3);
    assert_eq!(guard.cache_stats().entries, 3);
}

#[test]
fn malformed_source_is_cached_too() {
    let files = [file("lib.rs", "fn n( {")];
    let mut guard = Guard::new();
    for _ in 0..5 {
        assert!(matches!(
            guard.admit(&files, &files),
            Admission::Reject {
                rule: AdmissionRule::Parse,
                ..
            }
        ));
        assert!(matches!(
            guard.classify(&files, &files),
            Class::Stock {
                reason: Reason::Parse { .. }
            }
        ));
    }
    assert_eq!(guard.cache_stats().misses, 1);
}

#[test]
fn eviction_and_clear_recompute_without_changing_results() {
    let mut guard = Guard::with_cache_limits(1, 1024 * 1024);
    let a = [file("lib.rs", "fn n() -> u8 { 1 }")];
    let b = [file("lib.rs", "fn n() -> u8 { 2 }")];
    let first = guard.classify(&a, &b);
    assert_eq!(guard.cache_stats().entries, 1);
    let count = guard.cache_stats().misses;
    assert_eq!(guard.classify(&a, &b), first);
    assert!(guard.cache_stats().misses > count);
    guard.clear_cache();
    assert_eq!(guard.cache_stats().entries, 0);
    assert_eq!(guard.cache_stats().retained_bytes, 0);
    assert_eq!(guard.classify(&a, &b), first);
}

#[test]
fn frequently_reused_files_survive_history_eviction() {
    let mut guard = Guard::with_cache_limits(3, 1024 * 1024);
    let baseline = [file("stable.rs", "fn unchanged() {}")];
    for index in 0..100 {
        guard.classify(&baseline, &baseline);
        let current = [file("edited.rs", &format!("fn n() -> u8 {{ {index} }}"))];
        guard.classify(&current, &current);
    }
    assert_eq!(guard.cache_stats().misses, 101);
    assert_eq!(guard.cache_stats().entries, 3);
}

#[test]
fn cache_byte_budget_can_decline_oversized_entries() {
    let mut guard = Guard::with_cache_limits(128, 1);
    let files = [file("lib.rs", "fn n() {}")];
    let first = guard.classify(&files, &files);
    assert_eq!(guard.cache_stats().entries, 0);
    assert_eq!(guard.classify(&files, &files), first);
    assert_eq!(guard.cache_stats().misses, 4);
}

#[test]
fn bounded_history_under_many_saves() {
    let mut guard = Guard::with_cache_limits(4, 32 * 1024);
    for index in 0..100 {
        let files = [file("lib.rs", &format!("fn n() -> u8 {{ {index} }}"))];
        assert!(matches!(
            guard.classify(&files, &files),
            Class::LiveEligible { .. }
        ));
        assert!(guard.cache_stats().entries <= 4);
        assert!(guard.cache_stats().retained_bytes <= 32 * 1024);
    }
    assert_eq!(guard.cache_stats().misses, 100);
}

#[test]
fn duplicate_paths_fail_closed() {
    let files = [file("lib.rs", "fn n() {}"), file("lib.rs", "fn m() {}")];
    assert!(matches!(
        admit(&files, &files),
        Admission::Reject {
            rule: AdmissionRule::Parse,
            ..
        }
    ));
    assert!(matches!(
        classify(&files, &files),
        Class::Stock {
            reason: Reason::InvalidSnapshots { .. }
        }
    ));
}

#[test]
fn invalid_utf8_fails_closed() {
    let before = [file("lib.rs", "fn n() {}")];
    let after = [FileSnapshot::new("lib.rs", vec![0xff])];
    assert!(matches!(
        admit(&before, &after),
        Admission::Reject {
            rule: AdmissionRule::Parse,
            ..
        }
    ));
    assert!(matches!(
        classify(&before, &after),
        Class::Stock {
            reason: Reason::Parse { .. }
        }
    ));
}

#[test]
fn source_file_set_precedes_layout_and_parse() {
    let before = [file("lib.rs", "struct A;")];
    let after = [file("lib.rs", "fn broken( {"), file("new.rs", "struct B;")];
    assert!(matches!(
        classify(&before, &after),
        Class::Restart {
            reason: Reason::FileSet { .. }
        }
    ));
}

#[test]
fn default_api_matches_owned_guard() {
    let before = [file("lib.rs", "fn n() {}")];
    let after = [file("lib.rs", "fn n() { let x = 1; }")];
    let mut guard = Guard::new();
    assert_eq!(admit(&before, &after), guard.admit(&before, &after));
    assert_eq!(classify(&before, &after), guard.classify(&before, &after));
}

#[test]
fn cached_guard_can_move_between_controller_threads() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Guard>();
    let mut guard = Guard::new();
    let files = [file("lib.rs", "fn n() {}")];
    guard.classify(&files, &files);
    std::thread::spawn(move || {
        guard.classify(&files, &files);
        assert_eq!(guard.cache_stats().misses, 1);
    })
    .join()
    .unwrap();
}

#[test]
fn comment_edit_crossing_parallel_threshold_keeps_identical_policy() {
    let source = "mod a { fn f() {} } mod b { fn g() {} }";
    let before = [file("lib.rs", source)];
    let after = [file(
        "lib.rs",
        &(format!("// {}\n", "padding".repeat(1500)) + source),
    )];
    let mut guard = Guard::new();
    assert_eq!(guard.admit(&before, &after), Admission::Admit);
    assert_eq!(
        guard.classify(&before, &after),
        Class::LiveEligible {
            changed_fns: Vec::new()
        }
    );
}

#[test]
fn concurrent_large_file_requests_keep_session_results_separate() {
    let workers: Vec<_> = (0..6).map(|id| std::thread::spawn(move || {
        let prefix = format!("// {}\n", "padding".repeat(1500));
        let source = |value| format!("{prefix}mod a{id} {{ fn value() -> u32 {{ {value} }} }} mod b{id} {{ fn other() {{}} }}");
        let path = format!("session{id}.rs");
        let before = [file(&path, &source(id))];
        let after = [file(&path, &source(id + 1))];
        let mut guard = Guard::new();
        assert_eq!(guard.admit(&before, &after), Admission::Admit);
        let Class::LiveEligible { changed_fns } = guard.classify(&before, &after) else {
            panic!("unexpected class for session {id}");
        };
        assert_eq!(changed_fns.len(), 1);
        assert_eq!(changed_fns[0].name, format!("a{id}::value"));
        assert_eq!(changed_fns[0].path, std::path::Path::new(&path));
    })).collect();
    for worker in workers {
        worker.join().unwrap();
    }
}
