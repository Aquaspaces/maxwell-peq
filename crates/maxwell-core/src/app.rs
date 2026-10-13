//! The device session: backups, the recovery journal, and the rules for when
//! the headset may be written.
//!
//! Every write is journaled before it is sent, so a crash or a hung worker
//! leaves enough on disk to restore the original preset after a restart.

use crate::audeze::AudezeWatch;
use crate::fmt::py_str;
use crate::json::{get, get_or_null, object, truthy};
use crate::util::{atomic_json, token_urlsafe, utc_stamp};
use crate::worker::{Runner, Worker};
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

pub const DEVICE_ENDPOINTS: [&str; 8] = [
    "/api/read_device",
    "/api/apply",
    "/api/restore",
    "/api/save",
    "/api/restore_saved",
    "/api/balance/read",
    "/api/balance/apply",
    "/api/balance/source",
];

const UNCHANGED: &str = "unchanged";

pub struct App {
    pub enable_live: bool,
    pub audeze: Arc<AudezeWatch>,
    pub token: String,
    pub backup: Value,
    pub expected: Value,
    pub transport: String,
    pub device: Value,
    pub live_state: String,
    pub storage_state: String,
    pub backup_path: Value,
    pub pending: Value,
    pub last_profile: Value,
    pub saved_profile: Value,
    pub session_recovered: bool,
    pub session_path: Option<PathBuf>,
    /// The last balance read in this run; writes need one first.
    pub balance: Value,
    runner: Runner,
}

fn saved_hex_len(state: &Value) -> Option<usize> {
    let text = get(state, "saved_coefficients_hex").map_or(Some(""), Value::as_str)?;
    hex::decode(text).ok().map(|bytes| bytes.len())
}

fn valid_slot(state: &Value) -> bool {
    get(state, "slot").and_then(Value::as_u64).is_some_and(|slot| slot < 4)
}

impl App {
    /// A session; when `session_path` exists, it is recovered or refused.
    pub fn new(
        enable_live: bool,
        session_path: Option<PathBuf>,
        audeze: Arc<AudezeWatch>,
        runner: Runner,
    ) -> Result<Self> {
        let mut app = App {
            enable_live,
            audeze,
            token: token_urlsafe(),
            backup: Value::Null,
            expected: Value::Null,
            transport: "usb".into(),
            device: json!("Original Maxwell PlayStation"),
            live_state: UNCHANGED.into(),
            storage_state: UNCHANGED.into(),
            backup_path: Value::Null,
            pending: Value::Null,
            last_profile: Value::Null,
            saved_profile: Value::Null,
            session_recovered: false,
            session_path,
            balance: Value::Null,
            runner,
        };
        if app.session_path.as_ref().is_some_and(|path| path.exists()) {
            app.recover()?;
        }
        Ok(app)
    }

    fn session(&self) -> Value {
        object([
            ("version", json!(1)),
            ("backup", self.backup.clone()),
            ("backup_path", self.backup_path.clone()),
            ("expected", self.expected.clone()),
            ("transport", json!(self.transport)),
            ("device", self.device.clone()),
            ("live_state", json!(self.live_state)),
            ("storage_state", json!(self.storage_state)),
            ("pending", self.pending.clone()),
            ("last_profile", self.last_profile.clone()),
            ("saved_profile", self.saved_profile.clone()),
        ])
    }

    fn persist(&self) -> Result<()> {
        match &self.session_path {
            Some(path) => atomic_json(path, &self.session()),
            None => Ok(()),
        }
    }

