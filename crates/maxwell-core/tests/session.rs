//! The session state machine with a fake worker; nothing opens a headset.

use maxwell_core::app::App;
use maxwell_core::audeze::{AudezeWatch, matching};
use maxwell_core::worker::{Runner, Worker};
use maxwell_core::{Error, Result};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Reply = Box<dyn FnOnce(Worker, &Value) -> Result<Value> + Send>;

/// Worker calls in order, answered from a queue the test refills.
#[derive(Clone, Default)]
struct Fake {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    calls: Arc<Mutex<Vec<(Worker, Value)>>>,
}

impl Fake {
    fn runner(&self) -> Runner {
        let fake = self.clone();
        Box::new(move |worker, request| {
            fake.calls.lock().unwrap().push((worker, request.clone()));
            let reply = fake.replies.lock().unwrap().pop_front();
            reply.unwrap_or_else(|| panic!("unexpected {worker:?} call: {request}"))(worker, request)
        })
    }

    fn reply(&self, reply: impl FnOnce(Worker, &Value) -> Result<Value> + Send + 'static) -> &Self {
        self.replies.lock().unwrap().push_back(Box::new(reply));
        self
    }

    fn ok(&self, value: Value) -> &Self {
        self.reply(move |_, _| Ok(value))
    }

    fn err(&self, error: Error) -> &Self {
        self.reply(move |_, _| Err(error))
    }

    fn calls(&self) -> Vec<(Worker, Value)> {
        std::mem::take(&mut self.calls.lock().unwrap())
    }
}

fn worker_error(fields: Value) -> Error {
    Error::Worker(fields.as_object().unwrap().clone())
}

fn quiet() -> Arc<AudezeWatch> {
    Arc::new(AudezeWatch::new(Box::new(|| Some(vec![])), Duration::from_secs(60)))
}

/// A directory under the system temp dir, removed on drop.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("maxwell-peq-test-{}", maxwell_core::util::token_hex8()));
        std::fs::create_dir_all(&path).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

mod session {
    use super::*;

    fn app() -> (App, Fake) {
        let fake = Fake::default();
        (App::new(true, None, quiet(), fake.runner()), fake)
    }

    fn state(index: u64, saved: &str) -> Value {
        json!({"index": index, "slot": index.checked_sub(7), "saved_coefficients_hex": saved})
    }

    fn preset(gain: f64) -> Value {
        json!({"filters": [{"type": "peak", "frequency": 1000, "gain": gain, "q": 1, "enabled": true}], "preamp": "auto"})
    }

    fn save(slot: u64, gain: f64) -> Value {
        json!({"slot": slot, "preset": preset(gain), "profile_id": format!("p{slot}"), "name": format!("Profile {slot}")})
    }

    #[test]
    fn saving_links_the_profile_to_that_preset() {
        let (mut app, fake) = app();
        let mut found = state(10, "1122");
        found["transport"] = json!("usb");
        fake.ok(json!({"blob_hex": "3344"})).ok(json!({"state": found, "saved_verified": true, "live_applied": false}));
        let result = app.perform("/api/save", &save(1, -3.0)).unwrap();
        assert_eq!(result["message"], "Saved to Preset 2. Make it active to hear it.");
        let calls = fake.calls();
        assert_eq!(calls[0], (Worker::Compile, preset(-3.0)));
        assert_eq!(
            calls[1],
            (Worker::Device, json!({"operation": "save", "slot": 1, "blob_hex": "3344", "transport": "auto"}))
        );
        let status = app.status();
        assert_eq!(status["device"], "Original Maxwell PS · USB-C · Preset 4");
        assert_eq!(status["index"], 10);
        assert_eq!(
            status["slots"][1],
            json!({"slot": 1, "index": 8, "active": false, "profile_id": "p1", "name": "Profile 1", "preset": preset(-3.0)})
        );
        assert_eq!(status["slots"][3]["active"], true);
        assert!(status["slots"][3]["profile_id"].is_null());
        // Automatic stays the choice; the connection it found is reported.
        assert_eq!((&status["transport"], &status["connection"]), (&json!("auto"), &json!("usb")));
        for gone in ["backup_exists", "last_profile", "saved_profile"] {
            assert!(status.get(gone).is_none(), "{gone}");
        }
    }

