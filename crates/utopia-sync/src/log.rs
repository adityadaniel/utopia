//! The sync log: one JSON line for each round that did something (§12.6).
//!
//! `<config>/logs/sync.jsonl`, shared by the app, `utopia sync` and the daemon, each line
//! tagged with the graph key and its source. It rotates at [`MAX_BYTES`] into
//! `sync.1.jsonl` and `sync.2.jsonl`, so it never holds more than three files.
//!
//! A line holds counts, page ids, peers, counters, cursors and error kinds. It never holds
//! page titles or text, payloads, tokens or passphrases (§15.6). A quiet round that ends in
//! the same state as the line before it writes nothing, so a daemon that polls every minute
//! does not fill the file with "nothing happened".

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use utopia_store::Store;

use crate::{FailureKind, Result, SyncReport, SyncStatus};

/// Rotate the log when it would grow past this many bytes.
pub const MAX_BYTES: u64 = 1024 * 1024;
const FILE: &str = "sync.jsonl";
const KEEP: usize = 2;

/// Where one process writes its sync log lines.
#[derive(Clone, Debug)]
pub struct SyncLog {
    dir: PathBuf,
    graph: String,
    source: &'static str,
    /// The state the last written line described, to skip repeats of a quiet round.
    last: Arc<Mutex<Option<String>>>,
}

impl SyncLog {
    /// `dir` is `<config>/logs`; `graph` is the graph key (`utopia_store::graph_key`), never
    /// the path; `source` is `app`, `cli` or `daemon`.
    pub fn new(dir: impl Into<PathBuf>, graph: impl Into<String>, source: &'static str) -> Self {
        Self {
            dir: dir.into(),
            graph: graph.into(),
            source,
            last: Arc::new(Mutex::new(None)),
        }
    }

