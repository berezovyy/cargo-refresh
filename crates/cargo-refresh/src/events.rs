//! JSONL events. Wall time and monotonic are seconds; phase marks are monotonic milliseconds.
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SavePath {
    Skip,
    Live,
    Restart,
    Stock,
    Error,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Notice,
    Sources,
    WorkerStart,
    MirrorEnd,
    Gate,
    Classify,
    BuildStart,
    BuildEnd,
    ManifestEnd,
    CandidateEnd,
    StockStart,
    StockEnd,
    ExtractStart,
    ExtractEnd,
    PatchSent,
    PatchAck,
    CommitEnd,
    TriggerEnd,
    SpawnEnd,
    FirstProbe,
}

#[derive(Debug, Serialize)]
pub struct SaveEvent {
    pub path: SavePath,
    pub reason: String,
    pub latency_ms: f64,
    pub phases: BTreeMap<Phase, f64>,
    pub revision: u64,
    pub files: Vec<PathBuf>,
}

#[derive(Serialize)]
struct Record<'a, T> {
    event: &'a str,
    time: f64,
    monotonic: f64,
    generation: u64,
    loadavg: String,
    available_bytes: u64,
    #[serde(flatten)]
    fields: T,
}

pub struct EventLog {
    writer: BufWriter<File>,
}

impl EventLog {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            writer: BufWriter::new(OpenOptions::new().create(true).append(true).open(path)?),
        })
    }

    pub fn event(&mut self, event: &str, generation: u64) -> io::Result<()> {
        self.write(event, generation, BTreeMap::<String, String>::new())
    }

    pub fn save(&mut self, generation: u64, save: &SaveEvent) -> io::Result<()> {
        self.write("save", generation, save)
    }

    pub fn detail<T: Serialize>(
        &mut self,
        event: &str,
        generation: u64,
        fields: T,
    ) -> io::Result<()> {
        self.write(event, generation, fields)
    }

    fn write<T: Serialize>(&mut self, event: &str, generation: u64, fields: T) -> io::Result<()> {
        let record = Record {
            event,
            time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs_f64(),
            monotonic: monotonic_seconds()?,
            generation,
            loadavg: fs::read_to_string("/proc/loadavg")?.trim().to_owned(),
            available_bytes: available_bytes()?,
            fields,
        };
        // Serialize before writing, so serialization failure cannot leave a partial line.
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes)?;
        self.writer.flush()
    }
}

pub fn available_bytes() -> io::Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|kib| kib.checked_mul(1024))
        .ok_or_else(|| io::Error::other("MemAvailable missing from /proc/meminfo"))
}

pub fn monotonic_seconds() -> io::Result<f64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: value is a valid writable timespec and CLOCK_MONOTONIC needs no other pointers.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value.tv_sec as f64 + value.tv_nsec as f64 / 1_000_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn jsonl_has_required_fields_and_append_is_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/events.jsonl");
        let mut log = EventLog::open(&path).unwrap();
        log.event("start", 0).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        for path in [
            SavePath::Skip,
            SavePath::Live,
            SavePath::Restart,
            SavePath::Stock,
            SavePath::Error,
            SavePath::Waiting,
        ] {
            log.save(
                3,
                &SaveEvent {
                    path,
                    reason: "test\nquoted \"value\"".into(),
                    latency_ms: 1.5,
                    phases: BTreeMap::from([
                        (Phase::Notice, monotonic_seconds().unwrap() * 1000.0),
                        (Phase::Sources, monotonic_seconds().unwrap() * 1000.0),
                    ]),
                    revision: 7,
                    files: vec!["src/main.rs".into()],
                },
            )
            .unwrap();
        }
        drop(log);
        EventLog::open(&path).unwrap().event("stop", 3).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        let rows: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 8);
        for row in &rows {
            for key in [
                "event",
                "time",
                "monotonic",
                "generation",
                "loadavg",
                "available_bytes",
            ] {
                assert!(row.get(key).is_some(), "missing {key}");
            }
            assert!(row["time"].as_f64().unwrap() > 0.0);
            assert!(row["monotonic"].as_f64().unwrap() > 0.0);
            assert!(row["available_bytes"].as_u64().unwrap() > 0);
            assert!(row["loadavg"].as_str().unwrap().split_whitespace().count() >= 3);
        }
        for (row, route) in rows[1..7]
            .iter()
            .zip(["skip", "live", "restart", "stock", "error", "waiting"])
        {
            assert_eq!(row["event"], "save");
            assert_eq!(row["path"], route);
            assert_eq!(row["generation"], 3);
            assert_eq!(row["revision"], 7);
            assert_eq!(row["latency_ms"], 1.5);
            assert!(row["reason"].is_string());
            assert!(row["phases"]["notice"].is_number());
            assert!(row["phases"]["sources"].is_number());
        }
    }

    #[test]
    fn phase_names_are_stable() {
        let phases = [
            Phase::Notice,
            Phase::Sources,
            Phase::Gate,
            Phase::Classify,
            Phase::BuildStart,
            Phase::BuildEnd,
            Phase::ExtractStart,
            Phase::ExtractEnd,
            Phase::PatchSent,
            Phase::PatchAck,
            Phase::SpawnEnd,
            Phase::FirstProbe,
        ];
        let expected = [
            "notice",
            "sources",
            "gate",
            "classify",
            "build_start",
            "build_end",
            "extract_start",
            "extract_end",
            "patch_sent",
            "patch_ack",
            "spawn_end",
            "first_probe",
        ];
        for (phase, name) in phases.into_iter().zip(expected) {
            assert_eq!(serde_json::to_value(phase).unwrap(), name);
        }
    }
}
