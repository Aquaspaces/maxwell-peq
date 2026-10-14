//! Headset controls and onboard preset switching: framing, the allowlist, the
//! HID session's write-then-verify, the worker, and the app rules. Nothing
//! opens a headset.

use maxwell_core::app::App;
use maxwell_core::audeze::AudezeWatch;
use maxwell_core::controls::{Controls, Progress, execute, failure};
use maxwell_core::hid::{Interface, ReportIo, Session, Transport};
use maxwell_core::protocol::*;
use maxwell_core::worker::Worker;
use maxwell_core::{Error, Result};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn message(kind: u8, command: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x05, kind];
    packet.extend(((payload.len() + 2) as u16).to_le_bytes());
    packet.extend(command.to_le_bytes());
    packet.extend(payload);
    packet
}

fn response(command: u16, payload: &[u8]) -> Vec<u8> {
    message(0x5B, command, payload)
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

fn pair_reply(sub: u8, value: u8) -> Vec<u8> {
    response(0x2C83, &[0, sub, 0, value])
}

#[test]
fn commands_match_audeze_hq() {
    assert_eq!(hex::encode(read_control_command(CHAT_MIX).unwrap()), "055a0400832c0b00");
    assert_eq!(hex::encode(read_control_command(NOISE_REDUCTION).unwrap()), "055a040001092400");
    assert_eq!(hex::encode(write_control_command(CHAT_MIX, 20).unwrap()), "055a0500822c0b0014");
    assert_eq!(hex::encode(write_control_command(SIDETONE_ON, 1).unwrap()), "055a0500822c070001");
    assert_eq!(hex::encode(write_control_command(AMP_GAIN, 136).unwrap()), "055a05000009280088");
    assert_eq!(hex::encode(write_control_command(PRESET_INDEX, 8).unwrap()), "055a05000009000008");
    // 30 minutes: enabled, 0x0708 seconds, twice.
    assert_eq!(
        hex::encode(write_control_command(AUTO_OFF, 1800).unwrap()),
        "055a0c00822c010001000807010008 07".replace(' ', "")
    );
    assert_eq!(hex::encode(write_control_command(AUTO_OFF, 0).unwrap()), "055a0c00822c01000000000000000000");
    assert_eq!(hex::encode(battery_command()), "055a0300d60c00");
    assert_eq!(hex::encode(firmware_command()), "055a0300071c00");
}

#[test]
fn control_values_are_range_checked() {
    for (sub, value) in [
        (PRESET_INDEX, 0),
        (PRESET_INDEX, 11),
        (CHAT_MIX, 21),
        (NOISE_REDUCTION, 3),
        (VOICE_PROMPT, 16),
        (SIDETONE_ON, 2),
        (AMP_GAIN, 140),
        (AUTO_OFF, 1200),
        (SIDETONE_LEVEL, 256),
    ] {
        assert!(write_control_command(sub, value).is_err(), "{sub:#x} {value}");
    }
    // Read-only and unknown subs cannot be written.
    for sub in [MIC_MUTE, MAX_SIDETONE, LEFT_GAIN, 0x2F, 0x30] {
        assert!(write_control_command(sub, 0).is_err(), "{sub:#x}");
    }
}

#[test]
fn the_controls_allowlist_is_separate_from_eq_and_balance() {
    let writes = [
        write_control_command(PRESET_INDEX, 1).unwrap(),
        write_control_command(CHAT_MIX, 10).unwrap(),
        write_control_command(AMP_GAIN, 142).unwrap(),
        write_control_command(AUTO_OFF, 300).unwrap(),
    ];
    for write in &writes {
        validate_control(write, true).unwrap();
        assert!(validate_control(write, false).is_err());
        assert!(validate_request(write, true, true).is_err());
        assert!(validate_setting_write(write).is_err());
    }
    // The EQ session may reselect a custom preset only for the Maxwell 2 reload after a save.
    let reselect = write_control_command(PRESET_INDEX, 7).unwrap();
    validate_control(&reselect, true).unwrap();
    assert!(validate_request(&reselect, true, false).is_err());
    validate_request(&reselect, false, true).unwrap();
    assert!(validate_setting_write(&reselect).is_err());
    for sub in [PRESET_INDEX, CHAT_MIX, AUTO_OFF, MIC_MUTE, MAX_SIDETONE] {
        validate_control(&read_control_command(sub).unwrap(), false).unwrap();
    }
    validate_control(&battery_command(), false).unwrap();
    validate_control(&firmware_command(), false).unwrap();
    // Near misses: a mismatched auto-off copy, a gain sub, EQ, an out-of-range mix.
    for raw in [
        "055a0c00822c010001000807010009 07",
        "055a05000009290088",
        "055a0500822c0b0015",
        "055a0500822c010001",
        "055a0300030e00",
    ] {
        let raw = hex::decode(raw.replace(' ', "")).unwrap();
        assert!(validate_control(&raw, true).is_err(), "{}", hex::encode(&raw));
    }
    // Balance still cannot switch the preset or touch amp gain.
    assert!(write_setting_command(PRESET_INDEX, 7).is_err());
    assert!(write_setting_command(AMP_GAIN, 140).is_err());
}

#[test]
fn control_replies_must_carry_status_and_sub() {
    assert_eq!(parse_control(&pair_reply(CHAT_MIX, 12), CHAT_MIX).unwrap(), 12);
    assert!(parse_control(&pair_reply(SIDETONE_ON, 1), CHAT_MIX).is_err());
    assert!(
        parse_control(&response(0x2C83, &[3, CHAT_MIX, 0, 12]), CHAT_MIX).unwrap_err().to_string().contains("status 3")
    );
    let auto_off = response(0x2C83, &[0, AUTO_OFF, 0, 1, 0, 0x08, 0x07, 1, 0, 0x08, 0x07]);
    assert_eq!(parse_control(&auto_off, AUTO_OFF).unwrap(), 1800);
    assert_eq!(parse_control(&response(0x0901, &[NOISE_REDUCTION, 0, 0, 2]), NOISE_REDUCTION).unwrap(), 2);
    assert!(control_matches(AMP_GAIN, 136, 130));
    assert!(!control_matches(AMP_GAIN, 142, 136));
    assert!(control_matches(AMP_GAIN, 142, 142));
}

#[test]
fn battery_and_firmware_come_from_notifications() {
    let notice = message(0x5D, 0x0CD6, &[0, 0, 85]);
    assert_eq!(parse_battery(&notice).unwrap(), 85);
    assert!(parse_battery(&response(0x0CD6, &[0, 0, 85])).is_err());
    let version = message(0x5D, 0x1C07, &[0, 0, 5, b'1', b'.', b'2', b'.', b'3']);
    assert_eq!(parse_firmware(&version).unwrap(), "1.2.3");
    assert!(parse_firmware(&message(0x5D, 0x1C07, &[0, 0, 9, b'1'])).is_err());
}

/// A fake headset that answers control reads, writes and queries.
struct Headset {
    values: HashMap<u8, u16>,
    keeps: bool,
    /// Answer queries with the notification first, in the same report.
    notice_first: bool,
    stray: Option<Vec<u8>>,
    /// Behind the dongle, report flag 0 addresses the dongle itself.
    on_dongle: bool,
    queue: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl Headset {
    fn new() -> Self {
        Headset {
            values: HashMap::from([
                (PRESET_INDEX, 7),
                (CHAT_MIX, 10),
                (AMP_GAIN, 142),
                (SIDETONE_ON, 0),
                (SIDETONE_LEVEL, 3),
                (MAX_SIDETONE, 10),
                (NOISE_REDUCTION, 1),
                (VOICE_PROMPT, 8),
                (AUTO_OFF, 1800),
                (MIC_MUTE, 0xFF),
            ]),
            keeps: true,
            notice_first: false,
            stray: None,
            on_dongle: false,
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
            0x0901 => response(0x0901, &[sub, 0, 0, self.values[&sub] as u8]),
            0x2C83 if sub == AUTO_OFF => {
                let [lo, hi] = self.values[&sub].to_le_bytes();
                response(0x2C83, &[0, sub, 0, 1, 0, lo, hi, 1, 0, lo, hi])
            }
            0x2C83 => pair_reply(sub, self.values[&sub] as u8),
            0x0900 | 0x2C82 => {
                if self.keeps {
                    let value = if sub == AUTO_OFF {
                        u16::from_le_bytes([payload[4], payload[5]])
                    } else {
                        u16::from(payload[2])
                    };
                    self.values.insert(sub, value);
                }
                response(command, &[0, sub, 0])
            }
            0x0CD6 | 0x1C07 => {
                let ack = response(command, &[0]);
                let notice = if command == 0x0CD6 {
                    message(0x5D, command, &[0, 0, 85])
                } else {
                    let text: &[u8] = if self.on_dongle && report[2] == 0 { b"dongle-2.0" } else { b"headset-1.0" };
                    message(0x5D, command, &[&[0, 0, text.len() as u8][..], text].concat())
                };
                let both = if self.notice_first { [notice, ack].concat() } else { [ack, notice].concat() };
                self.queue.push_back(input_report(&both));
                return Ok(true);
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

fn session(mut headset: Headset, transport: Transport, allow_controls: bool) -> Session<Headset> {
    headset.on_dongle = transport == Transport::Dongle;
    let interface = Interface { path: "test-hid-path".into(), input_size: 62, output_size: 62, device_version: 0x0100 };
    let mut session = Session::new(headset, transport, interface, false, false);
    session.allow_controls = allow_controls;
    session.sleep = |_| {};
    session
}

fn commands(headset: &Headset) -> Vec<String> {
    headset.writes.iter().map(|r| hex::encode(&r[3..3 + usize::from(r[1])])).collect()
}

#[test]
fn a_late_reply_for_another_control_is_ignored() {
    let mut headset = Headset::new();
    headset.stray = Some(pair_reply(SIDETONE_ON, 1));
    let mut session = session(headset, Transport::Usb, false);
    assert_eq!(session.read_control(CHAT_MIX).unwrap(), 10);
    assert_eq!(session.read_control(AUTO_OFF).unwrap(), 1800);
}

#[test]
fn a_control_write_is_sent_once_and_read_back() {
    let mut session = session(Headset::new(), Transport::Usb, true);
    session.write_control(CHAT_MIX, 15).unwrap();
    let sent = commands(&session.io);
    assert_eq!(sent[0], "055a0500822c0b000f");
    assert!(sent[1..].iter().all(|c| c == "055a0400832c0b00"), "{sent:?}");
    assert_eq!(session.io.values[&CHAT_MIX], 15);
    session.write_control(AUTO_OFF, 0).unwrap();
    assert_eq!(session.io.values[&AUTO_OFF], 0);
}

#[test]
fn a_control_the_headset_drops_is_an_error() {
    let mut headset = Headset::new();
    headset.keeps = false;
    let mut session = session(headset, Transport::Usb, true);
    let error = session.write_control(NOISE_REDUCTION, 2).unwrap_err().to_string();
    assert!(error.contains("did not keep"), "{error}");
    assert_eq!(commands(&session.io).iter().filter(|c| c.starts_with("055a0500")).count(), 1, "no blind retries");
}

#[test]
fn control_writes_need_the_controls_permission() {
    let mut session = self::session(Headset::new(), Transport::Usb, false);
    assert!(session.write_control(CHAT_MIX, 5).is_err());
    assert!(session.transact(&write_control_command(CHAT_MIX, 5).unwrap(), None).is_err());
    assert!(session.io.writes.is_empty());
    // A controls session still cannot send EQ or balance.
    let mut session = self::session(Headset::new(), Transport::Usb, true);
    assert!(session.transact(&live_command(&[1]).unwrap(), None).is_err());
    assert!(session.transact(&write_setting_command(LEFT_GAIN, 140).unwrap(), None).is_err());
    assert!(session.io.writes.is_empty());
}

#[test]
fn queries_accept_the_notification_in_either_order() {
    for notice_first in [false, true] {
        let mut headset = Headset::new();
        headset.notice_first = notice_first;
        let mut session = session(headset, Transport::Usb, false);
        assert_eq!(session.battery().unwrap(), 85);
        assert_eq!(session.firmware(false).unwrap(), "headset-1.0");
        assert!(session.firmware(true).is_err(), "no dongle on USB-C");
    }
}

#[test]
fn the_dongle_firmware_query_clears_the_report_flag() {
    let mut session = session(Headset::new(), Transport::Dongle, false);
    assert_eq!(session.firmware(false).unwrap(), "headset-1.0");
    assert_eq!(session.firmware(true).unwrap(), "dongle-2.0");
    let flags: Vec<u8> = session.io.writes.iter().map(|r| r[2]).collect();
    assert_eq!(flags, vec![0x80, 0]);
}

/// The worker against a mock that records what it was asked to do.
#[derive(Default)]
struct Log {
    opened: Vec<(bool, Transport)>,
    writes: Vec<(u8, u16)>,
}

struct Mock {
    values: HashMap<u8, u16>,
    fail_write: bool,
    log: Rc<RefCell<Log>>,
}

impl Controls for Mock {
    fn read_control(&mut self, sub: u8) -> Result<u16> {
        Ok(self.values[&sub])
    }

    fn write_control(&mut self, sub: u8, value: u16) -> Result<Option<Vec<u8>>> {
        self.log.borrow_mut().writes.push((sub, value));
        if self.fail_write {
            return Err(Error::other("The headset did not keep the new value."));
        }
        self.values.insert(sub, value);
        Ok(None)
    }

    fn battery(&mut self) -> Result<u8> {
        Err(Error::other("No answer to the query"))
    }

    fn firmware(&mut self, dongle: bool) -> Result<String> {
        Ok(if dongle { "dongle-2.0" } else { "headset-1.0" }.into())
    }
}

fn run_worker(request: Value, fail_write: bool) -> (Result<Value>, Progress, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let values = Headset::new().values;
    let mut mock = Some(Mock { values, fail_write, log: log.clone() });
    let mut open = |write, transport| -> Result<Box<dyn Controls>> {
        log.borrow_mut().opened.push((write, transport));
        Ok(Box::new(mock.take().expect("opened twice")))
    };
    let mut progress = Progress::default();
    let result = execute(&request, &mut open, &mut progress);
    (result, progress, log)
}

#[test]
fn read_reports_every_control_without_write_permission() {
    let (result, _, log) = run_worker(json!({"operation": "controls_read", "transport": "dongle"}), false);
    let result = result.unwrap();
    assert_eq!(log.borrow().opened, vec![(false, Transport::Dongle)]);
    assert_eq!(result["preset"], json!({"index": 7, "name": "EQ 1", "custom": true}));
    assert_eq!(result["volume_limiter"], json!(false));
    assert_eq!(result["sidetone"], json!(false));
    assert_eq!(result["mic_muted"], json!(false));
    assert_eq!(result["auto_off"], json!(1800));
    assert_eq!(result["battery"], Value::Null, "a missing battery reading is not fatal");
    assert_eq!(result["firmware"], json!({"headset": "headset-1.0", "dongle": "dongle-2.0"}));
}

#[test]
fn a_write_changes_one_control_and_reports_the_old_value() {
    let (result, progress, log) =
        run_worker(json!({"operation": "control_write", "control": "volume_limiter", "value": true}), false);
    let result = result.unwrap();
    assert_eq!(log.borrow().writes, vec![(AMP_GAIN, 136)]);
    assert_eq!((result["value"].clone(), result["previous"].clone()), (json!(true), json!(false)));
    assert!(progress.write_started);
    // Already set: nothing is written.
    let (result, progress, log) =
        run_worker(json!({"operation": "control_write", "control": "chat_mix", "value": 10}), false);
    assert!(log.borrow().writes.is_empty());
    assert_eq!(result.unwrap()["changed"], json!(false));
    assert!(!progress.write_started);
}

#[test]
fn sidetone_level_is_capped_by_the_headset_maximum() {
    let request = json!({"operation": "control_write", "control": "sidetone_level", "value": 11});
    let (result, progress, log) = run_worker(request, false);
    assert!(result.unwrap_err().to_string().contains("maximum"));
    assert!(log.borrow().writes.is_empty());
    assert!(!progress.write_started);
    let (result, _, _) =
        run_worker(json!({"operation": "control_write", "control": "sidetone_level", "value": 10}), false);
    assert_eq!(result.unwrap()["value"], json!(10));
}

#[test]
fn bad_control_values_are_refused_before_opening() {
    for (control, value) in [
        ("chat_mix", json!(21)),
        ("chat_mix", json!(-1)),
        ("noise_reduction", json!(3)),
        ("voice_prompt", json!(1.5)),
        ("sidetone", json!(1)),
        ("volume_limiter", json!("on")),
        ("auto_off", json!(1200)),
        ("preset", json!(0)),
        ("preset", json!(11)),
        ("mic_mute", json!(true)),
        ("factory_reset", json!(1)),
    ] {
        let request = json!({"operation": "control_write", "control": control, "value": value});
        let (result, progress, log) = run_worker(request.clone(), false);
        assert!(result.is_err(), "{request}");
        assert!(log.borrow().opened.is_empty(), "{request}");
        let output = failure(&result.unwrap_err(), &progress);
        assert!(output["error"].as_str().unwrap().ends_with("Nothing on the headset was changed."), "{output}");
    }
}

#[test]
fn a_failed_write_says_the_headset_may_have_changed() {
    let request = json!({"operation": "control_write", "control": "noise_reduction", "value": 2});
    let (result, progress, _) = run_worker(request, true);
    let output = failure(&result.unwrap_err(), &progress);
    assert_eq!(output["write_started"], json!(true));
    assert_eq!(output["previous"], json!(1));
    assert!(!output["error"].as_str().unwrap().contains("Nothing on the headset"));
}

/// App rules with a fake worker.
fn app(data_dir: Option<PathBuf>, replies: Vec<Result<Value>>) -> (App, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let mut replies: VecDeque<Result<Value>> = replies.into();
    let runner = Box::new(move |worker: Worker, request: &Value| {
        assert_eq!(worker, Worker::Device);
        log.lock().unwrap().push(request.clone());
        replies.pop_front().expect("unexpected worker call")
    });
    let quiet = Arc::new(AudezeWatch::new(Box::new(|| Some(vec![])), Duration::from_secs(60)));
    (App::new(true, data_dir, quiet, runner), calls)
}

fn controls_reply() -> Value {
    json!({"preset": {"index": 2, "name": "Treble Boost", "custom": false}, "chat_mix": 10,
           "volume_limiter": false, "amp_gain": 142, "transport": "usb"})
}

fn written(control: &str, value: Value, previous: Value) -> Value {
    json!({"control": control, "value": value, "previous": previous, "changed": true, "raw": 136,
           "write_replies_hex": ["00"], "transport": "usb"})
}

#[test]
fn control_writes_need_a_read_through_the_same_connection() {
    let (mut app, calls) = app(None, vec![Ok(controls_reply())]);
    let error = app.perform("/api/controls/set", &json!({"control": "chat_mix", "value": 5})).unwrap_err();
    assert!(error.to_string().contains("Read the headset controls first"));
    app.perform("/api/controls/read", &json!({"transport": "usb"})).unwrap();
    let error = app
        .perform("/api/controls/set", &json!({"control": "chat_mix", "value": 5, "transport": "dongle"}))
        .unwrap_err();
    assert!(error.to_string().contains("through this connection"));
    // The preset goes through its own endpoint.
    assert!(app.perform("/api/controls/set", &json!({"control": "preset", "value": 7})).is_err());
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn a_control_write_updates_the_cached_read() {
    let reply = written("volume_limiter", json!(true), json!(false));
    let (mut app, calls) = app(None, vec![Ok(controls_reply()), Ok(reply)]);
    app.perform("/api/controls/read", &json!({})).unwrap();
    let result = app.perform("/api/controls/set", &json!({"control": "volume_limiter", "value": true})).unwrap();
    assert!(result.get("write_replies_hex").is_none());
    assert_eq!(
        calls.lock().unwrap()[1],
        json!({"operation": "control_write", "control": "volume_limiter", "value": true, "transport": "usb"})
    );
    let status = app.status();
    assert_eq!(
        (status["controls"]["volume_limiter"].clone(), status["controls"]["amp_gain"].clone()),
        (json!(true), json!(136))
    );
}

#[test]
fn an_interrupted_control_write_forgets_the_last_read() {
    let mut details = serde_json::Map::new();
    details.insert("error".into(), json!("The headset did not keep the new value."));
    details.insert("write_started".into(), json!(true));
    let (mut app, _) = app(None, vec![Ok(controls_reply()), Err(Error::Worker(details))]);
    app.perform("/api/controls/read", &json!({})).unwrap();
    assert!(app.perform("/api/controls/set", &json!({"control": "chat_mix", "value": 2})).is_err());
    assert!(app.controls.is_null());
}

#[test]
fn switching_to_a_built_in_preset_reads_the_headset_again() {
    let switched = written("preset", json!({"index": 3, "name": "Bass Boost", "custom": false}), json!({"index": 7}));
    let read = json!({"index": 3, "slot": null, "device": {"path": "p"}, "transport": "usb"});
    let (mut app, calls) = app(None, vec![Ok(switched), Ok(read)]);
    let result = app.perform("/api/preset/select", &json!({"index": 3})).unwrap();
    assert!(result["message"].as_str().unwrap().starts_with("Switched to Bass Boost. Make Preset 1 to 4 active"));
    assert_eq!(result["previous"], json!({"index": 7}));
    assert_eq!(app.index, json!(3));
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0], json!({"operation": "control_write", "control": "preset", "value": 3, "transport": "auto"}));
    assert_eq!(calls[1], json!({"operation": "read", "transport": "auto"}));
}

#[test]
fn switching_to_a_custom_preset_keeps_its_saved_profile() {
    let switched = written("preset", json!({"index": 8, "name": "EQ 2", "custom": true}), json!({"index": 2}));
    let blob = hex::encode(vec![0u8; SAVED_BYTES]);
    let read = json!({"index": 8, "slot": 1, "raw_ui_hex": "aa", "saved_coefficients_hex": blob,
                      "device": {"path": "p"}, "transport": "usb"});
    let (mut app, _) = app(None, vec![Ok(switched), Ok(read)]);
    app.links[1] = json!({"profile_id": "p1", "name": "Mine", "preset": {}, "blob_hex": blob});
    let result = app.perform("/api/preset/select", &json!({"index": 8})).unwrap();
    assert_eq!(result["message"], json!("Switched to EQ 2."));
    assert_eq!(app.index, json!(8));
    assert_eq!(app.links[1]["profile_id"], json!("p1"));
}

#[test]
fn offline_mode_blocks_controls() {
    let quiet = Arc::new(AudezeWatch::new(Box::new(|| Some(vec![])), Duration::from_secs(60)));
    let mut app =
        App::new(false, None, quiet, Box::new(|_: Worker, _: &Value| -> Result<Value> { panic!("no worker calls") }));
    for path in ["/api/controls/read", "/api/controls/set", "/api/preset/select"] {
        assert!(app.perform(path, &json!({})).unwrap_err().to_string().contains("--offline"));
    }
}

#[test]
fn a_glance_reads_the_preset_mic_and_battery_only() {
    let (result, progress, log) = run_worker(json!({"operation": "glance", "transport": "usb"}), false);
    let result = result.unwrap();
    assert_eq!(log.borrow().opened, vec![(false, Transport::Usb)]);
    assert!(log.borrow().writes.is_empty() && !progress.write_started);
    assert_eq!(result["preset"], json!({"index": 7, "name": "EQ 1", "custom": true}));
    assert_eq!(result["mic_muted"], json!(false));
    assert_eq!(result["battery"], Value::Null);
    assert_eq!(result["transport"], json!("usb"));
}
