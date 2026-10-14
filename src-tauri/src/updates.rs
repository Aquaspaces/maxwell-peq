//! Checking GitHub for a newer release, with the notes of recent ones.
//!
//! A background thread checks shortly after start and then once a day while
//! the setting is on. A new release never pops anything up: the tray menu
//! gains an item for it and the editor's Settings show it.

use crate::{preferences, tray};
use maxwell_core::updates::{self, RELEASES_URL};
use serde_json::{Value, json};
use std::sync::{Mutex, mpsc};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const FIRST_CHECK: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Updates {
    wake: Mutex<mpsc::Sender<()>>,
    /// The last check's result, or why it failed.
    last: Mutex<Option<Value>>,
}

pub fn setup(app: &AppHandle) {
    let (wake, woken) = mpsc::channel();
    app.manage(Updates { wake: Mutex::new(wake), last: Mutex::new(None) });
    let app = app.clone();
    std::thread::spawn(move || {
        let mut wait = FIRST_CHECK;
        loop {
            if let Err(mpsc::RecvTimeoutError::Disconnected) = woken.recv_timeout(wait) {
                return;
            }
            while woken.try_recv().is_ok() {}
            if preferences::get(&app, "check_updates") {
                check(&app);
            }
            wait = EVERY;
        }
    });
}

/// Check now on the background thread, e.g. when the setting is turned on.
pub fn wake(app: &AppHandle) {
    if let Some(updates) = app.try_state::<Updates>() {
        let _ = updates.wake.lock().unwrap_or_else(|e| e.into_inner()).send(());
    }
}

fn fetch(current: &str) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(format!("MaxwellPEQ/{current}"))
        .build()
        .into();
    let text = agent
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|mut reply| reply.body_mut().read_to_string())
        .map_err(|error| format!("Could not reach GitHub: {error}"))?;
    let reply =
        serde_json::from_str(&text).map_err(|_| "GitHub sent something that isn't a release list.".to_owned())?;
    Ok(updates::read(current, &reply))
}

/// Ask GitHub now; blocks for up to 15 seconds. Returns what [`last`] will.
pub fn check(app: &AppHandle) -> Value {
    let current = app.package_info().version.to_string();
    let mut result = fetch(&current).unwrap_or_else(|error| json!({"current": current, "error": error}));
    result["checked_at"] =
        json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs()));
    if let Some(updates) = app.try_state::<Updates>() {
        *updates.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(result.clone());
    }
    let _ = app.emit_to("main", "updates-changed", result.clone());
    tray::update(app);
    result
}

/// The last check's result; `null` before the first one.
pub fn last(app: &AppHandle) -> Value {
    app.try_state::<Updates>()
        .and_then(|updates| updates.last.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .unwrap_or(Value::Null)
}

/// The newer release's version and page, if the last check found one.
pub fn available(app: &AppHandle) -> Option<(String, String)> {
    let last = last(app);
    (last["update_available"] == json!(true)).then(|| {
        (last["latest"].as_str().unwrap_or_default().to_owned(), last["url"].as_str().unwrap_or_default().to_owned())
    })
}

/// Open a release page in the browser. Only pages under the repo's releases.
pub fn open(app: &AppHandle, url: &str) -> Result<(), String> {
    if !updates::is_release_page(url) {
        return Err("That isn't a Maxwell PEQ release page.".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|error| format!("Could not open the browser: {error}"))
}