    #[test]
    fn a_link_lasts_until_the_preset_stores_other_eq() {
        let (mut app, fake) = app();
        fake.ok(json!({"blob_hex": "5566"})).ok(json!({"state": state(10, "5566"), "live_applied": true}));
        let result = app.perform("/api/save", &save(3, 2.0)).unwrap();
        assert_eq!(result["message"], "Saved to Preset 4. It is playing now.");

        let mut read = state(10, "5566");
        read["slots"] = json!([null, null, null, "5566"]);
        fake.ok(read);
        let result = app.perform("/api/read_device", &json!({})).unwrap();
        assert_eq!(result["message"], "Headset read. Preset 4 is active.");
        assert!(result["state"].get("slots").is_none());
        assert_eq!(app.links[3]["name"], "Profile 3");

        // The Audeze app rewrote the preset: the link goes.
        fake.ok(state(10, "7788"));
        app.perform("/api/read_device", &json!({})).unwrap();
        assert!(app.links[3].is_null());
    }

    #[test]
    fn links_are_kept_across_restarts() {
        let temp = Temp::new();
        let fake = Fake::default();
        let mut app = App::new(true, Some(temp.0.clone()), quiet(), fake.runner());
        fake.ok(json!({"blob_hex": "5566"})).ok(json!({"state": state(10, "1122")}));
        app.perform("/api/save", &save(0, 1.0)).unwrap();
        let again = App::new(true, Some(temp.0.clone()), quiet(), fake.runner());
        assert_eq!(again.links[0]["profile_id"], "p0");
        assert_eq!(again.links[0]["blob_hex"], "5566");
        assert!(again.links[1].is_null());
    }

