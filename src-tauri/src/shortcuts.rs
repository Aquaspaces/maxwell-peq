//! Global shortcut keys that switch the headset's EQ from any app.
//!
//! A shortcut never shows anything on screen: a popup could take focus from
//! a full-screen game. The tray icon's menu and tooltip show the result.

use maxwell_core::service::Service;
use maxwell_core::settings::{self, SHORTCUTS, shortcut_target};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

/// Why each shortcut that isn't working failed to register.
#[derive(Default)]
pub struct Problems(Mutex<HashMap<String, String>>);

pub fn data(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.state::<Arc<Service>>().data_dir.clone()
}

/// Every action with its keys, as saved.
pub fn current(app: &AppHandle) -> Vec<(String, String)> {
    settings::shortcuts(data(app).as_deref())
}

fn label(id: &str) -> &'static str {
    SHORTCUTS.iter().find(|(action, _, _)| *action == id).map_or("another action", |(_, label, _)| label)
}

/// Function and media keys may stand alone; anything else needs a modifier,
/// or it would take that key away from typing everywhere.
fn stands_alone(shortcut: &Shortcut) -> bool {
    use Code::*;
    matches!(
        shortcut.key,
        F1 | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | MediaPlayPause
            | MediaTrackNext
            | MediaTrackPrevious
    )
}

/// Register every saved shortcut, replacing the ones registered before.
pub fn register(app: &AppHandle) {
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();
    let mut problems = HashMap::new();
    let mut taken: HashMap<u32, String> = HashMap::new();
    for (id, keys) in current(app) {
        let (Some(target), false) = (shortcut_target(&id), keys.is_empty()) else { continue };
        let shortcut = match keys.parse::<Shortcut>() {
            Ok(shortcut) => shortcut,
            Err(_) => {
                problems.insert(id, "These keys can't be used as a shortcut.".to_owned());
                continue;
            }
        };
        if shortcut.mods.is_empty() && !stands_alone(&shortcut) {
            problems.insert(id, "Add Ctrl, Alt, Shift or Win.".to_owned());
            continue;
        }
        if let Some(other) = taken.get(&shortcut.id()) {
            problems.insert(id, format!("{} already uses these keys.", label(other)));
            continue;
        }
        taken.insert(shortcut.id(), id.clone());
        let value = json!(target);
        let registered = manager.on_shortcut(shortcut, move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                crate::tray::run(app, json!({"command": "preset", "value": value}));
            }
        });
        if registered.is_err() {
            problems.insert(id, "Another app already uses these keys.".to_owned());
        }
    }
    *app.state::<Problems>().0.lock().unwrap_or_else(|e| e.into_inner()) = problems;
}

/// Let the editor record keys that are registered now: while paused, no
/// shortcut is registered, so its keys reach the page.
pub fn pause(app: &AppHandle, paused: bool) {
    if paused {
        let _ = app.global_shortcut().unregister_all();
    } else {
        register(app);
    }
}

/// The shortcuts for the editor's dialog: `{actions: [{id, label, keys, problem}]}`.
pub fn listing(app: &AppHandle) -> Value {
    let problems = app.state::<Problems>().0.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let saved = current(app);
    let actions: Vec<Value> = SHORTCUTS
        .iter()
        .map(|(id, label, default)| {
            let keys = saved.iter().find(|(action, _)| action == id).map_or("", |(_, keys)| keys.as_str());
            json!({"id": id, "label": label, "keys": keys, "default": default, "problem": problems.get(*id)})
        })
        .collect();
    json!({"actions": actions})
}

/// Save `{action: keys}` (missing actions keep their keys), register, and list.
pub fn save(app: &AppHandle, changes: &Value) -> Result<Value, String> {
    let changes = changes.as_object().ok_or("Expected a JSON object.")?;
    let mut shortcuts = current(app);
    for (id, keys) in &mut shortcuts {
        if let Some(value) = changes.get(id.as_str()) {
            *keys = value.as_str().ok_or("Each shortcut must be text.")?.trim().to_owned();
        }
    }
    settings::save_shortcuts(data(app).as_deref(), &shortcuts).map_err(|error| error.to_string())?;
    register(app);
    crate::tray::update(app);
    Ok(listing(app))
}
