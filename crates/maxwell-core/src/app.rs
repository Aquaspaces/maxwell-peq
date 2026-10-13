//! The device session: which connection to use, which preset is active, and
//! which profile each of the four custom presets was saved from.
//!
//! Profiles are saved straight into a custom preset, then played by making
//! that preset active, here or on the headset. Nothing is sent as temporary
//! live EQ, so switching presets always plays what the preset stores.

use crate::audeze::AudezeWatch;
use crate::fmt::py_str;
use crate::json::{get, get_or_null, object, truthy};
use crate::util::{atomic_json, token_urlsafe, utc_stamp};
use crate::worker::{Runner, Worker};
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub const DEVICE_ENDPOINTS: [&str; 6] =
    ["/api/read_device", "/api/save", "/api/select", "/api/balance/read", "/api/balance/apply", "/api/balance/source"];

/// Custom preset `slot` (0..3) is headset preset `slot + 7`, shown as Preset
/// `slot + 1` in the Audeze app.
const FIRST_CUSTOM: u64 = 7;

pub struct App {
    pub enable_live: bool,
    pub audeze: Arc<AudezeWatch>,
    pub token: String,
    /// The connection the user chose: `auto`, `usb` or `dongle`.
    pub transport: String,
    /// The connection the last headset read went through, once there is one.
    pub connection: Value,
    pub device: Value,
    /// The preset index last read from the headset.
    pub index: Value,
    /// Per custom preset: `{profile_id, name, preset, blob_hex}` for the
    /// profile saved there, or null. Dropped once the preset stores other EQ.
    pub links: [Value; 4],
    /// The last balance read in this run; writes need one first.
    pub balance: Value,
    pub data_dir: Option<PathBuf>,
    runner: Runner,
}

/// "Preset 2" for a custom preset, "built-in preset 3" otherwise.
fn preset_name(index: &Value) -> String {
    match index.as_u64() {
        Some(index) if (FIRST_CUSTOM..FIRST_CUSTOM + 4).contains(&index) => {
            format!("Preset {}", index - FIRST_CUSTOM + 1)
        }
        _ => format!("built-in preset {}", py_str(index)),
    }
}

fn device_line(transport: &str, state: &Value) -> Value {
    json!(format!(
        "Original Maxwell PS · {} · {}",
        match transport {
            "usb" => "USB-C",
            "dongle" => "dongle",
            _ => "connected",
        },
        preset_name(get_or_null(state, "index"))
    ))
}

fn load_links(data_dir: Option<&PathBuf>) -> [Value; 4] {
    let mut links = [Value::Null, Value::Null, Value::Null, Value::Null];
    let stored = data_dir
        .map(|dir| crate::paths::links_path(dir))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if let Some(Value::Array(saved)) = stored.as_ref().and_then(|data| data.get("slots")) {
        for (link, saved) in links.iter_mut().zip(saved) {
            if saved.get("blob_hex").is_some_and(Value::is_string) {
                *link = saved.clone();
            }
        }
    }
    links
}

impl App {
    pub fn new(enable_live: bool, data_dir: Option<PathBuf>, audeze: Arc<AudezeWatch>, runner: Runner) -> Self {
        App {
            enable_live,
            audeze,
            token: token_urlsafe(),
            transport: "auto".into(),
            connection: Value::Null,
            device: json!("Original Maxwell PlayStation"),
            index: Value::Null,
            links: load_links(data_dir.as_ref()),
            balance: Value::Null,
            data_dir,
            runner,
        }
    }

    /// Write a timestamped record to the logs folder; returns its path.
    fn audit(&self, kind: &str, value: &Value) -> Result<String> {
        let data = self.data_dir.clone().unwrap_or_else(crate::paths::data_root);
        let stamp = utc_stamp();
        let target = crate::paths::logs_path(&data).join(format!("{stamp}-{kind}.json"));
        let mut record = Map::new();
        record.insert("created_at".into(), json!(stamp));
        if let Value::Object(fields) = value {
            record.extend(fields.clone());
        }
        atomic_json(&target, &Value::Object(record))?;
        Ok(target.display().to_string())
    }

