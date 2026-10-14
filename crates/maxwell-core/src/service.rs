//! The editor's API, shared by the desktop window and the `--serve` dev server.
//!
//! Requests that never touch the headset run immediately; device requests take
//! the device lock and are refused while another one is running. Shortcut
//! keys, the tray and scripts wait for the lock instead, and the tray's
//! battery poll never makes anyone wait more than one short read.

use crate::app::App;
use crate::audeze::{self, AudezeWatch};
use crate::json::{get, get_or_null, object};
use crate::profiles::{Profiles, check_preset};
use crate::traffic::Traffic;
use crate::worker::Runner;
use crate::{Result, apo};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

/// A refused request: an HTTP-style status and the message for the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: u16,
    pub message: String,
}

impl Failure {
    pub fn body(&self) -> Value {
        json!({"error": self.message})
    }
}

impl From<crate::Error> for Failure {
    fn from(error: crate::Error) -> Self {
        Failure { status: 400, message: error.to_string() }
    }
}

pub struct Service {
    pub app: Mutex<App>,
    pub profiles: Mutex<Profiles>,
    pub audeze: Arc<AudezeWatch>,
    pub token: String,
    pub data_dir: Option<PathBuf>,
    /// The raw reports device workers exchange with the headset.
    pub traffic: Arc<Traffic>,
    /// Set while the tray's poll holds the device lock, so page requests wait
    /// for it instead of being refused.
    background: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Service {
    /// Load profiles from `data_dir`. `traffic` is the log `runner` writes to.
    pub fn new(
        enable_live: bool,
        data_dir: Option<PathBuf>,
        audeze: Arc<AudezeWatch>,
        runner: Runner,
        traffic: Arc<Traffic>,
    ) -> Result<Self> {
        let app = App::new(enable_live, data_dir.clone(), audeze.clone(), runner);
        let profiles = Profiles::load(data_dir.as_deref().map(crate::paths::profiles_path))?;
        Ok(Service {
            token: app.token.clone(),
            app: Mutex::new(app),
            profiles: Mutex::new(profiles),
            audeze,
            data_dir,
            traffic,
            background: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> Value {
        // Refresh the cached process check before waiting for the device lock.
        self.audeze.running(false);
        lock(&self.app).status()
    }

    pub fn profiles(&self) -> Value {
        lock(&self.profiles).listing()
    }

    fn local(&self, path: &str, request: &Value) -> Option<Result<Value>> {
        Some(match path {
            "/api/compile" => crate::worker::compile(request),
            // Never waits for the device lock, so the panel updates while a write runs.
            "/api/traffic" => Ok(self.traffic.since(get(request, "since").and_then(Value::as_u64).unwrap_or(0))),
            "/api/traffic/clear" => {
                self.traffic.clear();
                Ok(json!({}))
            }
            _ if path.starts_with("/api/profiles/") => lock(&self.profiles).perform(path, request),
            "/api/apo/parse" => apo::parse(get_or_null(request, "text")),
            "/api/apo/export" => check_preset(get(request, "preset"))
                .and_then(|preset| apo::export(&preset, get_or_null(request, "preamp_db")))
                .map(|text| object([("text", json!(text))])),
            "/api/close_audeze" => {
                let still = audeze::close();
                self.audeze.reset();
                if still.is_empty() {
                    let running = self.audeze.running(true).is_some_and(|apps| !apps.is_empty());
                    Ok(object([("audeze_running", json!(running))]))
                } else {
                    Err(crate::Error::value(format!(
                        "{} is still running. Quit it from the tray icon.",
                        still.join(", ")
                    )))
                }
            }
            _ => return None,
        })
    }

    /// One POST endpoint. `request` must already be a JSON object.
    pub fn post(&self, path: &str, request: &Value) -> Result<Value, Failure> {
        self.post_with(path, request, false)
    }

    /// Like [`Service::post`], but a device request waits for the one running
    /// instead of being refused.
    pub fn post_wait(&self, path: &str, request: &Value) -> Result<Value, Failure> {
        self.post_with(path, request, true)
    }

    /// A device request that gives way to everything else: `None` when another
    /// one is running. Requests that arrive meanwhile wait for it to finish.
    pub fn post_background(&self, path: &str, request: &Value) -> Option<Result<Value, Failure>> {
        self.background.store(true, Ordering::SeqCst);
        let mut app = match self.app.try_lock() {
            Ok(app) => app,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.background.store(false, Ordering::SeqCst);
                return None;
            }
        };
        let result = app.perform(path, request).map_err(Failure::from);
        self.background.store(false, Ordering::SeqCst);
        drop(app);
        Some(result)
    }

    /// A command from a script, a shortcut key or the tray:
    /// `{"command": "preset", "value": 1..4 | "next" | "previous"}`, or
    /// `{"command": "status"}`, which reads the headset and returns [`summary`].
    pub fn command(&self, request: &Value) -> Result<Value, Failure> {
        match get(request, "command").and_then(Value::as_str) {
            Some("preset") => {
                self.post_wait("/api/switch", &object([("preset", get_or_null(request, "value").clone())]))
            }
            Some("status") => {
                // A failed read is part of the answer.
                let _ = self.post_wait("/api/glance", &json!({}));
                Ok(summary(&self.status()))
            }
            _ => Err(Failure { status: 400, message: "Unknown command.".into() }),
        }
    }

    fn post_with(&self, path: &str, request: &Value, wait: bool) -> Result<Value, Failure> {
        if !request.is_object() {
            return Err(Failure { status: 400, message: "Expected a JSON object.".into() });
        }
        if let Some(result) = self.local(path, request) {
            return result.map_err(Failure::from);
        }
        let mut app = match self.app.try_lock() {
            Ok(app) => app,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) if wait || self.background.load(Ordering::SeqCst) => lock(&self.app),
            Err(TryLockError::WouldBlock) => {
                return Err(Failure {
                    status: 409,
                    message: "Another operation is running; wait for it to finish.".into(),
                });
            }
        };
        app.perform(path, request).map_err(Failure::from)
    }
}

/// What the tray and `MaxwellPEQ --status` show, from [`Service::status`]:
/// battery, mic mute, the active preset and the profile saved there.
pub fn summary(status: &Value) -> Value {
    let glance = get_or_null(status, "glance");
    let index = get_or_null(status, "index").as_u64();
    let slot = index.and_then(|index| index.checked_sub(7)).filter(|slot| *slot < 4);
    let profile = slot.map_or(Value::Null, |slot| get_or_null(&status["slots"][slot as usize], "name").clone());
    let preset_name = index
        .and_then(|index| crate::controls::PRESETS.get(usize::try_from(index).ok()?.checked_sub(1)?))
        .map_or(Value::Null, |name| json!(name));
    object([
        ("offline", json!(!status["live_enabled"].as_bool().unwrap_or(false))),
        ("audeze_running", json!(status["audeze_running"].as_bool().unwrap_or(false))),
        ("device", if status["device_read"] == json!(true) { status["device"].clone() } else { Value::Null }),
        ("battery", get_or_null(glance, "battery").clone()),
        ("mic_muted", get_or_null(glance, "mic_muted").clone()),
        ("preset", index.map_or(Value::Null, |index| json!(index))),
        ("preset_name", preset_name),
        ("slot", slot.map_or(Value::Null, |slot| json!(slot + 1))),
        ("profile", profile),
        ("profiles", Value::Array((0..4).map(|slot| get_or_null(&status["slots"][slot], "name").clone()).collect())),
        ("error", get_or_null(glance, "error").clone()),
    ])
}
