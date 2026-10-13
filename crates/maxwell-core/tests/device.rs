//! The device worker against a mock headset.

use maxwell_core::device::{Access, Device, Progress, execute, failure};
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
    opened: Vec<(Access, Transport)>,
    saves: Vec<(u8, Vec<u8>)>,
    lives: usize,
    selects: Vec<u8>,
}

struct Mock {
    states: VecDeque<Value>,
    slots: [Option<Vec<u8>>; 4],
    space: bool,
    save: Option<Error>,
    live: Option<Error>,
    select: Option<Error>,
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

    fn read_slot(&mut self, slot: u8) -> Result<Option<Vec<u8>>> {
        Ok(self.slots[usize::from(slot)].clone())
    }

    fn select_preset(&mut self, index: u8) -> Result<Value> {
        self.log.borrow_mut().selects.push(index);
        match self.select.take() {
            Some(error) => Err(error),
            None => Ok(json!({"select_ack_hex": "055b05000009000009"})),
        }
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

const SAVE: Access = Access { live: true, persistent: true, select: false };
const READ: Access = Access { live: false, persistent: false, select: false };

struct Fixture {
    original: String,
    desired: String,
    state: Value,
}

impl Fixture {
    fn new() -> Self {
        let (original, desired) = (blob(0.0), blob(-3.0));
        let state = json!({"device": {"path": "test-dongle", "transport": "dongle"}, "index": 10, "slot": 3,
                           "raw_ui_hex": "00", "saved_coefficients_hex": original, "transport": "dongle"});
        Fixture { original, desired, state }
    }

    /// The headset on custom preset `index` (7..10).
    fn on(&self, index: u64) -> Value {
        let mut state = self.state.clone();
        state["index"] = json!(index);
        state["slot"] = json!(index - 7);
        state
    }

    fn save(&self, slot: u8) -> Value {
        json!({"operation": "save", "slot": slot, "transport": "dongle", "blob_hex": self.desired})
    }

    /// Run with a mock headset; returns the result, the progress and the call log.
    fn run(
        &self,
        request: Value,
        states: Vec<Value>,
        setup: impl FnOnce(&mut Mock),
    ) -> (Result<Value>, Progress, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut mock = Some(Mock {
            states: states.into(),
            slots: [None, None, None, Some(hex::decode(&self.original).unwrap())],
            space: true,
            save: None,
            live: None,
            select: None,
            log: log.clone(),
        });
        setup(mock.as_mut().unwrap());
        let mut open = |access, transport| -> Result<Box<dyn Device>> {
            log.borrow_mut().opened.push((access, transport));
            Ok(Box::new(mock.take().expect("opened twice")))
        };
        let mut progress = Progress::default();
        let result = execute(&request, &mut open, &mut progress);
        (result, progress, log)
    }
}

#[test]
fn saving_to_the_active_preset_verifies_then_plays_it() {
    let f = Fixture::new();
    let (result, _, log) = f.run(f.save(3), vec![f.state.clone()], |_| {});
    let result = result.unwrap();
    let log = log.borrow();
    assert_eq!(log.opened, vec![(SAVE, Transport::Dongle)]);
    assert_eq!(log.saves, vec![(3, hex::decode(&f.desired).unwrap())]);
    assert_eq!(log.lives, 1);
    assert_eq!(result["saved_verified"], true);
    assert_eq!(result["live_applied"], true);
    assert_eq!(result["state"]["saved_coefficients_hex"], json!(f.desired));
}

#[test]
fn saving_to_another_preset_leaves_what_plays_alone() {
    let f = Fixture::new();
    let (result, _, log) = f.run(f.save(0), vec![f.state.clone()], |_| {});
    let result = result.unwrap();
    assert_eq!(log.borrow().saves, vec![(0, hex::decode(&f.desired).unwrap())]);
    assert_eq!(log.borrow().lives, 0);
    assert_eq!(result["live_applied"], false);
    assert_eq!(result["slot"], 0);
    assert_eq!(result["state"]["saved_coefficients_hex"], json!(f.original));
}

#[test]
fn a_built_in_preset_does_not_stop_a_save() {
    let f = Fixture::new();
    let built_in = json!({"device": {"path": "test-dongle"}, "index": 2, "slot": null, "raw_ui_hex": null});
    let (result, _, log) = f.run(f.save(1), vec![built_in], |_| {});
    assert_eq!(result.unwrap()["live_applied"], false);
    assert_eq!(log.borrow().saves.len(), 1);
}

#[test]
fn no_space_sends_no_profile_or_live_write() {
    let f = Fixture::new();
    let (result, progress, log) = f.run(f.save(3), vec![f.state.clone()], |mock| mock.space = false);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("insufficient"), "{error}");
    assert!(log.borrow().saves.is_empty());
    assert_eq!(log.borrow().lives, 0);
    assert!(!progress.write_started);
    assert!(failure(&error, &progress)["error"].as_str().unwrap().ends_with(" No EQ was sent."));
}

#[test]
fn readback_failure_sends_no_live_eq() {
    let f = Fixture::new();
    let (result, progress, log) =
        f.run(f.save(3), vec![f.state.clone()], |mock| mock.save = Some(Error::value("Readback mismatch")));
    assert!(result.is_err());
    assert!(progress.write_started);
    assert!(!progress.saved_verified);
    assert_eq!(log.borrow().lives, 0);
}

#[test]
fn live_failure_reports_the_verified_save() {
    let f = Fixture::new();
    let (result, progress, _) =
        f.run(f.save(3), vec![f.state.clone()], |mock| mock.live = Some(Error::other("No live ACK")));
    let error = result.unwrap_err();
    let report = failure(&error, &progress);
    assert_eq!(report["error"], "No live ACK");
    assert_eq!(report["write_started"], true);
    assert_eq!(report["saved_verified"], true);
}

#[test]
fn read_reports_what_every_custom_preset_stores() {
    let f = Fixture::new();
    let request = json!({"operation": "read", "transport": "dongle"});
    let (result, _, log) = f.run(request, vec![f.on(8)], |mock| mock.slots[0] = Some(vec![1, 2]));
    let state = result.unwrap();
    assert_eq!(log.borrow().opened, vec![(READ, Transport::Dongle)]);
    // Preset 2 is active, so its EQ comes from the state read itself.
    assert_eq!(state["slots"], json!(["0102", f.original, null, f.original]));
    assert_eq!(state["index"], 8);
}

#[test]
fn select_switches_presets_and_reads_the_result() {
    let f = Fixture::new();
    let request = json!({"operation": "select", "index": 8, "transport": "dongle"});
    let (result, progress, log) = f.run(request, vec![f.on(10), f.on(8)], |_| {});
    let result = result.unwrap();
    let log = log.borrow();
    assert_eq!(log.opened, vec![(Access { live: false, persistent: false, select: true }, Transport::Dongle)]);
    assert_eq!(log.selects, vec![8]);
    assert!(log.saves.is_empty() && log.lives == 0);
    assert!(progress.write_started);
    assert_eq!((&result["selected"], &result["state"]["index"]), (&json!(true), &json!(8)));
}

#[test]
fn selecting_the_active_preset_sends_nothing() {
    let f = Fixture::new();
    let (result, progress, log) = f.run(json!({"operation": "select", "index": 10}), vec![f.state.clone()], |_| {});
    assert_eq!(result.unwrap()["selected"], false);
    assert!(log.borrow().selects.is_empty());
    assert!(!progress.write_started);
}

#[test]
fn malformed_requests_are_refused_before_opening() {
    let f = Fixture::new();
    let mut open = |_, _| -> Result<Box<dyn Device>> { panic!("must not open the headset") };
    let mut progress = Progress::default();
    let mut bad_blob = f.save(3);
    bad_blob["blob_hex"] = json!(f.original[..20]);
    let mut bad_transport = f.save(3);
    bad_transport["transport"] = json!("bluetooth");
    let mut no_slot = f.save(3);
    no_slot["slot"] = json!(4);
    for request in [
        json!({"operation": "erase"}),
        json!({"operation": "apply", "blob_hex": f.desired}),
        json!({"operation": "restore_saved"}),
        json!({"operation": "select", "index": 3}),
        json!({"operation": "select"}),
        bad_blob,
        bad_transport,
        no_slot,
    ] {
        assert!(execute(&request, &mut open, &mut progress).is_err(), "{request}");
    }
}

mod automatic_connection {
    use maxwell_core::Error;
    use maxwell_core::hid::{BUSY, Transport, connect, requested};
    use serde_json::json;

