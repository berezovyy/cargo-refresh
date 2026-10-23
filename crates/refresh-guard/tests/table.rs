use refresh_guard::{Admission, Class, FileSnapshot, Guard};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Deserialize)]
#[serde(untagged)]
enum Source {
    Text(String),
    File { file: String },
}

#[derive(Deserialize)]
struct Expected {
    admission: String,
    class: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    changed_fns: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    before: BTreeMap<String, Source>,
    after: BTreeMap<String, Source>,
    expected: Expected,
}

fn run_suite(name: &str, min_cases: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(root.join(name)).unwrap()).unwrap();
    assert!(
        cases.len() >= min_cases,
        "{name} only has {} cases",
        cases.len()
    );
    let mut ids = BTreeSet::new();
    for case in cases {
        assert!(ids.insert(case.id.clone()), "duplicate case {}", case.id);
        let snapshots = |files: BTreeMap<String, Source>| {
            files
                .into_iter()
                .map(|(path, source)| {
                    let bytes = match source {
                        Source::Text(text) => text.into_bytes(),
                        Source::File { file } => fs::read(root.join(file)).unwrap(),
                    };
                    FileSnapshot::new(path, bytes)
                })
                .collect::<Vec<_>>()
        };
        let before = snapshots(case.before);
        let after = snapshots(case.after);
        let mut guard = Guard::new();
        let admission = guard.admit(&before, &after);
        let admission_code = match &admission {
            Admission::Admit => "admit",
            Admission::Reject { rule, .. } => rule.code(),
        };
        assert_eq!(
            admission_code, case.expected.admission,
            "{}: {admission:?}",
            case.id
        );
        let class = guard.classify(&before, &after);
        let (class_code, reason) = match &class {
            Class::LiveEligible { .. } => ("live", ""),
            Class::Restart { reason } => ("restart", reason.code()),
            Class::Stock { reason } => ("stock", reason.code()),
        };
        assert_eq!(class_code, case.expected.class, "{}: {class:?}", case.id);
        if let Some(expected) = case.expected.reason {
            assert_eq!(reason, expected, "{}", case.id);
        }
        if let Some(expected) = case.expected.changed_fns {
            let Class::LiveEligible { changed_fns } = &class else {
                panic!("{}: {class:?}", case.id)
            };
            assert_eq!(
                changed_fns
                    .iter()
                    .map(|f| f.name.clone())
                    .collect::<Vec<_>>(),
                expected,
                "{}",
                case.id
            );
        }
        let misses = guard.cache_stats().misses;
        assert_eq!(
            guard.admit(&before, &after),
            admission,
            "{} cached admission",
            case.id
        );
        assert_eq!(
            guard.classify(&before, &after),
            class,
            "{} cached class",
            case.id
        );
        assert_eq!(
            guard.cache_stats().misses,
            misses,
            "{} unexpectedly reanalyzed",
            case.id
        );
    }
}

#[test]
fn web_session_64() {
    run_suite("web-session.json", 64);
}

#[test]
fn fixture_scenarios() {
    run_suite("fixtures.json", 30);
}

#[test]
fn async_exp055() {
    run_suite("async-exp055.json", 25);
}

#[test]
fn policy_boundaries() {
    run_suite("boundaries.json", 60);
}