    /// Keep the links across restarts. Without a data folder they live in memory only.
    fn store_links(&self) {
        if let Some(dir) = &self.data_dir {
            // A failed write loses only the labels, never headset state.
            let _ = atomic_json(&crate::paths::links_path(dir), &json!({"version": 1, "slots": self.links}));
        }
    }

    pub fn status(&self) -> Value {
        let (dll_available, dll_error) = match crate::vendor::verified_dll() {
            Ok(_) => (true, Value::Null),
            Err(error) => (false, json!(error.to_string())),
        };
        let audeze = self.audeze.running(false).unwrap_or_default();
        let slots = (0..4u64)
            .map(|slot| {
                let index = FIRST_CUSTOM + slot;
                let link = &self.links[slot as usize];
                object([
                    ("slot", json!(slot)),
                    ("index", json!(index)),
                    ("active", json!(self.index.as_u64() == Some(index))),
                    ("profile_id", get_or_null(link, "profile_id").clone()),
                    ("name", get_or_null(link, "name").clone()),
                    ("preset", get_or_null(link, "preset").clone()),
                ])
            })
            .collect();
        object([
            ("token", json!(self.token)),
            ("dll_available", json!(dll_available)),
            ("platform", json!(std::env::consts::OS)),
            ("dll_error", dll_error),
            (
                "encoder",
                json!(match crate::compile::Encoder::from_env() {
                    crate::compile::Encoder::Auto if dll_available => "vendor",
                    _ => "built-in",
                }),
            ),
            ("live_enabled", json!(self.enable_live)),
            ("device", self.device.clone()),
            ("device_read", json!(!self.index.is_null())),
            ("index", self.index.clone()),
            ("slots", Value::Array(slots)),
            ("transport", json!(self.transport)),
            ("transports", json!(["auto", "usb", "dongle"])),
            ("connection", self.connection.clone()),
            ("audeze_running", json!(!audeze.is_empty())),
            ("audeze_apps", json!(audeze)),
            ("data_dir", self.data_dir.as_ref().map_or(Value::Null, |dir| json!(dir.display().to_string()))),
            ("hardware_audio_verified", json!(false)),
            ("balance", self.balance.clone()),
        ])
    }

    fn request_transport(request: &Value, default: &str) -> Result<String> {
        match get(request, "transport") {
            None => Ok(default.to_owned()),
            Some(Value::String(t)) if t == "auto" || t == "usb" || t == "dongle" => Ok(t.clone()),
            Some(_) => Err(Error::value("Choose automatic, USB-C or dongle.")),
        }
    }