    #[test]
    fn select_makes_a_preset_active() {
        let (mut app, fake) = app();
        fake.ok(json!({"state": state(8, "1122"), "selected": true}));
        let result = app.perform("/api/select", &json!({"index": 8, "transport": "dongle"})).unwrap();
        assert_eq!(result["message"], "Preset 2 is now active.");
        assert_eq!(fake.calls()[0].1, json!({"operation": "select", "index": 8, "transport": "dongle"}));
        assert_eq!(app.status()["slots"][1]["active"], true);
        for index in [json!(6), json!(11), json!("8"), Value::Null] {
            assert!(app.perform("/api/select", &json!({"index": index})).is_err(), "{index}");
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn reading_a_built_in_preset_says_how_to_hear_profiles() {
        let (mut app, fake) = app();
        fake.ok(state(1, ""));
        let result = app.perform("/api/read_device", &json!({"transport": "dongle"})).unwrap();
        assert!(result["message"].as_str().unwrap().contains("built-in preset 1"), "{result}");
        assert_eq!(result["custom"], false);
        assert_eq!(app.transport, "dongle");
        assert_eq!(app.device, "Original Maxwell PS · dongle · built-in preset 1");
        assert!(app.status()["slots"].as_array().unwrap().iter().all(|slot| slot["active"] == false));
    }

    #[test]
    fn a_save_that_started_but_was_not_verified_drops_the_link() {
        let (mut app, fake) = app();
        fake.ok(json!({"blob_hex": "3344"}))
            .err(worker_error(json!({"error": "No live ACK", "write_started": true, "saved_verified": true})));
        assert!(app.perform("/api/save", &save(2, 4.0)).is_err());
        assert_eq!(app.links[2]["blob_hex"], "3344");

        fake.ok(json!({"blob_hex": "5566"}))
            .err(worker_error(json!({"error": "Readback mismatch", "write_started": true, "saved_verified": false})));
        assert!(app.perform("/api/save", &save(2, 5.0)).is_err());
        assert!(app.links[2].is_null());
    }

    #[test]
    fn a_refused_save_keeps_the_link() {
        let (mut app, fake) = app();
        fake.ok(json!({"blob_hex": "3344"})).ok(json!({"state": state(7, "3344")}));
        app.perform("/api/save", &save(0, 1.0)).unwrap();
        fake.ok(json!({"blob_hex": "5566"})).err(worker_error(
            json!({"error": "Headset reports insufficient profile storage. No EQ was sent.", "write_started": false}),
        ));
        assert!(app.perform("/api/save", &save(0, 4.0)).is_err());
        assert_eq!(app.links[0]["blob_hex"], "3344");
    }

    #[test]
    fn bad_saves_send_nothing() {
        let (mut app, fake) = app();
        fake.err(Error::worker("Invalid filter"));
        assert!(app.perform("/api/save", &save(0, 1.0)).is_err());
        assert_eq!(fake.calls().len(), 1);
        for slot in [json!(4), json!(-1), Value::Null] {
            let mut request = save(0, 1.0);
            request["slot"] = slot;
            assert!(app.perform("/api/save", &request).is_err());
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn requests_use_and_remember_their_connection() {
        let (mut app, fake) = app();
        let mut request = save(0, 1.0);
        request["transport"] = json!("dongle");
        fake.ok(json!({"blob_hex": "3344"})).ok(json!({"state": state(10, "1122")}));
        app.perform("/api/save", &request).unwrap();
        assert_eq!(fake.calls()[1].1["transport"], "dongle");
        assert_eq!(app.transport, "dongle");
        request["transport"] = json!("bluetooth");
        assert!(app.perform("/api/save", &request).is_err());
    }

    #[test]
    fn balance_writes_are_logged_under_logs() {
        let temp = Temp::new();
        let fake = Fake::default();
        let mut app = App::new(true, Some(temp.0.clone()), quiet(), fake.runner());
        let read = json!({"left": 141, "right": 141, "source": 1, "source_profile": "usb-c"});
        fake.ok(read.clone()).ok(json!({"left": 140, "right": 141, "source": 1, "source_profile": "usb-c"}));
        app.perform("/api/balance/read", &json!({})).unwrap();
        app.perform("/api/balance/apply", &json!({"left": 140, "right": 141})).unwrap();
        let logs: Vec<String> = std::fs::read_dir(temp.0.join("logs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(logs.len() == 1 && logs[0].ends_with("-balance_write.json"), "{logs:?}");
        assert!(!temp.0.join("backups").exists());
    }

    #[test]
    fn offline_mode_blocks_device_access_and_old_endpoints_are_gone() {
        let (mut app, fake) = app();
        for path in ["/api/restore", "/api/restore_saved", "/api/apply"] {
            assert!(app.perform(path, &json!({})).unwrap_err().to_string().contains("Unknown endpoint"), "{path}");
        }
        app.enable_live = false;
        for path in ["/api/read_device", "/api/save", "/api/select"] {
            assert!(matches!(app.perform(path, &json!({})), Err(Error::Value(_))), "{path}");
        }
        assert!(fake.calls().is_empty());
    }
}

mod audeze_guard {
    use super::*;

    #[test]
    fn device_operations_refuse_while_audeze_runs() {
        let fake = Fake::default();
        let watch =
            Arc::new(AudezeWatch::new(Box::new(|| Some(vec!["Audeze HQ.exe".into()])), Duration::from_secs(60)));
        let mut app = App::new(true, None, watch, fake.runner());
        let error = app.perform("/api/read_device", &json!({})).unwrap_err();
        assert!(error.to_string().contains("Audeze App/HQ is open"), "{error}");
        assert!(fake.calls().is_empty());
        assert_eq!(app.status()["audeze_running"], true);
        assert_eq!(app.status()["audeze_apps"], json!(["Audeze HQ.exe"]));
    }

    #[test]
    fn status_reports_false_when_no_audeze_app_runs() {
        let app = App::new(true, None, quiet(), Fake::default().runner());
        assert_eq!(app.status()["audeze_running"], false);
        assert_eq!(app.status()["audeze_apps"], json!([]));
    }

    #[test]
    fn unsupported_check_is_not_reported_as_running() {
        let watch = Arc::new(AudezeWatch::new(Box::new(|| None), Duration::from_secs(60)));
        let app = App::new(true, None, watch, Fake::default().runner());
        assert_eq!(app.status()["audeze_running"], false);
    }

    #[test]
    #[cfg(windows)]
    fn matching_is_case_insensitive_and_canonical() {
        assert_eq!(matching(["audeze hq.exe", "explorer.exe", "AUDEZE.EXE"]), vec!["Audeze.exe", "Audeze HQ.exe"]);
        assert!(matching(["AudezeUpdater.exe"]).is_empty());
    }

    #[test]
    #[cfg(not(windows))]
    fn matching_is_case_insensitive_and_canonical() {
        assert_eq!(matching(["audeze hq", "Finder", "AUDEZE"]), vec!["Audeze", "Audeze HQ"]);
        assert!(matching(["AudezeUpdater"]).is_empty());
    }

    #[test]
    fn status_polls_share_one_check() {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let watch = AudezeWatch::new(
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Some(vec![])
            }),
            Duration::from_secs(60),
        );
        watch.running(false);
        watch.running(false);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        watch.running(true);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        watch.reset();
        watch.running(false);
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
