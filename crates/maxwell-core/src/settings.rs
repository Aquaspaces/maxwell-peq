//! App settings that aren't profiles: the global shortcut keys and the
//! desktop app's preferences.
//!
//! Shortcuts are stored as accelerator text such as `Ctrl+Alt+1`; the desktop
//! app checks and registers them.

use crate::Result;
use crate::util::atomic_json;
use serde_json::{Map, Value, json};
use std::path::Path;

/// Shortcut actions in the order the editor lists them: id, label, default keys.
pub const SHORTCUTS: [(&str, &str, &str); 6] = [
    ("preset1", "EQ 1", "Ctrl+Alt+1"),
    ("preset2", "EQ 2", "Ctrl+Alt+2"),
    ("preset3", "EQ 3", "Ctrl+Alt+3"),
    ("preset4", "EQ 4", "Ctrl+Alt+4"),
    ("next", "Next EQ", "Ctrl+Alt+E"),
    ("previous", "Previous EQ", ""),
];

/// The `preset` command value for a shortcut action: `1`..`4`, `next` or `previous`.
pub fn shortcut_target(action: &str) -> Option<&str> {
    match action {
        "next" | "previous" => Some(action),
        _ => action.strip_prefix("preset").filter(|n| matches!(*n, "1" | "2" | "3" | "4")),
    }
}

fn stored(data: Option<&Path>) -> Map<String, Value> {
    data.map(crate::paths::settings_path)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// Every shortcut action with its keys: the saved ones, else the defaults.
/// An empty string means the action has no shortcut.
pub fn shortcuts(data: Option<&Path>) -> Vec<(String, String)> {
    let saved = stored(data).get("shortcuts").and_then(Value::as_object).cloned().unwrap_or_default();
    SHORTCUTS
        .iter()
        .map(|(id, _, default)| {
            let keys = saved.get(*id).and_then(Value::as_str).unwrap_or(default);
            (id.to_string(), keys.to_owned())
        })
        .collect()
}

/// Save every shortcut. Without a data folder there is nothing to keep them in.
pub fn save_shortcuts(data: Option<&Path>, shortcuts: &[(String, String)]) -> Result<()> {
    let Some(data) = data else { return Ok(()) };
    let mut settings = stored(Some(data));
    let map: Map<String, Value> = shortcuts.iter().map(|(id, keys)| (id.clone(), json!(keys))).collect();
    settings.insert("shortcuts".into(), Value::Object(map));
    atomic_json(&crate::paths::settings_path(data), &Value::Object(settings))
}

/// Desktop app preferences: name and default. `tray_icon` shows the tray
/// icon; `close_to_tray` and `minimize_to_tray` hide the editor there instead
/// of quitting or minimizing; `check_updates` asks GitHub for new releases.
pub const PREFERENCES: [(&str, bool); 4] =
    [("tray_icon", true), ("close_to_tray", true), ("minimize_to_tray", false), ("check_updates", true)];

/// Every preference: the saved value, else the default.
pub fn preferences(data: Option<&Path>) -> Map<String, Value> {
    let saved = stored(data).get("preferences").and_then(Value::as_object).cloned().unwrap_or_default();
    PREFERENCES
        .iter()
        .map(|(name, default)| (name.to_string(), json!(saved.get(*name).and_then(Value::as_bool).unwrap_or(*default))))
        .collect()
}

/// One preference by name; unknown names are off.
pub fn preference(data: Option<&Path>, name: &str) -> bool {
    preferences(data).get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// Save the known preferences in `changes` over the current ones; returns them all.
pub fn save_preferences(data: Option<&Path>, changes: &Value) -> Result<Map<String, Value>> {
    let mut current = preferences(data);
    for (name, _) in PREFERENCES {
        if let Some(value) = changes.get(name).and_then(Value::as_bool) {
            current.insert(name.into(), json!(value));
        }
    }
    if let Some(data) = data {
        let mut settings = stored(Some(data));
        settings.insert("preferences".into(), Value::Object(current.clone()));
        atomic_json(&crate::paths::settings_path(data), &Value::Object(settings))?;
    }
    Ok(current)
}
