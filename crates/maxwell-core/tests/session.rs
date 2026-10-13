//! The session state machine with a fake worker; nothing opens a headset.
//! Ports tests/test_recovery.py, test_persistent_session.py and the Audeze guard.

use maxwell_core::app::App;
use maxwell_core::audeze::{AudezeWatch, matching};
use maxwell_core::worker::{Runner, Worker};
use maxwell_core::{Error, Result};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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

mod recovery {
    use super::*;

    fn setup() -> (App, Fake, Value) {
        let fake = Fake::default();
        let mut app = App::new(true, None, quiet(), fake.runner()).unwrap();
        let snapshot = json!({"saved_coefficients_hex": "1122", "slot": 0});
        app.backup = json!({"state": snapshot});
        app.backup_path = json!("test-backup.json");
        (app, fake, snapshot)
    }

    #[test]
    fn changed_preset_before_send_allows_new_backup() {
        let (mut app, fake, _) = setup();
        fake.ok(json!({"blob_hex": "3344"}))
            .err(worker_error(json!({"error": "Changed preset", "write_started": false, "state_changed": true})));
        assert!(matches!(app.perform("/api/apply", &json!({})), Err(Error::Worker(_))));
        assert_eq!(app.live_state, "unchanged");
        assert!(app.backup.is_null());
        assert!(app.backup_path.is_null());
    }

    #[test]
    fn preflight_disconnect_keeps_existing_live_state() {
        let (mut app, fake, _) = setup();
        app.live_state = "applied".into();
        fake.err(worker_error(json!({"error": "No headset", "write_started": false, "state_changed": false})));
        assert!(app.perform("/api/restore", &json!({})).is_err());
        assert_eq!(app.live_state, "applied");
        assert!(!app.backup.is_null());
    }

    #[test]
    fn live_eq_can_switch_connection_and_keep_the_backup() {
        let (mut app, fake, snapshot) = setup();
        app.live_state = "applied".into();
        let read = json!({"saved_coefficients_hex": "1122", "slot": 0, "index": 4});
        fake.ok(read.clone());
        app.perform("/api/read_device", &json!({"transport": "dongle"})).unwrap();
        assert_eq!(fake.calls(), vec![(Worker::Device, json!({"operation": "read", "transport": "dongle"}))]);
        assert_eq!(app.transport, "dongle");
        assert_eq!(app.live_state, "applied");
        assert_eq!(app.backup, json!({"state": snapshot}));
        assert_eq!(app.backup_path, json!("test-backup.json"));
        assert_eq!(app.expected, read);

        fake.ok(json!({}));
        app.perform("/api/restore", &json!({})).unwrap();
        assert_eq!(fake.calls()[0].1["transport"], "dongle");
    }

    #[test]
    fn live_eq_read_refuses_a_changed_saved_preset() {
        let (mut app, fake, snapshot) = setup();
        app.live_state = "applied".into();
        fake.ok(json!({"saved_coefficients_hex": "9999", "slot": 0, "index": 4}));
        let error = app.perform("/api/read_device", &json!({"transport": "dongle"})).unwrap_err();
        assert!(error.to_string().contains("no longer matches"), "{error}");
        assert_eq!(app.transport, "usb");
        assert_eq!(app.backup, json!({"state": snapshot}));
    }

    #[test]
    fn saved_or_uncertain_state_still_blocks_reading() {
        for (live, storage) in [("applied", "saved"), ("uncertain", "unchanged"), ("unchanged", "uncertain")] {
            let (mut app, fake, _) = setup();
            (app.live_state, app.storage_state) = (live.into(), storage.into());
            let error = app.perform("/api/read_device", &json!({"transport": "dongle"})).unwrap_err();
            assert!(error.to_string().contains("Restore the existing backup"), "{error}");
            assert!(fake.calls().is_empty());
        }
    }

    #[test]
    fn timeout_blocks_apply_but_restore_uses_exact_saved_bytes() {
        let (mut app, fake, snapshot) = setup();
        fake.ok(json!({"blob_hex": "3344"})).err(Error::other("Timeout"));
        assert!(matches!(app.perform("/api/apply", &json!({})), Err(Error::Other(_))));
        assert_eq!(app.live_state, "uncertain");
        fake.calls();

        let error = app.perform("/api/apply", &json!({})).unwrap_err();
        assert!(error.to_string().contains("Restore"), "{error}");
        assert!(fake.calls().is_empty());

        fake.ok(json!({}));
        app.perform("/api/restore", &json!({})).unwrap();
        assert_eq!(
            fake.calls(),
            vec![(
                Worker::Device,
                json!({"operation": "apply", "blob_hex": "1122", "expected": snapshot, "transport": "usb"})
            )]
        );
        assert_eq!(app.live_state, "unchanged");
    }

