//! The editor's API, shared by the desktop window and the `--serve` dev server.
//!
//! Requests that never touch the headset run immediately; device requests take
//! the device lock and are refused while another one is running.

use crate::app::App;
use crate::audeze::{self, AudezeWatch};
use crate::json::{get, get_or_null, object};
use crate::profiles::{Profiles, check_preset};
use crate::worker::Runner;
use crate::{Result, apo};
use serde_json::{Value, json};
use std::path::PathBuf;
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
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Service {
    /// Load profiles and recover any interrupted session from `data_dir`.
    pub fn new(enable_live: bool, data_dir: Option<PathBuf>, audeze: Arc<AudezeWatch>, runner: Runner) -> Result<Self> {
        let app = App::new(enable_live, data_dir.as_deref().map(crate::paths::session_path), audeze.clone(), runner)?;
        let profiles = Profiles::load(data_dir.as_deref().map(crate::paths::profiles_path))?;
        Ok(Service { token: app.token.clone(), app: Mutex::new(app), profiles: Mutex::new(profiles), audeze, data_dir })
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
        if !request.is_object() {
            return Err(Failure { status: 400, message: "Expected a JSON object.".into() });
        }
        if let Some(result) = self.local(path, request) {
            return result.map_err(Failure::from);
        }
        let mut app = match self.app.try_lock() {
            Ok(app) => app,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
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