    /// Channel balance and the source profile. Both write straight to the
    /// headset's storage, so each write is logged with the values it replaces.
    fn balance(&mut self, path: &str, request: &Value) -> Result<Value> {
        let read_through = self.balance.get("transport").and_then(Value::as_str).map(str::to_owned);
        let transport = Self::request_transport(request, read_through.as_deref().unwrap_or(&self.transport))?;
        // Writes go through the connection the balance was read through.
        let transport = match (&read_through, path) {
            (Some(read_through), "/api/balance/apply" | "/api/balance/source") if transport == "auto" => {
                read_through.clone()
            }
            _ => transport,
        };
        let mut device_request = object([("transport", json!(transport))]);
        let operation = match path {
            "/api/balance/read" => "balance_read",
            "/api/balance/apply" => {
                device_request["left"] = get_or_null(request, "left").clone();
                device_request["right"] = get_or_null(request, "right").clone();
                "balance_write"
            }
            _ => {
                device_request["source"] = get_or_null(request, "source").clone();
                "source_write"
            }
        };
        device_request["operation"] = json!(operation);
        if operation != "balance_read" {
            match read_through {
                None => return Err(Error::value("Read the current balance first, so it can be put back.")),
                Some(read_through) if read_through != transport => {
                    return Err(Error::value("Read the balance again through this connection first."));
                }
                Some(_) => {}
            }
            if self.data_dir.is_some() {
                let record = object([("request", device_request.clone()), ("last_read", self.balance.clone())]);
                self.audit(operation, &record)?;
            }
        }
        let outcome = self.run(Worker::Device, &device_request);
        if let Err(Error::Worker(details)) = &outcome
            && truthy(details.get("write_started").unwrap_or(&Value::Null))
        {
            // The headset may hold anything now; make the page read again.
            self.balance = Value::Null;
        }
        // Balance reads choose their own connection; EQ keeps `self.transport`.
        let mut result = outcome?;
        if let Value::Object(fields) = &mut result {
            fields.remove("write_replies_hex");
        }
        self.balance = object([
            ("left", get_or_null(&result, "left").clone()),
            ("right", get_or_null(&result, "right").clone()),
            ("source", get_or_null(&result, "source").clone()),
            ("source_profile", get_or_null(&result, "source_profile").clone()),
            ("transport", get(&result, "transport").cloned().unwrap_or_else(|| json!(transport))),
        ]);
        let (left, right) = (py_str(get_or_null(&result, "left")), py_str(get_or_null(&result, "right")));
        let message = match operation {
            "balance_read" => format!("Balance read: left {left}, right {right}."),
            // Balance writes land in the USB-C key (NVDM 0xF665). The loader
            // reads that key at power-on only on the USB-C source profile.
            "balance_write" if get(&result, "source_profile") == Some(&json!("usb-c")) => {
                format!("Balance set to left {left}, right {right} and saved on the headset.")
            }
            "balance_write" => format!(
                "Balance set to left {left}, right {right}. On the wireless source profile the headset may load its \
                 wireless defaults at the next power-on; switch the profile to USB-C to keep this balance."
            ),
            _ if get(&result, "source_profile") == Some(&json!("usb-c")) => {
                "Source profile set to USB-C. Power-cycle the headset for it to take effect.".into()
            }
            _ => "Source profile set to wireless. Power-cycle the headset for it to take effect.".into(),
        };
        result["message"] = json!(message);
        Ok(result)
    }

    fn run(&mut self, worker: Worker, request: &Value) -> Result<Value> {
        (self.runner)(worker, request)
    }

    /// Take in a fresh headset read. A custom preset that stores other EQ
    /// than the profile saved there was changed elsewhere, so its link goes.
    fn observe(&mut self, requested: &str, state: &Value) {
        let connection = get(state, "transport").and_then(Value::as_str).unwrap_or(requested);
        let mut stored: [Option<&Value>; 4] = [None; 4];
        if let Some(Value::Array(slots)) = get(state, "slots") {
            for (known, hex) in stored.iter_mut().zip(slots) {
                *known = Some(hex);
            }
        } else if let Some(slot) = get(state, "slot").and_then(Value::as_u64).filter(|slot| *slot < 4) {
            stored[slot as usize] = Some(get_or_null(state, "saved_coefficients_hex"));
        }
        let mut changed = false;
        for (link, hex) in self.links.iter_mut().zip(stored) {
            if let Some(hex) = hex
                && !link.is_null()
                && get_or_null(link, "blob_hex") != hex
            {
                *link = Value::Null;
                changed = true;
            }
        }
        if changed {
            self.store_links();
        }
        self.index = get_or_null(state, "index").clone();
        self.device = device_line(connection, state);
        self.connection = json!(connection);
        self.transport = requested.to_owned();
    }

    fn read_device(&mut self, request: &Value) -> Result<Value> {
        let transport = Self::request_transport(request, &self.transport)?;
        let mut state = self.run(Worker::Device, &json!({"operation": "read", "transport": transport}))?;
        self.observe(&transport, &state);
        if let Value::Object(fields) = &mut state {
            fields.remove("slots");
        }
        let custom = get(&state, "slot").and_then(Value::as_u64).is_some_and(|slot| slot < 4);
        let preset = preset_name(get_or_null(&state, "index"));
        let message = if custom {
            format!("Headset read. {preset} is active.")
        } else {
            format!("Headset read. The headset is on {preset}. Make Preset 1 to 4 active to hear your profiles.")
        };
        Ok(object([("state", state), ("custom", json!(custom)), ("message", json!(message))]))
    }