    fn missing(transport: Transport) -> Error {
        Error::other(format!("Expected one original Maxwell PlayStation {} interface, found 0", transport.name()))
    }

    #[test]
    fn usb_c_wins_when_both_are_present() {
        let mut tried = Vec::new();
        let (_, used) = connect(None, |t| {
            tried.push(t);
            Ok(())
        })
        .unwrap();
        assert_eq!((used, tried), (Transport::Usb, vec![Transport::Usb]));
    }

    #[test]
    fn falls_back_to_the_dongle() {
        let (_, used) = connect(None, |t| if t == Transport::Usb { Err(missing(t)) } else { Ok(()) }).unwrap();
        assert_eq!(used, Transport::Dongle);
    }

    #[test]
    fn neither_present_says_how_to_connect() {
        let error = connect::<()>(None, |t| Err(missing(t))).unwrap_err().to_string();
        assert!(error.starts_with("No headset found."), "{error}");
    }

    #[test]
    fn a_busy_usb_c_interface_is_reported_not_skipped() {
        let mut tried = Vec::new();
        let error = connect::<()>(None, |t| {
            tried.push(t);
            Err(Error::other(BUSY))
        })
        .unwrap_err();
        assert_eq!((error.to_string(), tried), (BUSY.to_owned(), vec![Transport::Usb]));
    }

    #[test]
    fn a_chosen_connection_is_the_only_one_tried() {
        let error = connect::<()>(Some(Transport::Dongle), |t| Err(missing(t))).unwrap_err().to_string();
        assert!(error.contains("dongle interface, found 0"), "{error}");
        assert_eq!(requested(&json!({})).unwrap(), None);
        assert_eq!(requested(&json!({"transport": "auto"})).unwrap(), None);
        assert_eq!(requested(&json!({"transport": "usb"})).unwrap(), Some(Transport::Usb));
        assert!(requested(&json!({"transport": "bluetooth"})).is_err());
    }
}
