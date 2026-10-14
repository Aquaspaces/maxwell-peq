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

mod switching {
    use super::*;
    use maxwell_core::service::{Service, summary};

    fn state(index: u64) -> Value {
        json!({"index": index, "slot": index.checked_sub(7), "saved_coefficients_hex": "1122"})
    }

    #[test]
    fn switch_picks_a_preset_by_number_or_step() {
        let fake = Fake::default();
        let mut app = App::new(true, None, quiet(), fake.runner());
        fake.ok(json!({"state": state(8), "selected": true}));
        let result = app.perform("/api/switch", &json!({"preset": "2"})).unwrap();
        assert_eq!(result["message"], "Preset 2 is now active.");
        assert_eq!(fake.calls()[0].1["index"], 8);
        fake.ok(json!({"state": state(10), "selected": true}));
        app.perform("/api/switch", &json!({"preset": 4})).unwrap();
        assert_eq!(fake.calls()[0].1["index"], 10);
        for (preset, step) in [("next", 1), ("Previous", -1), ("prev", -1)] {
            fake.ok(json!({"state": state(9), "selected": true}));
            app.perform("/api/switch", &json!({"preset": preset})).unwrap();
            let call = &fake.calls()[0].1;
            assert_eq!((call["operation"].clone(), call["step"].clone()), (json!("select"), json!(step)), "{preset}");
            assert!(call.get("index").is_none());
        }
        for preset in [json!("0"), json!("5"), json!("up"), Value::Null] {
            assert!(app.perform("/api/switch", &json!({"preset": preset})).is_err(), "{preset}");
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_glance_keeps_the_active_preset_in_step() {
        let fake = Fake::default();
        let mut app = App::new(true, None, quiet(), fake.runner());
        let reading = json!({"preset": {"index": 9, "name": "EQ 3", "custom": true}, "mic_muted": true,
                             "battery": 64, "transport": "dongle"});
        fake.ok(reading.clone());
        app.perform("/api/glance", &json!({})).unwrap();
        assert_eq!(fake.calls()[0].1["operation"], "glance");
        let status = app.status();
        assert_eq!(status["index"], 9);
        assert_eq!(status["glance"], reading);
        fake.err(Error::other("The headset is off."));
        assert!(app.perform("/api/glance", &json!({})).is_err());
        assert_eq!(app.status()["glance"], json!({"error": "The headset is off."}));
        assert_eq!(app.status()["index"], 9, "a failed read forgets nothing else");
    }

    #[test]
    fn commands_switch_and_summarize() {
        let fake = Fake::default();
        let service = Service::new(true, None, quiet(), fake.runner()).unwrap();
        fake.ok(json!({"state": state(7), "selected": true}));
        let result = service.command(&json!({"command": "preset", "value": "1"})).unwrap();
        assert_eq!(result["message"], "Preset 1 is now active.");
        fake.calls();
        fake.ok(json!({"preset": {"index": 7}, "mic_muted": false, "battery": 12, "transport": "usb"}));
        let summary = service.command(&json!({"command": "status"})).unwrap();
        assert_eq!(summary["battery"], 12);
        assert_eq!(summary["mic_muted"], false);
        assert_eq!(summary["preset"], 7);
        assert_eq!(summary["preset_name"], "EQ 1");
        assert_eq!(summary["slot"], 1);
        assert_eq!(summary["offline"], false);
        assert_eq!(summary["profiles"].as_array().unwrap().len(), 4);
        // A failed read is reported, not refused.
        fake.err(Error::other("No headset found."));
        let summary = service.command(&json!({"command": "status"})).unwrap();
        assert_eq!(summary["error"], "No headset found.");
        assert_eq!(service.command(&json!({"command": "reboot"})).unwrap_err().status, 400);
    }

    #[test]
    fn a_summary_of_a_built_in_preset_has_no_slot() {
        let status = json!({"live_enabled": false, "index": 3, "device_read": false, "device": "x",
                            "slots": [{"name": "Bass"}, {}, {}, {}], "glance": {}});
        let summary = summary(&status);
        assert_eq!(summary["offline"], true);
        assert_eq!((summary["slot"].clone(), summary["profile"].clone()), (Value::Null, Value::Null));
        assert_eq!(summary["device"], Value::Null);
        assert_eq!(summary["profiles"], json!(["Bass", null, null, null]));
        assert_eq!(summary["battery"], Value::Null);
    }

    #[test]
    fn a_background_read_gives_way_to_a_running_request() {
        let fake = Fake::default();
        let service = Arc::new(Service::new(true, None, quiet(), fake.runner()).unwrap());
        let (started, release) = (Arc::new(std::sync::Barrier::new(2)), Arc::new(std::sync::Barrier::new(2)));
        let (s, r) = (started.clone(), release.clone());
        fake.reply(move |_, _| {
            s.wait();
            r.wait();
            Ok(json!({"state": state(8), "selected": true}))
        });
        let busy = service.clone();
        let switch = std::thread::spawn(move || busy.command(&json!({"command": "preset", "value": 2})));
        started.wait();
        assert!(service.post_background("/api/glance", &json!({})).is_none());
        release.wait();
        assert!(switch.join().unwrap().is_ok());
        fake.ok(json!({"preset": {"index": 8}, "battery": 50}));
        assert!(service.post_background("/api/glance", &json!({})).unwrap().is_ok());
    }
}

mod settings_and_remote {
    use super::*;
    use maxwell_core::{remote, settings};
    use std::io::{BufRead, Write};

    #[test]
    fn shortcuts_default_then_save_and_load() {
        let temp = Temp::new();
        let defaults = settings::shortcuts(Some(&temp.0));
        assert_eq!(defaults[0], ("preset1".into(), "Ctrl+Alt+1".into()));
        assert_eq!(defaults.len(), settings::SHORTCUTS.len());
        let mut changed = defaults.clone();
        changed[4].1 = "Ctrl+Shift+F9".into();
        changed[0].1 = String::new();
        settings::save_shortcuts(Some(&temp.0), &changed).unwrap();
        assert_eq!(settings::shortcuts(Some(&temp.0)), changed);
        assert_eq!(settings::shortcuts(None), defaults);
        settings::save_shortcuts(None, &changed).unwrap();
    }

    #[test]
    fn shortcut_actions_map_to_preset_commands() {
        assert_eq!(settings::shortcut_target("preset3"), Some("3"));
        assert_eq!(settings::shortcut_target("next"), Some("next"));
        assert_eq!(settings::shortcut_target("previous"), Some("previous"));
        assert_eq!(settings::shortcut_target("preset5"), None);
        assert_eq!(settings::shortcut_target("quit"), None);
        for (id, _, _) in settings::SHORTCUTS {
            assert!(settings::shortcut_target(id).is_some(), "{id}");
        }
    }

    #[test]
    fn remote_commands_need_the_token() {
        let temp = Temp::new();
        remote::listen(
            &temp.0,
            Arc::new(|request: &Value| match request["command"].as_str() {
                Some("echo") => json!({"value": request["value"]}),
                _ => json!({"error": "Unknown command."}),
            }),
        )
        .unwrap();
        let reply = remote::send(&temp.0, &json!({"command": "echo", "value": 3})).unwrap();
        assert_eq!(reply["value"], 3);
        let error = remote::send(&temp.0, &json!({"command": "nope"})).unwrap_err().to_string();
        assert!(error.contains("Unknown command."), "{error}");
        // Without the published token the command is refused.
        let text = std::fs::read_to_string(maxwell_core::paths::remote_path(&temp.0)).unwrap();
        let port = serde_json::from_str::<Value>(&text).unwrap()["port"].as_u64().unwrap() as u16;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(stream, "{}", json!({"command": "echo", "value": 1, "token": "wrong"})).unwrap();
        let mut line = String::new();
        std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
        assert!(line.contains("wrong token"), "{line}");
    }

    #[test]
    fn sending_without_a_running_app_fails() {
        let temp = Temp::new();
        assert!(remote::send(&temp.0, &json!({"command": "status"})).is_err());
    }
}

mod preferences {
    use super::*;
    use maxwell_core::settings;

    #[test]
    fn preferences_default_then_save_only_known_names() {
        let temp = Temp::new();
        let defaults = settings::preferences(Some(&temp.0));
        assert_eq!(defaults.len(), settings::PREFERENCES.len());
        assert!(settings::preference(Some(&temp.0), "tray_icon"));
        assert!(!settings::preference(Some(&temp.0), "minimize_to_tray"));
        assert!(!settings::preference(Some(&temp.0), "nonsense"));
        settings::save_shortcuts(Some(&temp.0), &settings::shortcuts(None)).unwrap();
        let saved = settings::save_preferences(
            Some(&temp.0),
            &json!({"minimize_to_tray": true, "tray_icon": "yes", "nonsense": true}),
        )
        .unwrap();
        assert_eq!(saved["minimize_to_tray"], true);
        assert_eq!(saved["tray_icon"], true, "a value that isn't true or false is ignored");
        assert!(saved.get("nonsense").is_none());
        assert_eq!(settings::preferences(Some(&temp.0)), saved);
        assert_eq!(settings::shortcuts(Some(&temp.0)), settings::shortcuts(None), "shortcuts are kept");
        assert!(settings::save_preferences(None, &json!({"check_updates": false})).unwrap()["check_updates"] == false);
    }
}