    fn recover(&mut self) -> Result<()> {
        let path = self.session_path.clone().expect("recover needs a session path");
        let text = std::fs::read_to_string(&path)?;
        let invalid = || Error::value("Invalid recovery session; preserve backups/session.json for recovery.");
        let data: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| invalid())?;
        let field = |key| get(&data, key).and_then(Value::as_str);
        let one_of = |key, options: &[&str]| field(key).is_some_and(|value| options.contains(&value));
        if !data.is_object()
            || get(&data, "version").and_then(Value::as_f64) != Some(1.0)
            || !one_of("transport", &["usb", "dongle"])
            || !one_of("live_state", &["unchanged", "applied", "uncertain"])
            || !one_of("storage_state", &["unchanged", "saved", "uncertain"])
        {
            return Err(invalid());
        }
        for key in ["backup", "expected"] {
            let value = get_or_null(&data, key);
            if value.is_null() {
                continue;
            }
            let state = if key == "backup" && value.is_object() { get_or_null(value, "state") } else { value };
            if !state.is_object() || !valid_slot(state) {
                return Err(Error::value("Invalid preset snapshot in recovery session."));
            }
            if saved_hex_len(state) != Some(crate::compile::BLOB_BYTES) {
                return Err(Error::value("Invalid saved coefficients in recovery session."));
            }
        }
        if (field("live_state") != Some(UNCHANGED) || field("storage_state") != Some(UNCHANGED))
            && !truthy(get_or_null(&data, "backup"))
        {
            return Err(Error::value("Recovery session has no original preset backup."));
        }
        let pending = get_or_null(&data, "pending");
        if !pending.is_null() && !pending.is_object() {
            return Err(Error::value("Invalid pending recovery operation."));
        }
        let take = |key| get_or_null(&data, key).clone();
        self.backup = take("backup");
        self.backup_path = take("backup_path");
        self.expected = take("expected");
        self.transport = field("transport").unwrap_or("usb").into();
        self.device = take("device");
        self.live_state = field("live_state").unwrap_or(UNCHANGED).into();
        self.storage_state = field("storage_state").unwrap_or(UNCHANGED).into();
        self.pending = take("pending");
        self.last_profile = take("last_profile");
        self.saved_profile = take("saved_profile");
        self.session_recovered = true;
        Ok(())
    }

    /// Write a timestamped audit file next to the session; returns its path.
    fn audit(&self, kind: &str, value: &Value) -> Result<String> {
        let directory = match &self.session_path {
            Some(path) => path.parent().map(PathBuf::from).unwrap_or_default(),
            None => crate::paths::data_root().join("backups"),
        };
        let stamp = utc_stamp();
        let target = directory.join(format!("{stamp}-{kind}.json"));
        let mut record = Map::new();
        record.insert("created_at".into(), json!(stamp));
        if let Value::Object(fields) = value {
            record.extend(fields.clone());
        }
        atomic_json(&target, &Value::Object(record))?;
        Ok(target.display().to_string())
    }

    pub fn data_dir(&self) -> Option<PathBuf> {
        self.session_path.as_ref().and_then(|path| path.parent()?.parent().map(PathBuf::from))
    }

    pub fn status(&self) -> Value {
        let (dll_available, dll_error) = match crate::vendor::verified_dll() {
            Ok(_) => (true, Value::Null),
            Err(error) => (false, json!(error.to_string())),
        };
        let audeze = self.audeze.running(false).unwrap_or_default();
        let pending_operation =
            if truthy(&self.pending) { get_or_null(&self.pending, "operation").clone() } else { Value::Null };
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
            ("backup_exists", json!(!self.backup.is_null())),
            ("live_state", json!(self.live_state)),
            ("storage_state", json!(self.storage_state)),
            ("transport", json!(self.transport)),
            ("transports", json!(["usb", "dongle"])),
            ("session_recovered", json!(self.session_recovered)),
            ("pending_operation", pending_operation),
            ("last_profile", self.last_profile.clone()),
            ("saved_profile", self.saved_profile.clone()),
            ("audeze_running", json!(!audeze.is_empty())),
            ("audeze_apps", json!(audeze)),
            ("data_dir", self.data_dir().map_or(Value::Null, |dir| json!(dir.display().to_string()))),
            ("backup_path", self.backup_path.clone()),
            ("hardware_audio_verified", json!(false)),
            ("balance", self.balance.clone()),
        ])
    }

    fn request_transport(request: &Value, default: &str) -> Result<String> {
        match get(request, "transport") {
            None => Ok(default.to_owned()),
            Some(Value::String(t)) if t == "usb" || t == "dongle" => Ok(t.clone()),
            Some(_) => Err(Error::value("Choose USB-C or dongle.")),
        }
    }

    /// Channel balance and the source profile. Both write straight to the
    /// headset's storage, so each write is logged with the values it replaces.
    fn balance(&mut self, path: &str, request: &Value) -> Result<Value> {
        let read_through = self.balance.get("transport").and_then(Value::as_str).map(str::to_owned);
        let transport = Self::request_transport(request, read_through.as_deref().unwrap_or(&self.transport))?;
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
            if self.session_path.is_some() {
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
        // The EQ backup stays tied to the connection it was read through, so
        // a balance run leaves `self.transport` alone.
        let mut result = outcome?;
        if let Value::Object(fields) = &mut result {
            fields.remove("write_replies_hex");
        }
        self.balance = object([
            ("left", get_or_null(&result, "left").clone()),
            ("right", get_or_null(&result, "right").clone()),
            ("source", get_or_null(&result, "source").clone()),
            ("source_profile", get_or_null(&result, "source_profile").clone()),
            ("transport", json!(transport)),
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

    fn read_device(&mut self, request: &Value) -> Result<Value> {
        // Once EQ has been sent, reading again only switches or rebinds the
        // connection: the headset must still hold what the session expects, and
        // the original backup and live/saved states are kept. No EQ is sent.
        let handoff = !self.backup.is_null() && (self.live_state != UNCHANGED || self.storage_state != UNCHANGED);
        if handoff && self.storage_state == "uncertain" {
            return Err(Error::value(
                "Restore the uncertain saved profile through its current connection before switching.",
            ));
        }
        if !handoff && (self.live_state != UNCHANGED || self.storage_state != UNCHANGED) {
            return Err(Error::value("Restore the existing backup before reading a new one."));
        }
        let transport = match get(request, "transport") {
            None => self.transport.clone(),
            Some(Value::String(t)) if t == "usb" || t == "dongle" => t.clone(),
            Some(_) => return Err(Error::value("Choose USB-C or dongle.")),
        };
        let state = self.run(Worker::Device, &json!({"operation": "read", "transport": transport}))?;
        if handoff {
            let expected = if self.expected.is_null() { &self.backup["state"] } else { &self.expected };
            if ["index", "slot", "raw_ui_hex", "saved_coefficients_hex"]
                .iter()
                .any(|key| get_or_null(&state, key) != get_or_null(expected, key))
            {
                return Err(Error::value(
                    "This connection has a different preset or stored EQ. Your existing connection and original backup are unchanged.",
                ));
            }
            let index = py_str(get_or_null(&state, "index"));
            let previous = (self.transport.clone(), self.device.clone(), self.expected.clone());
            self.device = json!(format!(
                "Original Maxwell PS · {} · preset {index}",
                if transport == "usb" { "USB-C" } else { "dongle" }
            ));
            self.transport = transport;
            self.expected = state.clone();
            if let Err(error) = self.persist() {
                (self.transport, self.device, self.expected) = previous;
                return Err(error);
            }
            return Ok(object([
                ("state", state),
                ("backup_exists", json!(true)),
                ("backup_path", self.backup_path.clone()),
                ("message", json!("Connection verified. Your original backup is kept. No EQ was sent.")),
            ]));
        }
        let index = py_str(get_or_null(&state, "index"));
        self.device = json!(format!(
            "Original Maxwell PS · {} · preset {index}",
            if transport == "usb" { "USB-C" } else { "dongle" }
        ));
        self.transport = transport;
        let slot = get_or_null(&state, "slot");
        if slot.is_null() {
            self.backup = Value::Null;
            self.backup_path = Value::Null;
            self.expected = Value::Null;
            self.pending = Value::Null;
            self.persist()?;
            return Ok(object([
                ("state", state.clone()),
                ("backup_exists", json!(false)),
                (
                    "message",
                    json!(format!(
                        "Headset read successfully. Preset {index} is built-in. Choose a custom preset in the Audeze app, then read again."
                    )),
                ),
            ]));
        }
        let slot = slot.as_u64().filter(|slot| *slot < 4).ok_or_else(|| Error::value("Unexpected preset slot."))?;
        let blob = get(&state, "saved_coefficients_hex")
            .and_then(Value::as_str)
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| Error::value("The headset returned no saved coefficients."))?;
        let backup = object([
            ("state", state.clone()),
            ("coefficient_sha256", json!(hex::encode(Sha256::digest(&blob)))),
            ("note", json!("Exact saved coefficients. Existing unsaved live DSP state cannot be read.")),
        ]);
        let target = self.audit(&format!("slot{}", slot + 1), &backup)?;
        self.backup = backup;
        self.backup_path = json!(target);
        self.expected = state.clone();
        self.pending = Value::Null;
        self.persist()?;
        Ok(object([
            ("state", state),
            ("backup_exists", json!(true)),
            ("backup_path", json!(target)),
            (
                "message",
                json!(format!(
                    "Custom preset {} backed up. Restore will reapply its exact saved coefficients.",
                    slot + 1
                )),
            ),
        ]))
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
        if path == "/api/read_device" {
            return self.read_device(request);
        }
        if path.starts_with("/api/balance/") {
            return self.balance(path, request);
        }
        if self.backup.is_null() {
            return Err(Error::value("Read and back up the active custom preset first."));
        }
        let persistent = matches!(path, "/api/save" | "/api/restore_saved");
        let restoring = matches!(path, "/api/restore" | "/api/restore_saved");
        let (blob_hex, compiled) = if !restoring {
            if self.live_state == "uncertain" || self.storage_state == "uncertain" {
                return Err(Error::value("Restore after the previous uncertain write before applying a new filter."));
            }
            let compiled = self.run(Worker::Compile, request)?;
            let blob_hex = get(&compiled, "blob_hex")
                .cloned()
                .ok_or_else(|| Error::other("The compiler returned no coefficients."))?;
            (blob_hex, true)
        } else {
            (self.backup["state"]["saved_coefficients_hex"].clone(), false)
        };
        if path == "/api/restore" && self.storage_state == "uncertain" {
            return Err(Error::value("Saved preset state is uncertain. Use Restore saved preset to recover it."));
        }
        let previous = (self.live_state.clone(), self.storage_state.clone());
        if self.expected.is_null() {
            self.expected = self.backup["state"].clone();
        }
        let operation = match path {
            "/api/restore_saved" => "restore_saved",
            _ if persistent => "save",
            _ => "apply",
        };
        let preset = if compiled { request.clone() } else { Value::Null };
        let previous_pending = std::mem::replace(
            &mut self.pending,
            object([("operation", json!(operation)), ("blob_hex", blob_hex.clone()), ("preset", preset.clone())]),
        );
        self.live_state = "uncertain".into();
        if persistent {
            self.storage_state = "uncertain".into();
        }
        // From here on, a crash must leave enough on disk to restore.
        let journal = self.persist().and_then(|()| {
            if persistent {
                let record = object([
                    ("preset", preset.clone()),
                    ("blob_hex", blob_hex.clone()),
                    ("original_backup", self.backup_path.clone()),
                    ("expected", self.expected.clone()),
                ]);
                self.audit(operation, &record)?;
            }
            Ok(())
        });
        if let Err(error) = journal {
            (self.live_state, self.storage_state) = previous;
            self.pending = previous_pending;
            let _ = self.persist();
            return Err(error);
        }
        let device_request = object([
            ("operation", json!(operation)),
            ("blob_hex", blob_hex.clone()),
            ("expected", self.expected.clone()),
            ("transport", json!(self.transport)),
        ]);
        let outcome = self.run(Worker::Device, &device_request).and_then(|result| {
            let state = get_or_null(&result, "state");
            if persistent
                && (!truthy(get_or_null(&result, "saved_verified"))
                    || !state.is_object()
                    || get_or_null(state, "saved_coefficients_hex") != &blob_hex)
            {
                return Err(Error::other("Saved coefficient readback is unconfirmed. Use Restore saved preset."));
            }
            if persistent && !truthy(get_or_null(&result, "live_applied")) {
                let Value::Object(details) = json!({
                    "error": "Saved coefficients were verified, but live application is unconfirmed.",
                    "write_started": true, "persistent_write_started": true,
                    "saved_verified": true, "state": state,
                }) else {
                    unreachable!()
                };
                return Err(Error::Worker(details));
            }
            Ok(result)
        });
        let mut result = match outcome {
            Ok(result) => result,
            Err(Error::Worker(details)) => {
                let mut error = Error::Worker(details.clone());
                let detail = |key: &str| details.get(key).unwrap_or(&Value::Null);
                if detail("write_started") == &json!(false) && !truthy(detail("persistent_write_started")) {
                    (self.live_state, self.storage_state) = previous;
                    self.pending = previous_pending;
                    if truthy(detail("state_changed")) && self.storage_state == UNCHANGED {
                        self.backup = Value::Null;
                        self.expected = Value::Null;
                        self.backup_path = Value::Null;
                        self.pending = Value::Null;
                        self.live_state = UNCHANGED.into();
                    }
                } else if persistent && truthy(detail("saved_verified")) && detail("state").is_object() {
                    self.expected = detail("state").clone();
                    self.storage_state = if get_or_null(&self.expected, "saved_coefficients_hex") == &blob_hex {
                        if restoring { UNCHANGED } else { "saved" }
                    } else {
                        "uncertain"
                    }
                    .into();
                    if !restoring {
                        self.last_profile = request.clone();
                    }
                    self.saved_profile = if self.storage_state == "saved" { request.clone() } else { Value::Null };
                    let mut details = details.clone();
                    details.insert(
                        "error".into(),
                        json!(format!(
                            "{error} Saved coefficients were read back; live EQ is unconfirmed. The original backup is intact."
                        )),
                    );
                    error = Error::Worker(details);
                } else if persistent && detail("persistent_write_started") == &json!(false) {
                    self.storage_state = previous.1;
                }
                self.persist()?;
                return Err(error);
            }
            Err(error) => {
                self.persist()?;
                return Err(error);
            }
        };
        if persistent {
            self.expected = result["state"].clone();
            self.storage_state = if restoring { UNCHANGED } else { "saved" }.into();
            self.saved_profile = if restoring { Value::Null } else { request.clone() };
        }
        if !restoring {
            self.last_profile = request.clone();
        }
        self.live_state = if restoring { UNCHANGED } else { "applied" }.into();
        self.pending = Value::Null;
        self.persist()?;
        let message = match path {
            "/api/apply" => "Headset acknowledged the temporary EQ. Saved presets are unchanged.",
            "/api/restore" => {
                "Headset acknowledged the original backup in live EQ. This does not change saved storage."
            }
            "/api/save" => {
                "Preset saved, read back exactly, and applied. Audeze HQ sliders still show their old values; editing them can replace this EQ."
            }
            _ => "Original saved coefficients restored, read back exactly, and applied.",
        };
        if !result.is_object() {
            result = json!({});
        }
        result["message"] = json!(message);
        result["live_state"] = json!(self.live_state);
        result["storage_state"] = json!(self.storage_state);
        Ok(result)
    }
}