    /// Make custom preset `index` (7..10) active.
    fn select(&mut self, request: &Value) -> Result<Value> {
        let transport = Self::request_transport(request, &self.transport)?;
        let index = get(request, "index")
            .and_then(Value::as_u64)
            .filter(|index| (FIRST_CUSTOM..FIRST_CUSTOM + 4).contains(index))
            .ok_or_else(|| Error::value("Choose Preset 1, 2, 3 or 4."))?;
        let device_request = json!({"operation": "select", "index": index, "transport": transport});
        let mut result = self.run(Worker::Device, &device_request)?;
        let state = get_or_null(&result, "state").clone();
        if state.is_object() {
            self.observe(&transport, &state);
        }
        if !result.is_object() {
            result = json!({});
        }
        result["message"] = json!(format!("{} is now active.", preset_name(&json!(index))));
        Ok(result)
    }

    /// Save a profile into custom preset `slot` (0..3). The request carries
    /// `preset` (filters and preamp), `slot`, and the profile's id and name.
    fn save(&mut self, request: &Value) -> Result<Value> {
        let transport = Self::request_transport(request, &self.transport)?;
        let slot = get(request, "slot")
            .and_then(Value::as_u64)
            .filter(|slot| *slot < 4)
            .ok_or_else(|| Error::value("Choose Preset 1, 2, 3 or 4."))?;
        let preset = get_or_null(request, "preset").clone();
        let compiled = self.run(Worker::Compile, &preset)?;
        let blob_hex = get(&compiled, "blob_hex")
            .cloned()
            .ok_or_else(|| Error::other("The compiler returned no coefficients."))?;
        let device_request = object([
            ("operation", json!("save")),
            ("slot", json!(slot)),
            ("blob_hex", blob_hex.clone()),
            ("transport", json!(transport)),
        ]);
        let outcome = self.run(Worker::Device, &device_request);
        let detail = |key: &str| match &outcome {
            Ok(_) => true,
            Err(Error::Worker(details)) => truthy(details.get(key).unwrap_or(&Value::Null)),
            Err(_) => false,
        };
        let (verified, started) = (detail("saved_verified"), detail("write_started"));
        if started || verified {
            // The preset now holds this profile, or something unknown.
            self.links[slot as usize] = if verified {
                object([
                    ("profile_id", get_or_null(request, "profile_id").clone()),
                    ("name", get_or_null(request, "name").clone()),
                    ("preset", preset),
                    ("blob_hex", blob_hex),
                ])
            } else {
                Value::Null
            };
            self.store_links();
        }
        let mut result = outcome?;
        let state = get_or_null(&result, "state").clone();
        if state.is_object() {
            self.observe(&transport, &state);
        }
        let target = preset_name(&json!(FIRST_CUSTOM + slot));
        let message = if truthy(get_or_null(&result, "live_applied")) {
            format!("Saved to {target}. It is playing now.")
        } else {
            format!("Saved to {target}. Make it active to hear it.")
        };
        if !result.is_object() {
            result = json!({});
        }
        result["message"] = json!(message);
        Ok(result)
    }

    /// A device action. The caller holds the device lock.
    pub fn perform(&mut self, path: &str, request: &Value) -> Result<Value> {
        if !DEVICE_ENDPOINTS.contains(&path) {
            return Err(Error::value("Unknown endpoint."));
        }
        if !self.enable_live {
            return Err(Error::value("Device controls are off because the app was started with --offline."));
        }
        if self.audeze.running(true).is_some_and(|apps| !apps.is_empty()) {
            return Err(Error::value(
                "Audeze App/HQ is open. Custom PEQ will not work while it is open; close it first. No EQ was sent.",
            ));
        }
        match path {
            "/api/read_device" => self.read_device(request),
            "/api/select" => self.select(request),
            "/api/save" => self.save(request),
            _ => self.balance(path, request),
        }
    }
}
