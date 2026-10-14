//! The editor's Settings: start at login, the tray icon, closing and
//! minimizing to the tray, and checking for updates.
//!
//! Starting at login belongs to the system (the autostart plugin); the rest
//! are kept in settings.json and held here so window events can read them.

use crate::{shortcuts, tray, updates};
use maxwell_core::settings;
use serde_json::{Map, Value, json};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

pub struct Preferences(Mutex<Map<String, Value>>);

pub fn setup(app: &AppHandle) {
    app.manage(Preferences(Mutex::new(settings::preferences(shortcuts::data(app).as_deref()))));
}

pub fn get(app: &AppHandle, name: &str) -> bool {
    app.try_state::<Preferences>().is_some_and(|preferences| {
        preferences.0.lock().unwrap_or_else(|e| e.into_inner()).get(name).and_then(Value::as_bool).unwrap_or(false)
    })
}

/// The editor keeps running in the tray when its window closes.
pub fn keep_running(app: &AppHandle) -> bool {
    get(app, "tray_icon") && get(app, "close_to_tray")
}

/// Minimizing the editor hides it in the tray instead.
pub fn minimize_to_tray(app: &AppHandle) -> bool {
    get(app, "tray_icon") && get(app, "minimize_to_tray")
}

/// Every setting, with whether the app starts at login.
pub fn listing(app: &AppHandle) -> Value {
    let mut listing = app.state::<Preferences>().0.lock().unwrap_or_else(|e| e.into_inner()).clone();
    listing.insert("start_at_login".into(), json!(app.autolaunch().is_enabled().unwrap_or(false)));
    listing.insert("version".into(), json!(app.package_info().version.to_string()));
    Value::Object(listing)
}

/// Apply and save `changes`; returns every setting.
pub fn set(app: &AppHandle, changes: &Value) -> Result<Value, String> {
    if let Some(start) = changes.get("start_at_login").and_then(Value::as_bool) {
        let autolaunch = app.autolaunch();
        let result = if start { autolaunch.enable() } else { autolaunch.disable() };
        result.map_err(|error| format!("Could not change the login item: {error}"))?;
    }
    let saved =
        settings::save_preferences(shortcuts::data(app).as_deref(), changes).map_err(|error| error.to_string())?;
    let checking_turned_on = changes.get("check_updates") == Some(&json!(true));
    *app.state::<Preferences>().0.lock().unwrap_or_else(|e| e.into_inner()) = saved;
    tray::show(app, get(app, "tray_icon"));
    if checking_turned_on {
        updates::wake(app);
    }
    tray::update(app);
    Ok(listing(app))
}
