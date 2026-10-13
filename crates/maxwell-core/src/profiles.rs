//! Named presets in one JSON file. Contents are editor drafts, so only the
//! shape is checked; compiling happens when a profile is applied.

use crate::dsp::Kind;
use crate::json::{get, object, truthy};
use crate::util::atomic_json;
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const FIELDS: [&str; 5] = ["type", "frequency", "gain", "q", "enabled"];
pub const MAX_PROFILES: usize = 50;

pub struct Profiles {
    path: Option<PathBuf>,
    data: Value,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn is_number(value: &Value) -> bool {
    value.is_number()
}

/// Strip a profile name and require 1 to 60 characters.
pub fn check_name(name: Option<&Value>) -> Result<String> {
    match name {
        Some(Value::String(name)) => {
            let name = crate::apo::py_trim(name);
            if name.is_empty() || name.chars().count() > 60 {
                Err(Error::value("Profile names need 1 to 60 characters."))
            } else {
                Ok(name.to_owned())
            }
        }
        _ => Err(Error::value("Profile names need 1 to 60 characters.")),
    }
}

/// Normalize a preset's shape: 1 to 10 known filters, numbers or nulls, and a preamp.
pub fn check_preset(preset: Option<&Value>) -> Result<Value> {
    let rows = match preset.and_then(|p| get(p, "filters")) {
        Some(Value::Array(rows)) if (1..=10).contains(&rows.len()) && preset.is_some_and(Value::is_object) => rows,
        _ => return Err(Error::value("A profile needs 1 to 10 filters.")),
    };
    let mut filters = Vec::with_capacity(rows.len());
    for row in rows {
        let known = matches!(get(row, "type"), Some(Value::String(name)) if Kind::parse(name).is_some());
        if !row.is_object() || !known {
            return Err(Error::value("Invalid filter in profile."));
        }
        for key in ["frequency", "gain", "q"] {
            if get(row, key).is_some_and(|value| !value.is_null() && !is_number(value)) {
                return Err(Error::value("Filter values must be numbers."));
            }
        }
        let mut filter: Map<String, Value> =
            FIELDS.iter().map(|key| (key.to_string(), get(row, key).cloned().unwrap_or(Value::Null))).collect();
        filter.insert("enabled".into(), Value::Bool(get(row, "enabled") != Some(&Value::Bool(false))));
        filters.push(Value::Object(filter));
    }
    let preamp = preset.and_then(|p| get(p, "preamp")).cloned().unwrap_or_else(|| json!("auto"));
    if preamp != json!("auto") && !is_number(&preamp) {
        return Err(Error::value("Preamp must be auto or a number."));
    }
    Ok(object([("filters", Value::Array(filters)), ("preamp", preamp)]))
}

fn valid(profile: &Value) -> bool {
    let (Some(preset), Some(name), Some(id)) = (get(profile, "preset"), get(profile, "name"), get(profile, "id"))
    else {
        return false;
    };
    check_preset(Some(preset)).is_ok() && check_name(Some(name)).is_ok() && id.is_string()
}

impl Profiles {
    /// Load the library, dropping damaged entries; a missing file starts with Flat.
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let mut data = json!({"version": 1, "active": null, "profiles": []});
        if let Some(path) = path.as_ref().filter(|path| path.exists()) {
            let text = std::fs::read_to_string(path)?;
            let loaded: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                .map_err(|_| Error::value(format!("Invalid profile library: {}", path.display())))?;
            let Some(Value::Array(profiles)) = get(&loaded, "profiles") else {
                return Err(Error::value(format!("Invalid profile library: {}", path.display())));
            };
            data["active"] = get(&loaded, "active").cloned().unwrap_or(Value::Null);
            data["profiles"] = Value::Array(profiles.iter().filter(|p| valid(p)).cloned().collect());
        }
        if data["profiles"].as_array().is_some_and(Vec::is_empty) {
            let flat = json!({"filters": [{"type": "peak", "frequency": 1000, "gain": 0, "q": 1, "enabled": true}], "preamp": "auto"});
            data["profiles"].as_array_mut().expect("array").push(object([
                ("id", Value::from(new_id())),
                ("name", Value::from("Flat")),
                ("preset", flat),
            ]));
        }
        let mut profiles = Profiles { path, data };
        if profiles.find(&profiles.data["active"].clone()).is_none() {
            profiles.data["active"] = profiles.list()[0]["id"].clone();
        }
        Ok(profiles)
    }

