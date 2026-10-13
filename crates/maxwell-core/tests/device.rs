//! The device worker against a mock headset. Ports tests/test_device_persistence.py.

use maxwell_core::device::{Device, Progress, execute, failure};
use maxwell_core::dsp::RATES;
use maxwell_core::encoder::encode_bank;
use maxwell_core::hid::Transport;
use maxwell_core::{Error, Result};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

fn blob(preamp: f64) -> String {
    let mut raw = 4u32.to_le_bytes().to_vec();
    for (rate_id, rate) in RATES {
        let words = encode_bank(&[], preamp, f64::from(rate)).unwrap();
        raw.extend(rate_id.to_le_bytes());
        raw.extend((words.len() as u16).to_le_bytes());
        raw.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    }
    hex::encode(raw)
}

#[derive(Default)]
struct Log {
    opened: Vec<(bool, bool, Transport)>,
    saves: Vec<(u8, Vec<u8>)>,
    lives: usize,
}

struct Mock {
    states: VecDeque<Value>,
    space: bool,
    save: Option<Error>,
    live: Option<Error>,
    log: Rc<RefCell<Log>>,
}

impl Device for Mock {
    fn read_state(&mut self) -> Result<Value> {
        let state = self.states.pop_front().expect("unexpected read_state");
        if self.states.is_empty() {
            self.states.push_back(state.clone());
        }
        Ok(state)
    }

    fn has_storage_space(&mut self) -> Result<bool> {
        Ok(self.space)
    }

    fn save_coefficients(&mut self, slot: u8, coefficients: &[u8]) -> Result<Value> {
        self.log.borrow_mut().saves.push((slot, coefficients.to_vec()));
        match self.save.take() {
            Some(error) => Err(error),
            None => Ok(json!({"saved_verified": true})),
        }
    }

    fn apply_live(&mut self, _: &[u8]) -> Result<Value> {
        self.log.borrow_mut().lives += 1;
        match self.live.take() {
            Some(error) => Err(error),
            None => Ok(json!({"accepted": true, "ack_hex": "055b0300030e00"})),
        }
    }
}

struct Fixture {
    original: String,
    desired: String,
    state: Value,
    saved: Value,
}

impl Fixture {
    fn new() -> Self {
        let (original, desired) = (blob(0.0), blob(-3.0));
        let state = json!({"device": {"path": "test-dongle", "transport": "dongle"}, "index": 10, "slot": 3,
                           "raw_ui_hex": "00", "saved_coefficients_hex": original});
        let mut saved = state.clone();
        saved["saved_coefficients_hex"] = json!(desired);
        Fixture { original, desired, state, saved }
    }

    fn request(&self, operation: &str) -> Value {
        json!({"operation": operation, "transport": "dongle", "expected": self.state, "blob_hex": self.desired})
    }

    /// Run with a mock headset; returns the result, the progress and the call log.
    fn run(
        &self,
        operation: &str,
        states: Vec<Value>,
        setup: impl FnOnce(&mut Mock),
    ) -> (Result<Value>, Progress, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut mock = Some(Mock { states: states.into(), space: true, save: None, live: None, log: log.clone() });
        setup(mock.as_mut().unwrap());
        let mut open = |live, persistent, transport| -> Result<Box<dyn Device>> {
            log.borrow_mut().opened.push((live, persistent, transport));
            Ok(Box::new(mock.take().expect("opened twice")))
        };
        let mut progress = Progress::default();
        let result = execute(&self.request(operation), &mut open, &mut progress);
        (result, progress, log)
    }
}

#[test]
fn save_checks_snapshot_then_verifies_then_applies() {
    let f = Fixture::new();
    let (result, _, log) = f.run("save", vec![f.state.clone(), f.saved.clone()], |_| {});
    let result = result.unwrap();
    let log = log.borrow();
    assert_eq!(log.opened, vec![(true, true, Transport::Dongle)]);
    assert_eq!(log.saves, vec![(3, hex::decode(&f.desired).unwrap())]);
    assert_eq!(result["saved_verified"], true);
    assert_eq!(result["live_applied"], true);
    assert_eq!(result["state"]["saved_coefficients_hex"], json!(f.desired));
    let keys: Vec<_> = result.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["saved_verified", "state", "accepted", "ack_hex", "live_applied", "persistence_write_sent", "sha256"]
    );
}

