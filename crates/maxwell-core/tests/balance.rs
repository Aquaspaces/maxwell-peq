//! Channel balance and the source profile: framing, the allowlist, the HID
//! session's write-then-verify, the worker, and the app rules. Nothing opens a
//! headset.

use maxwell_core::app::App;
use maxwell_core::audeze::AudezeWatch;
use maxwell_core::balance::{Progress, Settings, execute, failure};
use maxwell_core::hid::{Interface, ReportIo, Session, Transport};
use maxwell_core::protocol::*;
use maxwell_core::worker::Worker;
use maxwell_core::{Error, Result};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn response(command: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x05, 0x5B];
    packet.extend(((payload.len() + 2) as u16).to_le_bytes());
    packet.extend(command.to_le_bytes());
    packet.extend(payload);
    packet
}

fn input_report(data: &[u8]) -> Vec<u8> {
    let mut report = vec![7, data.len() as u8, 0x80];
    report.extend(data);
    report.resize(62, 0);
    report
}

fn empty() -> Vec<u8> {
    [&[7, 0, 0][..], &[0; 59]].concat()
}

fn gain_reply(sub: u8, value: u8) -> Vec<u8> {
    response(0x0901, &[sub, 0, 0, value])
}

#[test]
fn setting_vectors_match_the_research_notes() {
    // PROTOCOL.md: "06 08 80 05 5A 04 00 01 09 2A" and "06 09 80 05 5A 05 00 00 09 2F 00 0A".
    let read = read_setting_command(RIGHT_GAIN).unwrap();
    assert_eq!(hex::encode(&read), "055a040001092a00");
    let write = write_setting_command(SOURCE_PROFILE, USB_C_PROFILE).unwrap();
    assert_eq!(hex::encode(&write), "055a050000092f000a");
    let reports = output_reports(&read, true, 62, false, false).unwrap();
    assert_eq!(hex::encode(&reports[0][..11]), "060880055a040001092a00");
    // "05 5B 06 00 01 09 2F 00 00 <state>"
    assert_eq!(parse_setting(&hex::decode("055b060001092f000000").unwrap(), SOURCE_PROFILE).unwrap(), 0);
}

#[test]
fn only_gains_and_the_source_profile_are_settable() {
    for value in [0, 100, 141, 149, 150] {
        write_setting_command(LEFT_GAIN, value).unwrap();
        validate_setting_write(&write_setting_command(RIGHT_GAIN, value).unwrap()).unwrap();
    }
    for value in [136, 142, 151, 255] {
        assert!(write_setting_command(LEFT_GAIN, value).is_err(), "{value}");
    }
    assert!(write_setting_command(SOURCE_PROFILE, 5).is_err());
    // Sub 0x00 would switch the active preset; 0x28 is master gain.
    for sub in [0x00, 0x28, 0x2B] {
        assert!(write_setting_command(sub, 100).is_err(), "{sub:#x}");
        let raw = [&[0x05, 0x5A, 0x05, 0x00, 0x00, 0x09][..], &[sub, 0, 100]].concat();
        assert!(validate_setting_write(&raw).is_err(), "{sub:#x}");
    }
    let reserved = hex::decode("055a05000009290088").unwrap();
    assert!(validate_setting_write(&reserved).is_err());
    // EQ permissions never cover settings writes; reads need none.
    let write = write_setting_command(LEFT_GAIN, 140).unwrap();
    assert!(validate_request(&write, true, true).is_err());
    for sub in [LEFT_GAIN, RIGHT_GAIN, SOURCE_PROFILE] {
        validate_request(&read_setting_command(sub).unwrap(), false, false).unwrap();
    }
    assert!(read_setting_command(0x28).is_err());
}

#[test]
fn setting_replies_must_name_the_setting() {
    assert_eq!(parse_setting(&gain_reply(LEFT_GAIN, 141), LEFT_GAIN).unwrap(), 141);
    assert!(parse_setting(&gain_reply(RIGHT_GAIN, 141), LEFT_GAIN).is_err());
    assert!(parse_setting(&response(0x0901, &[LEFT_GAIN, 0, 141]), LEFT_GAIN).is_err());
    assert_eq!(parse_setting(&gain_reply(0, 7), 0).unwrap(), 7);
}