    fn list(&self) -> &Vec<Value> {
        self.data["profiles"].as_array().expect("profiles is an array")
    }

    fn list_mut(&mut self) -> &mut Vec<Value> {
        self.data["profiles"].as_array_mut().expect("profiles is an array")
    }

    fn position(&self, id: &Value) -> Option<usize> {
        self.list().iter().position(|p| &p["id"] == id)
    }

    fn find(&self, id: &Value) -> Option<&Value> {
        self.position(id).map(|i| &self.list()[i])
    }

    pub fn listing(&self) -> Value {
        self.data.clone()
    }

    /// One of the `/api/profiles/*` actions; returns the library plus `saved`.
    pub fn perform(&mut self, path: &str, request: &Value) -> Result<Value> {
        let id = get(request, "id").cloned().unwrap_or(Value::Null);
        let saved = match path {
            "/api/profiles/save" => {
                let index = if truthy(&id) { self.position(&id) } else { None };
                let existing = index.map(|i| self.list()[i].clone());
                let name = match &existing {
                    Some(profile) if get(request, "name").is_none() => profile["name"].clone(),
                    _ => Value::from(check_name(get(request, "name"))?),
                };
                let preset = match &existing {
                    Some(profile) if get(request, "preset").is_none() => profile["preset"].clone(),
                    _ => check_preset(get(request, "preset"))?,
                };
                let index = match index {
                    Some(index) => index,
                    None => {
                        if self.list().len() >= MAX_PROFILES {
                            return Err(Error::value("The library holds 50 profiles. Delete one first."));
                        }
                        self.list_mut().push(object([("id", Value::from(new_id()))]));
                        self.list().len() - 1
                    }
                };
                let profile = self.list_mut()[index].as_object_mut().expect("profile is an object");
                profile.insert("name".into(), name);
                profile.insert("preset".into(), preset);
                let profile_id = profile["id"].clone();
                if get(request, "activate").is_some_and(truthy) {
                    self.data["active"] = profile_id.clone();
                }
                profile_id
            }
            "/api/profiles/delete" => {
                if self.list().len() == 1 {
                    return Err(Error::value("Keep at least one profile."));
                }
                let index = self.position(&id).ok_or_else(|| Error::value("That profile no longer exists."))?;
                let removed = self.list_mut().remove(index);
                if self.data["active"] == removed["id"] {
                    let next = index.min(self.list().len() - 1);
                    self.data["active"] = self.list()[next]["id"].clone();
                }
                Value::Null
            }
            "/api/profiles/activate" => {
                if self.find(&id).is_none() {
                    return Err(Error::value("That profile no longer exists."));
                }
                self.data["active"] = id.clone();
                id
            }
            "/api/profiles/order" => {
                let mismatch = || Error::value("The profile order does not match the library.");
                let Some(Value::Array(ids)) = get(request, "ids") else { return Err(mismatch()) };
                let mut wanted: Vec<String> = ids.iter().map(Value::to_string).collect();
                let mut current: Vec<String> = self.list().iter().map(|p| p["id"].to_string()).collect();
                wanted.sort();
                current.sort();
                if wanted != current {
                    return Err(mismatch());
                }
                let ordered = ids.iter().map(|id| self.find(id).cloned().expect("checked above")).collect();
                self.data["profiles"] = Value::Array(ordered);
                Value::Null
            }
            _ => return Err(Error::value("Unknown endpoint.")),
        };
        if let Some(path) = &self.path {
            atomic_json(path, &self.data)?;
        }
        let mut result = self.data.clone();
        result["saved"] = saved;
        Ok(result)
    }
}