#[test]
fn no_space_sends_no_profile_or_live_write() {
    let f = Fixture::new();
    let (result, progress, log) = f.run("save", vec![f.state.clone()], |mock| mock.space = false);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("insufficient"), "{error}");
    assert!(log.borrow().saves.is_empty());
    assert_eq!(log.borrow().lives, 0);
    assert!(!progress.write_started);
    assert!(failure(&error, &progress)["error"].as_str().unwrap().ends_with(" No EQ was sent."));
}

#[test]
fn readback_failure_keeps_persistent_uncertainty() {
    let f = Fixture::new();
    let (result, progress, log) =
        f.run("save", vec![f.state.clone()], |mock| mock.save = Some(Error::value("Readback mismatch")));
    assert!(result.is_err());
    assert!(progress.persistent_write_started);
    assert!(!progress.saved_verified);
    assert_eq!(log.borrow().lives, 0);
}

#[test]
fn live_failure_preserves_verified_saved_state() {
    let f = Fixture::new();
    let (result, progress, _) =
        f.run("save", vec![f.state.clone(), f.saved.clone()], |mock| mock.live = Some(Error::other("No live ACK")));
    let error = result.unwrap_err();
    assert!(progress.saved_verified);
    assert_eq!(progress.last_state, Some(f.saved.clone()));
    let report = failure(&error, &progress);
    assert_eq!(report["error"], "No live ACK");
    assert_eq!(report["state"], f.saved);
    assert_eq!(report["state_changed"], false);
}

#[test]
fn restore_can_repair_bad_coefficients_but_not_a_different_slot() {
    let f = Fixture::new();
    let mut damaged = f.state.clone();
    damaged["saved_coefficients_hex"] = json!("1122");
    let (result, _, _) = f.run("restore_saved", vec![damaged.clone(), f.saved.clone()], |_| {});
    result.unwrap();

    damaged["index"] = json!(9);
    damaged["slot"] = json!(2);
    let (result, progress, log) = f.run("restore_saved", vec![damaged], |_| {});
    let error = result.unwrap_err();
    assert!(matches!(error, Error::StateChanged(_)), "{error:?}");
    assert!(log.borrow().saves.is_empty());
    assert_eq!(failure(&error, &progress)["state_changed"], true);
}

#[test]
fn damaged_saved_coefficients_block_a_normal_save() {
    let f = Fixture::new();
    let mut damaged = f.state.clone();
    damaged["saved_coefficients_hex"] = json!("1122");
    let (result, _, log) = f.run("save", vec![damaged], |_| {});
    assert!(result.is_err());
    assert!(log.borrow().saves.is_empty());
}

#[test]
fn read_opens_without_write_permission() {
    let f = Fixture::new();
    let (result, _, log) = f.run("read", vec![f.state.clone()], |_| {});
    assert_eq!(result.unwrap(), f.state);
    assert_eq!(log.borrow().opened, vec![(false, false, Transport::Dongle)]);
}

#[test]
fn malformed_requests_are_refused_before_opening() {
    let f = Fixture::new();
    let mut open = |_, _, _| -> Result<Box<dyn Device>> { panic!("must not open the headset") };
    let mut progress = Progress::default();
    let mut bad_blob = f.request("apply");
    bad_blob["blob_hex"] = json!(f.original[..20]);
    let mut no_backup = f.request("apply");
    no_backup["expected"] = json!({});
    let mut bad_transport = f.request("apply");
    bad_transport["transport"] = json!("bluetooth");
    for request in [json!({"operation": "erase"}), bad_blob, no_backup, bad_transport] {
        assert!(execute(&request, &mut open, &mut progress).is_err(), "{request}");
    }
}
