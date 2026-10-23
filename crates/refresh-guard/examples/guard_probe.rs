//! JSONL adapter for the retained Python equivalence/benchmark scripts.
use refresh_guard::{Admission, Class, FileSnapshot, Guard};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    time::Instant,
};

#[derive(Deserialize)]
struct Request {
    before: BTreeMap<String, String>,
    after: BTreeMap<String, String>,
    #[serde(default)]
    measure: Option<String>,
}

fn main() {
    let mut guard = Guard::new();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let request: Request = serde_json::from_str(&line.unwrap()).unwrap();
        let snapshots = |files: BTreeMap<String, String>| {
            files
                .into_iter()
                .map(|(path, text)| FileSnapshot::new(path, text.into_bytes()))
                .collect::<Vec<_>>()
        };
        let before = snapshots(request.before);
        let after = snapshots(request.after);
        if matches!(
            request.measure.as_deref(),
            Some("cold" | "analysis" | "classify")
        ) {
            guard.clear_cache();
        }
        if request.measure.as_deref() == Some("edit") {
            guard.clear_cache();
            guard.classify(&before, &before);
        }
        let start = Instant::now();
        let (admission, class, elapsed_ns) = if request.measure.as_deref() == Some("classify") {
            let class = guard.classify(&before, &after);
            let elapsed_ns = start.elapsed().as_nanos();
            // Verify admission too, but outside the classifier-only timer.
            (guard.admit(&before, &after), class, elapsed_ns)
        } else {
            let admission = guard.admit(&before, &after);
            let class = guard.classify(&before, &after);
            (admission, class, start.elapsed().as_nanos())
        };
        let (admission_code, detail) = match admission {
            Admission::Admit => ("admit", String::new()),
            Admission::Reject { rule, detail } => (rule.code(), detail),
        };
        let (class_code, reason, changed_fns) = match class {
            Class::LiveEligible { changed_fns } => (
                "live",
                "",
                changed_fns
                    .into_iter()
                    .map(|f| json!({ "path": f.path, "name": f.name }))
                    .collect::<Vec<_>>(),
            ),
            Class::Restart { reason } => ("restart", reason.code(), Vec::new()),
            Class::Stock { reason } => ("stock", reason.code(), Vec::new()),
        };
        writeln!(
            stdout,
            "{}",
            json!({ "admission": admission_code, "detail": detail,
            "class": class_code, "reason": reason, "changed_fns": changed_fns,
            "elapsed_ns": elapsed_ns })
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}
