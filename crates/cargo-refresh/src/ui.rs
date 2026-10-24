use crate::events::{SaveEvent, SavePath};
pub fn save(event: &SaveEvent, verbose: u8) {
    let (symbol, label) = match event.path {
        SavePath::Live => ("⚡", "live"),
        SavePath::Restart => ("↻", "restart"),
        SavePath::Stock => ("⏳", "cargo"),
        SavePath::Error => ("✗", "error"),
        SavePath::Waiting => ("⏳", "waiting"),
        SavePath::Skip if event.reason == "no effect on this build" => {
            ("·", "no effect on this build")
        }
        SavePath::Skip => ("·", "unchanged"),
    };
    eprintln!(
        "{symbol} {label}{}",
        if event.reason.is_empty()
            || (matches!(event.path, SavePath::Skip) && event.reason == label)
        {
            String::new()
        } else {
            format!(" — {}", event.reason.lines().next().unwrap_or_default())
        }
    );
    if verbose > 0 {
        eprintln!(
            "  revision {}: {:.1} ms; {:?}",
            event.revision, event.latency_ms, event.phases
        );
    }
}