/// A fake headset: answers each written report from a table of RACE replies.
struct Headset {
    gains: HashMap<u8, u8>,
    /// Values to answer before the real one (stale reads), per setting.
    stale: HashMap<u8, VecDeque<u8>>,
    /// When false, writes are acknowledged but not kept.
    keeps: bool,
    /// A reply for some other request that lands just before the real one.
    stray: Option<Vec<u8>>,
    queue: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl Headset {
    fn new(left: u8, right: u8, source: u8) -> Self {
        Headset {
            gains: HashMap::from([(LEFT_GAIN, left), (RIGHT_GAIN, right), (SOURCE_PROFILE, source)]),
            stale: HashMap::new(),
            keeps: true,
            stray: None,
            queue: VecDeque::new(),
            writes: Vec::new(),
        }
    }
}

impl ReportIo for Headset {
    fn input(&mut self) -> Result<Vec<u8>> {
        Ok(self.queue.pop_front().unwrap_or_else(empty))
    }

    fn write(&mut self, report: &[u8]) -> Result<bool> {
        self.writes.push(report.to_vec());
        let packet = &report[3..3 + usize::from(report[1])];
        let (command, payload) = check_packet(packet, 0x5A).unwrap();
        let sub = payload[0];
        let reply = match command {
            0x0901 => {
                let value = self.stale.get_mut(&sub).and_then(VecDeque::pop_front).unwrap_or(self.gains[&sub]);
                gain_reply(sub, value)
            }
            0x0900 => {
                if self.keeps {
                    self.gains.insert(sub, payload[2]);
                }
                response(0x0900, &[0, sub, 0])
            }
            _ => panic!("unexpected command {command:#06x}"),
        };
        if let Some(stray) = self.stray.take() {
            self.queue.push_back(input_report(&stray));
        }
        self.queue.push_back(input_report(&reply));
        Ok(true)
    }
}

fn session(headset: Headset, allow_settings: bool) -> Session<Headset> {
    let interface = Interface { path: "test-hid-path".into(), input_size: 62, output_size: 62, device_version: 0x0100 };
    let mut session = Session::new(headset, Transport::Usb, interface, false, false);
    session.allow_settings = allow_settings;
    session.sleep = |_| {};
    session
}

fn commands(headset: &Headset) -> Vec<String> {
    headset.writes.iter().map(|r| hex::encode(&r[3..3 + usize::from(r[1])])).collect()
}

#[test]
fn reads_wait_for_two_matching_values() {
    let mut headset = Headset::new(141, 149, 0x0A);
    headset.stale.insert(LEFT_GAIN, VecDeque::from([7, 141]));
    let mut session = session(headset, false);
    assert_eq!(session.read_setting(LEFT_GAIN).unwrap(), 141);
    assert_eq!(session.io.writes.len(), 3, "7, then 141 twice");
}

#[test]
fn a_late_reply_for_another_setting_is_ignored() {
    let mut headset = Headset::new(141, 149, 0);
    headset.stray = Some(gain_reply(RIGHT_GAIN, 149));
    let mut session = session(headset, false);
    let reply = session.transact(&read_setting_command(LEFT_GAIN).unwrap(), None).unwrap();
    assert_eq!(parse_setting(&reply, LEFT_GAIN).unwrap(), 141);
}

#[test]
fn write_is_sent_once_and_verified_by_reading_back() {
    let mut session = session(Headset::new(141, 149, 0x0A), true);
    let reply = session.write_setting(RIGHT_GAIN, 144).unwrap();
    assert_eq!(reply, Some(response(0x0900, &[0, RIGHT_GAIN, 0])));
    let sent = commands(&session.io);
    assert_eq!(sent[0], "055a050000092a0090");
    assert!(sent[1..].iter().all(|c| c == "055a040001092a00"), "{sent:?}");
    assert_eq!(session.io.gains[&RIGHT_GAIN], 144);
}

#[test]
fn a_write_the_headset_drops_is_an_error() {
    let mut headset = Headset::new(141, 149, 0);
    headset.keeps = false;
    let mut session = session(headset, true);
    let error = session.write_setting(LEFT_GAIN, 140).unwrap_err().to_string();
    assert!(error.contains("did not keep"), "{error}");
    assert_eq!(commands(&session.io).iter().filter(|c| c.starts_with("055a050000")).count(), 1, "no blind retries");
}

#[test]
fn writes_need_the_settings_permission() {
    let mut session = session(Headset::new(141, 149, 0), false);
    assert!(session.write_setting(LEFT_GAIN, 140).is_err());
    assert!(session.transact(&write_setting_command(LEFT_GAIN, 140).unwrap(), None).is_err());
    assert!(session.io.writes.is_empty());
    // A settings session still cannot send EQ.
    let mut session = self::session(Headset::new(141, 149, 0), true);
    assert!(session.transact(&live_command(&[1]).unwrap(), None).is_err());
    assert!(session.io.writes.is_empty());
}

/// The worker against a mock that records what it was asked to do.
#[derive(Default)]
struct Log {
    opened: Vec<(bool, Transport)>,
    writes: Vec<(u8, u8)>,
}

struct Mock {
    values: HashMap<u8, u8>,
    fail_write: Option<u8>,
    log: Rc<RefCell<Log>>,
}

impl Settings for Mock {
    fn read_setting(&mut self, sub: u8) -> Result<u8> {
        Ok(self.values[&sub])
    }