    #[test]
    fn unacknowledged_write_retains_backup() {
        let (mut app, fake, _) = setup();
        fake.ok(json!({"blob_hex": "3344"}))
            .err(worker_error(json!({"error": "Rejected", "write_started": true, "state_changed": false})));
        assert!(app.perform("/api/apply", &json!({})).is_err());
        assert_eq!(app.live_state, "uncertain");
        assert!(!app.backup.is_null());
    }

    #[test]
    fn compile_failure_does_not_change_device_state() {
        let (mut app, fake, _) = setup();
        fake.err(Error::worker("Invalid filter"));
        assert!(app.perform("/api/apply", &json!({})).is_err());
        assert_eq!(app.live_state, "unchanged");
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn offline_mode_blocks_device_access() {
        let (mut app, fake, _) = setup();
        app.enable_live = false;
        for path in ["/api/read_device", "/api/apply", "/api/restore"] {
            assert!(matches!(app.perform(path, &json!({})), Err(Error::Value(_))), "{path}");
        }
        assert!(fake.calls().is_empty());
    }
}

mod persistent {
    use super::*;

    struct Setup {
        temp: Temp,
        path: PathBuf,
        app: App,
        fake: Fake,
        original: String,
        modified: String,
        snapshot: Value,
        preset: Value,
    }

    impl Setup {
        fn new() -> Self {
            let temp = Temp::new();
            let path = temp.0.join("session.json");
            let (original, modified) = ("11".repeat(844), "22".repeat(844));
            let snapshot = json!({"device": {"path": "test-headset", "transport": "usb"}, "index": 7, "slot": 0,
                                  "raw_ui_hex": "aa", "saved_coefficients_hex": original});
            let fake = Fake::default();
            let mut app = App::new(true, Some(path.clone()), quiet(), fake.runner()).unwrap();
            app.backup = json!({"state": snapshot});
            app.backup_path = json!(temp.0.join("original.json").display().to_string());
            app.expected = snapshot.clone();
            let preset =
                json!({"filters": [{"type": "peak", "frequency": 1000, "gain": -3, "q": 1}], "preamp": "auto"});
            Setup { temp, path, app, fake, original, modified, snapshot, preset }
        }

        fn saved(&self, blob: Option<&str>) -> Value {
            let mut state = self.snapshot.clone();
            state["saved_coefficients_hex"] = json!(blob.unwrap_or(&self.modified));
            json!({"saved_verified": true, "live_applied": true, "state": state})
        }

        fn reopen(&self) -> Result<App> {
            App::new(true, Some(self.path.clone()), quiet(), Fake::default().runner())
        }

        fn audits(&self, suffix: &str) -> usize {
            std::fs::read_dir(&self.temp.0)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(suffix))
                .count()
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn save_journal_precedes_write_and_original_survives_restart() {
        let mut s = Setup::new();
        let (path, modified, snapshot, saved) = (s.path.clone(), s.modified.clone(), s.snapshot.clone(), s.saved(None));
        s.fake.ok(json!({"blob_hex": modified})).reply(move |worker, request| {
            assert_eq!(worker, Worker::Device);
            let pending = read_json(&path);
            assert_eq!(pending["storage_state"], "uncertain");
            assert_eq!(pending["pending"]["blob_hex"], json!(modified));
            assert_eq!(pending["backup"]["state"], snapshot);
            assert_eq!(request["operation"], "save");
            Ok(saved)
        });
        s.app.perform("/api/save", &s.preset).unwrap();
        let restored = s.reopen().unwrap();
        assert!(restored.session_recovered);
        assert_eq!(restored.storage_state, "saved");
        assert_eq!(restored.expected["saved_coefficients_hex"], json!(s.modified));
        assert_eq!(restored.backup["state"], s.snapshot);
        assert_eq!(restored.last_profile, s.preset);
        assert!(restored.pending.is_null());
        assert_eq!(s.audits("-save.json"), 1);
    }

