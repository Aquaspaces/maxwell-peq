//! The raw HID reports a device worker exchanges with the headset, for the
//! traffic panel.
//!
//! The worker prints one JSON line per report on stderr (see [`trace`]); the
//! app reads them as they arrive and keeps the most recent [`CAPACITY`].

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CAPACITY: usize = 2000;

static TRACING: AtomicBool = AtomicBool::new(false);

/// Make [`trace`] print. Only the `--worker device` process turns it on.
pub fn enable() {
    TRACING.store(true, Ordering::Relaxed);
}

/// Print one report, exactly as written to or read from the HID collection.
pub fn trace(direction: &str, report: &[u8]) {
    if TRACING.load(Ordering::Relaxed) {
        let line = json!({"trace": direction, "hex": hex::encode(report)});
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
}

#[derive(Default)]
struct Log {
    next: u64,
    entries: VecDeque<Value>,
}

/// What the workers sent and received in this run, oldest first.
#[derive(Default)]
pub struct Traffic {
    log: Mutex<Log>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl Traffic {
    /// Add `entry` (an object) with its sequence number and arrival time.
    pub fn push(&self, mut entry: Value) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        entry["seq"] = json!(log.next);
        entry["at"] = json!(now_ms());
        log.next += 1;
        if log.entries.len() == CAPACITY {
            log.entries.pop_front();
        }
        log.entries.push_back(entry);
    }

    /// A worker's stderr line: a traced report, or anything else it printed.
    pub fn push_line(&self, line: &str) {
        let parsed = serde_json::from_str::<Value>(line).ok();
        match parsed.as_ref().and_then(|v| Some((v.get("trace")?.as_str()?, v.get("hex")?.as_str()?))) {
            Some((direction @ ("out" | "in"), hex)) => self.push(json!({"kind": direction, "hex": hex})),
            _ if !line.trim().is_empty() => self.push(json!({"kind": "note", "text": line.trim()})),
            _ => {}
        }
    }

    /// Entries numbered `since` or later, and the number to ask from next time.
    pub fn since(&self, since: u64) -> Value {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let entries: Vec<_> =
            log.entries.iter().filter(|entry| entry["seq"].as_u64().is_some_and(|seq| seq >= since)).cloned().collect();
        json!({"entries": entries, "next": log.next})
    }

    pub fn clear(&self) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).entries.clear();
    }
}