    fn write_setting(&mut self, sub: u8, value: u8) -> Result<Option<Vec<u8>>> {
        self.log.borrow_mut().writes.push((sub, value));
        if self.fail_write == Some(sub) {
            return Err(Error::other("The headset did not keep the new value."));
        }
        self.values.insert(sub, value);
        Ok(None)
    }
}

fn run_worker(request: Value, fail_write: Option<u8>) -> (Result<Value>, Progress, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let values = HashMap::from([(LEFT_GAIN, 141), (RIGHT_GAIN, 149), (SOURCE_PROFILE, 0x0A)]);
    let mut mock = Some(Mock { values, fail_write, log: log.clone() });
    let mut open = |write, transport| -> Result<Box<dyn Settings>> {
        log.borrow_mut().opened.push((write, transport));
        Ok(Box::new(mock.take().expect("opened twice")))
    };
    let mut progress = Progress::default();
    let result = execute(&request, &mut open, &mut progress);
    (result, progress, log)
}

#[test]
fn read_opens_without_write_permission() {
    let (result, _, log) = run_worker(json!({"operation": "balance_read", "transport": "dongle"}), None);
    assert_eq!(
        result.unwrap(),
        json!({"left": 141, "right": 149, "source": 10, "source_profile": "usb-c", "transport": "dongle"})
    );
    assert_eq!(log.borrow().opened, vec![(false, Transport::Dongle)]);
}

#[test]
fn balance_write_changes_only_what_differs_and_reports_the_old_values() {
    let (result, progress, log) = run_worker(json!({"operation": "balance_write", "left": 141, "right": 144}), None);
    let result = result.unwrap();
    assert_eq!(log.borrow().writes, vec![(RIGHT_GAIN, 144)]);
    assert_eq!(log.borrow().opened, vec![(true, Transport::Usb)]);
    assert_eq!((result["left"].clone(), result["right"].clone()), (json!(141), json!(144)));
    assert_eq!(result["previous"]["right"], json!(149));
    assert!(progress.write_started);
}

#[test]
fn bad_values_are_refused_before_opening() {
    for request in [
        json!({"operation": "balance_write", "left": 136, "right": 140}),
        json!({"operation": "balance_write", "left": 140, "right": 142}),
        json!({"operation": "balance_write", "left": 151, "right": 140}),
        json!({"operation": "balance_write", "left": -1, "right": 140}),
        json!({"operation": "balance_write", "left": 140.5, "right": 140}),
        json!({"operation": "balance_write", "left": 140}),
        json!({"operation": "source_write", "source": "bluetooth"}),
        json!({"operation": "balance_write", "left": 140, "right": 140, "transport": "bt"}),
        json!({"operation": "gain_write"}),
    ] {
        let (result, progress, log) = run_worker(request.clone(), None);
        assert!(result.is_err(), "{request}");
        assert!(log.borrow().opened.is_empty(), "{request}");
        let output = failure(&result.unwrap_err(), &progress);
        assert!(output["error"].as_str().unwrap().ends_with("Nothing on the headset was changed."), "{output}");
    }
}

#[test]
fn source_write_sets_wireless() {
    let (result, _, log) = run_worker(json!({"operation": "source_write", "source": "wireless"}), None);
    assert_eq!(log.borrow().writes, vec![(SOURCE_PROFILE, WIRELESS_PROFILE)]);
    assert_eq!(result.unwrap()["source_profile"], json!("wireless"));
}

#[test]
fn a_failed_second_channel_reports_the_first_as_changed() {
    let (result, progress, log) =
        run_worker(json!({"operation": "balance_write", "left": 140, "right": 145}), Some(RIGHT_GAIN));
    let output = failure(&result.unwrap_err(), &progress);
    assert_eq!(log.borrow().writes, vec![(LEFT_GAIN, 140), (RIGHT_GAIN, 145)]);
    assert_eq!(output["write_started"], json!(true));
    assert_eq!(output["previous"]["left"], json!(141));
    assert!(!output["error"].as_str().unwrap().contains("Nothing on the headset"));
}

/// App rules with a fake worker.
fn app(enable: bool, replies: Vec<Result<Value>>) -> (App, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let mut replies: VecDeque<Result<Value>> = replies.into();
    let runner = Box::new(move |worker: Worker, request: &Value| {
        assert_eq!(worker, Worker::Device);
        log.lock().unwrap().push(request.clone());
        replies.pop_front().expect("unexpected worker call")
    });
    let quiet = Arc::new(AudezeWatch::new(Box::new(|| Some(vec![])), Duration::from_secs(60)));
    (App::new(enable, None, quiet, runner), calls)
}

fn read_reply() -> Value {
    json!({"left": 141, "right": 149, "source": 10, "source_profile": "usb-c"})
}

#[test]
fn apply_needs_a_read_first() {
    let (mut app, calls) = app(true, vec![]);
    let error = app.perform("/api/balance/apply", &json!({"left": 140, "right": 140})).unwrap_err();
    assert!(error.to_string().contains("Read the current balance first"));
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn read_then_apply_routes_to_the_worker() {
    let mut written = read_reply();
    written["right"] = json!(145);
    written["previous"] = read_reply();
    let (mut app, calls) = app(true, vec![Ok(read_reply()), Ok(written)]);
    let read = app.perform("/api/balance/read", &json!({"transport": "dongle"})).unwrap();
    assert_eq!(read["message"], json!("Balance read: left 141, right 149."));
    let result = app.perform("/api/balance/apply", &json!({"left": 141, "right": 145})).unwrap();
    assert!(result["message"].as_str().unwrap().contains("saved on the headset"));
    let calls = calls.lock().unwrap();
    assert_eq!(calls[1], json!({"operation": "balance_write", "transport": "dongle", "left": 141, "right": 145}));
    assert_eq!(app.status()["balance"]["right"], json!(145));
    // EQ state is untouched.
    assert!(app.index.is_null() && app.links.iter().all(Value::is_null));
    assert_eq!(app.status()["transport"], json!("auto"));
}

#[test]
fn writes_go_through_the_connection_that_was_read() {
    let (mut app, calls) = app(true, vec![Ok(read_reply())]);
    app.perform("/api/balance/read", &json!({"transport": "dongle"})).unwrap();
    // The EQ connection is not moved by a balance read.
    assert_eq!(app.status()["transport"], json!("auto"));
    let error = app.perform("/api/balance/apply", &json!({"transport": "usb", "left": 140, "right": 149})).unwrap_err();
    assert!(error.to_string().contains("through this connection"));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn a_wireless_profile_write_does_not_promise_to_persist() {
    let mut wireless = read_reply();
    wireless["source"] = json!(0);
    wireless["source_profile"] = json!("wireless");
    let mut written = wireless.clone();
    written["left"] = json!(140);
    let (mut app, _) = app(true, vec![Ok(wireless), Ok(written)]);
    app.perform("/api/balance/read", &json!({})).unwrap();
    let result = app.perform("/api/balance/apply", &json!({"left": 140, "right": 149})).unwrap();
    let message = result["message"].as_str().unwrap();
    assert!(!message.contains("saved on the headset"));
    assert!(message.contains("switch the profile to USB-C"));
}

#[test]
fn an_interrupted_write_forgets_the_last_read() {
    let mut details = serde_json::Map::new();
    details.insert("error".into(), json!("The headset did not keep the new value."));
    details.insert("write_started".into(), json!(true));
    let (mut app, _) = app(true, vec![Ok(read_reply()), Err(Error::Worker(details))]);
    app.perform("/api/balance/read", &json!({})).unwrap();
    assert!(app.perform("/api/balance/source", &json!({"source": "wireless"})).is_err());
    assert!(app.balance.is_null());
}

#[test]
fn offline_mode_blocks_balance() {
    let (mut app, calls) = app(false, vec![]);
    for path in ["/api/balance/read", "/api/balance/apply", "/api/balance/source"] {
        assert!(app.perform(path, &json!({})).unwrap_err().to_string().contains("--offline"));
    }
    assert!(calls.lock().unwrap().is_empty());
}