    #[test]
    fn repeat_save_uses_current_expected_and_keeps_original() {
        let mut s = Setup::new();
        s.fake.ok(json!({"blob_hex": s.modified})).ok(s.saved(None));
        s.app.perform("/api/save", &s.preset).unwrap();
        s.fake.calls();
        s.fake.ok(json!({"blob_hex": s.modified})).ok(s.saved(None));
        s.app.perform("/api/save", &s.preset).unwrap();
        let calls = s.fake.calls();
        assert_eq!(calls[1].1["expected"]["saved_coefficients_hex"], json!(s.modified));
        assert_eq!(s.app.backup["state"]["saved_coefficients_hex"], json!(s.original));
    }

    #[test]
    fn timeout_recovers_and_compensating_restore_needs_no_compiler() {
        let mut s = Setup::new();
        s.fake.ok(json!({"blob_hex": s.modified})).err(Error::other("timeout"));
        assert_eq!(s.app.perform("/api/save", &s.preset).unwrap_err().to_string(), "timeout");

        let fake = Fake::default();
        let mut recovered = App::new(true, Some(s.path.clone()), quiet(), fake.runner()).unwrap();
        assert_eq!(recovered.storage_state, "uncertain");
        assert_eq!(recovered.live_state, "uncertain");
        for endpoint in ["/api/apply", "/api/save", "/api/read_device", "/api/restore"] {
            assert!(matches!(recovered.perform(endpoint, &json!({})), Err(Error::Value(_))), "{endpoint}");
        }
        assert!(fake.calls().is_empty());

        fake.ok(s.saved(Some(&s.original)));
        recovered.perform("/api/restore_saved", &json!({})).unwrap();
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "restore must not compile");
        assert_eq!(calls[0].0, Worker::Device);
        assert_eq!(calls[0].1["blob_hex"], json!(s.original));
        assert_eq!(calls[0].1["operation"], "restore_saved");
        assert_eq!(recovered.storage_state, "unchanged");
        assert_eq!(recovered.live_state, "unchanged");
        assert!(!recovered.backup.is_null());
    }

    #[test]
    fn readback_mismatch_is_uncertain_and_keeps_backup() {
        let mut s = Setup::new();
        s.fake.ok(json!({"blob_hex": s.modified})).err(worker_error(json!({
            "error": "Readback mismatch", "write_started": true,
            "persistent_write_started": true, "saved_verified": false})));
        assert!(matches!(s.app.perform("/api/save", &s.preset), Err(Error::Worker(_))));
        assert_eq!(s.app.storage_state, "uncertain");
        assert_eq!(s.app.expected, s.snapshot);
        assert_eq!(s.reopen().unwrap().backup["state"], s.snapshot);
    }

    #[test]
    fn verified_storage_survives_live_failure() {
        let mut s = Setup::new();
        s.fake.ok(json!({"blob_hex": s.modified})).err(worker_error(json!({
            "error": "Live ACK timeout", "write_started": true, "persistent_write_started": true,
            "saved_verified": true, "state": s.saved(None)["state"]})));
        let error = s.app.perform("/api/save", &s.preset).unwrap_err();
        assert!(error.to_string().contains("Saved coefficients were read back"), "{error}");
        let recovered = s.reopen().unwrap();
        assert_eq!(recovered.storage_state, "saved");
        assert_eq!(recovered.live_state, "uncertain");
        assert_eq!(recovered.expected["saved_coefficients_hex"], json!(s.modified));
    }

    #[test]
    fn unverified_save_result_is_uncertain() {
        let mut s = Setup::new();
        let mut result = s.saved(None);
        result["saved_verified"] = json!(false);
        s.fake.ok(json!({"blob_hex": s.modified})).ok(result);
        assert!(matches!(s.app.perform("/api/save", &s.preset), Err(Error::Other(_))));
        assert_eq!(s.app.storage_state, "uncertain");
    }

    #[test]
    fn preflight_failure_preserves_prior_states() {
        let mut s = Setup::new();
        (s.app.storage_state, s.app.live_state) = ("saved".into(), "applied".into());
        s.fake.err(worker_error(json!({"error": "Disconnected", "write_started": false,
                                       "persistent_write_started": false, "state_changed": true})));
        assert!(s.app.perform("/api/restore_saved", &json!({})).is_err());
        assert_eq!(s.app.storage_state, "saved");
        assert_eq!(s.app.live_state, "applied");
        assert!(!s.app.backup.is_null());
    }