    /// The file lines go to.
    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// The line for a round, or `None` when it would only repeat a quiet state.
    /// `destination` names where the round went (a relay URL, a folder), never a secret.
    /// Cheap: a
    /// few counts from the store, so it may run on the thread that owns the store; the
    /// write itself belongs off the UI thread ([`SyncLog::append`], rule 10).
    pub fn entry(
        &self,
        result: &Result<SyncReport>,
        store: &Store,
        destination: Option<&str>,
    ) -> Option<Value> {
        let status = SyncStatus::from_result(result, store);
        let outcome = match &status {
            SyncStatus::Synced { .. } => "synced",
            SyncStatus::Partial { .. } => "partial",
            SyncStatus::Offline { .. } => "offline",
            SyncStatus::LocalError { .. } => "local_error",
            SyncStatus::NeedsAuth => "needs_auth",
            SyncStatus::Unsupported => "unsupported",
        };
        let waiting: Vec<Value> = store
            .waiting_pages()
            .unwrap_or_default()
            .into_iter()
            .map(|w| {
                json!({
                    "page": w.page.as_str(),
                    "since": w.since,
                    "ranges": w.ranges.iter().map(|r| json!({
                        "peer": r.peer.to_string(), "start": r.start, "end": r.end,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let queued = store.outbox_count().unwrap_or(0);
        let quiet = match result {
            Ok(r) => {
                r.is_quiet()
                    && r.failed.is_empty()
                    && r.resent == 0
                    && r.blobs_up + r.blobs_down == 0
            }
            Err(_) => false,
        };
        // A quiet round is written only when its state differs from the last line's.
        let state = format!("{outcome}|{queued}|{}", Value::Array(waiting.clone()));
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if quiet && last.as_deref() == Some(state.as_str()) {
                return None;
            }
            if quiet && last.is_none() && outcome == "synced" && waiting.is_empty() {
                *last = Some(state);
                return None;
            }
            *last = Some(state);
        }
        let mut line = json!({
            "ts": store.now(),
            "graph": self.graph,
            "source": self.source,
            "destination": destination,
            "outcome": outcome,
            "queued": queued,
            "waiting": waiting,
        });
        match result {
            Ok(r) => {
                let failed: Vec<Value> = r
                    .failed
                    .iter()
                    .map(|f| {
                        json!({
                            "page": f.page.as_ref().map(|p| p.as_str()),
                            "kind": kind_text(&f.kind),
                        })
                    })
                    .collect();
                let extra = json!({
                    "pushed": r.pushed,
                    "duplicates": r.duplicates,
                    "pulled_pages": r.pulled_pages,
                    "imported": r.imported.len(),
                    "resent": r.resent,
                    "snapshots": r.snapshots,
                    "blobs_up": r.blobs_up,
                    "blobs_down": r.blobs_down,
                    "failed": failed,
                });
                merge(&mut line, extra);
            }
            Err(e) => merge(&mut line, json!({ "error": e.to_string() })),
        }
        Some(line)
    }

    /// Append one line, rotating first when the file would pass [`MAX_BYTES`]. Blocking
    /// file I/O: call it off the UI thread.
    pub fn append(&self, line: &Value) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let text = format!("{line}\n");
        let path = self.path();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size + text.len() as u64 > MAX_BYTES {
            rotate(&self.dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        f.write_all(text.as_bytes())
    }

    /// [`SyncLog::entry`] then [`SyncLog::append`], for callers that own no UI thread (the
    /// CLI and the daemon). A failed write is reported on stderr and never fails the sync.
    pub fn record(&self, result: &Result<SyncReport>, store: &Store, destination: Option<&str>) {
        if let Some(line) = self.entry(result, store, destination) {
            if let Err(e) = self.append(&line) {
                eprintln!("utopia-sync: cannot write {}: {e}", self.path().display());
            }
        }
    }
}

fn kind_text(k: &FailureKind) -> String {
    match k {
        FailureKind::Unauthorized => "unauthorized".into(),
        FailureKind::ProtocolVersion(v) => format!("protocol version {v}"),
        FailureKind::Transport(m) => format!("transport: {m}"),
        FailureKind::Store(m) => format!("store: {m}"),
    }
}

fn merge(into: &mut Value, extra: Value) {
    if let (Value::Object(a), Value::Object(b)) = (into, extra) {
        a.extend(b);
    }
}

/// `sync.jsonl` → `sync.1.jsonl` → `sync.2.jsonl`; the oldest is dropped.
fn rotate(dir: &Path) -> std::io::Result<()> {
    let name = |i: usize| {
        if i == 0 {
            dir.join(FILE)
        } else {
            dir.join(format!("sync.{i}.jsonl"))
        }
    };
    let _ = std::fs::remove_file(name(KEEP));
    for i in (0..KEEP).rev() {
        if name(i).exists() {
            std::fs::rename(name(i), name(i + 1))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_round_is_written_once_and_a_busy_one_always() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let log = SyncLog::new(dir.path(), "key", "cli");
        let quiet = Ok(SyncReport::default());
        assert!(log.entry(&quiet, &store, None).is_none(), "nothing to say");
        let busy = Ok(SyncReport {
            pushed: 2,
            ..Default::default()
        });
        let line = log.entry(&busy, &store, None).expect("a round that moved");
        assert_eq!(line["pushed"], 2);
        assert_eq!(line["outcome"], "synced");
        assert!(
            log.entry(&quiet, &store, None).is_none(),
            "a quiet round in the state the last line described says nothing new"
        );
        let offline: Result<SyncReport> = Err(crate::SyncError::Transport("refused".into()));
        let line = log.entry(&offline, &store, None).unwrap();
        assert_eq!(line["outcome"], "offline");
        assert_eq!(line["error"], "transport: refused");
        assert!(
            log.entry(&quiet, &store, None).is_some(),
            "back online is a change of state"
        );
    }

    #[test]
    fn the_log_rotates_and_keeps_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let log = SyncLog::new(dir.path(), "key", "cli");
        let big = json!({ "pad": "x".repeat(400 * 1024) });
        for _ in 0..12 {
            log.append(&big).unwrap();
        }
        let mut files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, vec!["sync.1.jsonl", "sync.2.jsonl", "sync.jsonl"]);
        for f in files {
            let len = std::fs::metadata(dir.path().join(f)).unwrap().len();
            assert!(len <= MAX_BYTES, "{len}");
        }
    }
}