    #[test]
    fn journal_failure_prevents_device_write() {
        let mut s = Setup::new();
        // A regular file where the session's directory should be makes every journal write fail.
        let blocker = s.temp.0.join("blocker");
        std::fs::write(&blocker, "").unwrap();
        let fake = Fake::default();
        let mut app = App::new(true, Some(blocker.join("session.json")), quiet(), fake.runner()).unwrap();
        (app.backup, app.expected) = (s.app.backup.clone(), s.app.expected.clone());
        fake.ok(json!({"blob_hex": s.modified}));
        assert!(matches!(app.perform("/api/save", &s.preset), Err(Error::Other(_))));
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Worker::Compile);
        assert_eq!(app.storage_state, "unchanged");
        assert_eq!(app.live_state, "unchanged");
        assert!(app.pending.is_null());
        s.app.storage_state = "unchanged".into();
    }

    #[test]
    fn dongle_route_is_explicit_and_preserved() {
        let mut s = Setup::new();
        let mut state = s.snapshot.clone();
        state["device"]["transport"] = json!("dongle");
        s.fake.ok(state);
        s.app.perform("/api/read_device", &json!({"transport": "dongle"})).unwrap();
        assert_eq!(s.fake.calls()[0].1, json!({"operation": "read", "transport": "dongle"}));
        assert_eq!(s.audits("-slot1.json"), 1);

        let fake = Fake::default();
        let mut recovered = App::new(true, Some(s.path.clone()), quiet(), fake.runner()).unwrap();
        fake.ok(json!({}));
        recovered.perform("/api/restore", &json!({})).unwrap();
        assert_eq!(fake.calls()[0].1["transport"], "dongle");
    }

    #[test]
    fn corrupt_recovery_file_fails_closed() {
        let s = Setup::new();
        std::fs::write(&s.path, json!({"version": 1, "live_state": "uncertain"}).to_string()).unwrap();
        let error = s.reopen().err().expect("must refuse");
        assert!(error.to_string().contains("Invalid recovery"), "{error}");
    }

    #[test]
    fn python_session_file_is_recovered() {
        // As server.py wrote it: indented, with a BOM-free UTF-8 body and Python key order.
        let s = Setup::new();
        let session = json!({"version": 1, "backup": {"state": s.snapshot}, "backup_path": "x.json",
                             "expected": s.snapshot, "transport": "usb", "device": "Original Maxwell PS · USB-C · preset 7",
                             "live_state": "applied", "storage_state": "unchanged", "pending": null,
                             "last_profile": s.preset, "saved_profile": null});
        std::fs::write(&s.path, serde_json::to_string_pretty(&session).unwrap()).unwrap();
        let app = s.reopen().unwrap();
        assert_eq!(app.live_state, "applied");
        assert_eq!(app.status()["last_profile"], s.preset);
    }

    #[test]
    fn offline_mode_blocks_storage_writes() {
        let mut s = Setup::new();
        s.app.enable_live = false;
        for endpoint in ["/api/save", "/api/restore_saved"] {
            assert!(matches!(s.app.perform(endpoint, &json!({})), Err(Error::Value(_))), "{endpoint}");
        }
        assert!(s.fake.calls().is_empty());
    }
}

mod audeze_guard {
    use super::*;

    #[test]
    fn device_operations_refuse_while_audeze_runs() {
        let fake = Fake::default();
        let watch =
            Arc::new(AudezeWatch::new(Box::new(|| Some(vec!["Audeze HQ.exe".into()])), Duration::from_secs(60)));
        let mut app = App::new(true, None, watch, fake.runner()).unwrap();
        let error = app.perform("/api/read_device", &json!({})).unwrap_err();
        assert!(error.to_string().contains("Audeze App/HQ is open"), "{error}");
        assert!(fake.calls().is_empty());
        assert_eq!(app.status()["audeze_running"], true);
        assert_eq!(app.status()["audeze_apps"], json!(["Audeze HQ.exe"]));
    }

    #[test]
    fn status_reports_false_when_no_audeze_app_runs() {
        let app = App::new(true, None, quiet(), Fake::default().runner()).unwrap();
        assert_eq!(app.status()["audeze_running"], false);
        assert_eq!(app.status()["audeze_apps"], json!([]));
    }

    #[test]
    fn unsupported_check_is_not_reported_as_running() {
        let watch = Arc::new(AudezeWatch::new(Box::new(|| None), Duration::from_secs(60)));
        let app = App::new(true, None, watch, Fake::default().runner()).unwrap();
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
