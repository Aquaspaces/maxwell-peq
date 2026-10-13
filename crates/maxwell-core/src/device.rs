//! The device worker: snapshots and explicitly requested live or saved EQ.
//!
//! It runs in its own process (`--worker device`) so a stalled HID call can be
//! killed. On failure it reports how far the write got, so the app knows
//! whether the headset may have changed.

use crate::compile::BLOB_BYTES;
use crate::dsp::{RATES, WORDS, decode_words};
use crate::hid::Transport;
use crate::json::{get, object, truthy};
use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The headset operations a worker run needs.
pub trait Device {
    fn read_state(&mut self) -> Result<Value>;
    fn has_storage_space(&mut self) -> Result<bool>;
    fn save_coefficients(&mut self, slot: u8, coefficients: &[u8]) -> Result<Value>;
    fn apply_live(&mut self, coefficients: &[u8]) -> Result<Value>;
    /// After a save, make the headset play the stored preset at `index` where
    /// the model needs it (Maxwell 2 over USB). True when it reselected.
    fn reload_saved(&mut self, _index: u8) -> Result<bool> {
        Ok(false)
    }
}

/// How far a run got; reported with any error.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Progress {
    pub write_started: bool,
    pub persistent_write_started: bool,
    pub saved_verified: bool,
    pub last_state: Option<Value>,
}

pub fn validate_blob(blob: &[u8]) -> Result<()> {
    if blob.len() != BLOB_BYTES || blob[..4] != 4u32.to_le_bytes() {
        return Err(Error::value("Expected the verified 844-byte, four-rate coefficient format."));
    }
    for ((rate_id, _), offset) in RATES.iter().zip((4..BLOB_BYTES).step_by(210)) {
        let word = |i: usize| u16::from_le_bytes([blob[offset + 2 * i], blob[offset + 2 * i + 1]]);
        if (word(0), word(1)) != (*rate_id, WORDS as u16) {
            return Err(Error::value("Unexpected coefficient bank header."));
        }
        decode_words(&(2..2 + WORDS).map(word).collect::<Vec<_>>())?;
    }
    Ok(())
}

fn hex_blob(value: Option<&Value>) -> Option<Vec<u8>> {
    value.and_then(Value::as_str).and_then(|text| hex::decode(text).ok())
}

fn same_state(actual: &Value, expected: &Value, check_coefficients: bool) -> Result<()> {
    let path = |state: &Value| state["device"]["path"].as_str().unwrap_or_default().to_lowercase();
    if path(actual) != path(expected) {
        return Err(Error::StateChanged(
            "The connected control interface changed. Use the connection associated with this backup.".into(),
        ));
    }
    let keys: &[&str] =
        if check_coefficients { &["index", "raw_ui_hex", "saved_coefficients_hex"] } else { &["index", "raw_ui_hex"] };
    for key in keys {
        if get(actual, key).unwrap_or(&Value::Null) != get(expected, key).unwrap_or(&Value::Null) {
            return Err(Error::StateChanged(
                "The active or saved preset changed. Return to the backed-up custom preset before retrying.".into(),
            ));
        }
    }
    Ok(())
}

/// `open(allow_live, allow_persistent, transport)`; production uses [`crate::hid::open`].
pub type Open<'a> = dyn FnMut(bool, bool, Transport) -> Result<Box<dyn Device>> + 'a;

pub fn execute(request: &Value, open: &mut Open, progress: &mut Progress) -> Result<Value> {
    *progress = Progress::default();
    let operation = match get(request, "operation").and_then(Value::as_str) {
        Some(op @ ("read" | "apply" | "save" | "restore_saved")) => op,
        _ => return Err(Error::value("Unsupported device operation.")),
    };
    let persistent = matches!(operation, "save" | "restore_saved");
    let mut blob = Vec::new();
    if operation != "read" {
        blob = hex_blob(get(request, "blob_hex"))
            .ok_or_else(|| Error::value("Expected the verified 844-byte, four-rate coefficient format."))?;
        validate_blob(&blob)?;
        let expected = get(request, "expected");
        let slot = expected.and_then(|e| get(e, "slot")).and_then(Value::as_u64);
        if !expected.is_some_and(truthy) || !slot.is_some_and(|slot| slot < 4) {
            return Err(Error::value("A custom-preset backup is required before applying."));
        }
    }
    let transport = match get(request, "transport") {
        None => Transport::Usb,
        Some(value) => {
            value.as_str().and_then(Transport::parse).ok_or_else(|| Error::value("Choose usb or dongle transport."))?
        }
    };
    let mut device = open(operation != "read", persistent, transport)?;
    let state = device.read_state()?;
    // Explicit saved-backup restoration may repair a damaged stored blob.
    if operation != "restore_saved"
        && let Some(saved) = get(&state, "saved_coefficients_hex").filter(|v| truthy(v))
    {
        validate_blob(&hex_blob(Some(saved)).ok_or_else(|| Error::value("Invalid saved coefficient hex."))?)?;
    }
    if operation == "read" {
        return Ok(state);
    }
    same_state(&state, &request["expected"], operation != "restore_saved")?;
    let mut result = serde_json::Map::new();
    if persistent {
        if !device.has_storage_space()? {
            return Err(Error::value("Headset reports insufficient profile storage. No profile was saved."));
        }
        progress.write_started = true;
        progress.persistent_write_started = true;
        let slot = state["slot"].as_u64().ok_or_else(|| Error::value("The headset is not on a custom preset."))? as u8;
        if let Value::Object(saved) = device.save_coefficients(slot, &blob)? {
            result.extend(saved);
        }
        // Save returns only after a byte-for-byte read-back of the same key.
        progress.saved_verified = true;
        let mut desired = state.clone();
        desired["saved_coefficients_hex"] = json!(hex::encode(&blob));
        progress.last_state = Some(desired.clone());
        let saved = device.read_state()?;
        same_state(&saved, &desired, true)?;
        progress.last_state = Some(saved.clone());
        result.insert("state".into(), saved);
    }
    progress.write_started = true;
    let live = device.apply_live(&blob)?;
    result.insert("accepted".into(), live.get("accepted").cloned().unwrap_or(Value::Null));
    result.insert("ack_hex".into(), live.get("ack_hex").cloned().unwrap_or(Value::Null));
    result.insert("live_applied".into(), json!(true));
    result.insert("persistence_write_sent".into(), json!(persistent));
    result.insert("sha256".into(), json!(hex::encode(Sha256::digest(&blob))));
    if persistent {
        let index =
            state["index"].as_u64().ok_or_else(|| Error::value("The headset is not on a custom preset."))? as u8;
        result.insert("preset_reloaded".into(), json!(device.reload_saved(index)?));
    }
    Ok(Value::Object(result))
}

/// The worker's stdout on failure, as `device_worker.py` printed it.
pub fn failure(error: &Error, progress: &Progress) -> Value {
    let mut message = error.to_string();
    if !progress.write_started && !message.contains("No EQ was sent") {
        message.push_str(" No EQ was sent.");
    }
    object([
        ("error", json!(message)),
        ("write_started", json!(progress.write_started)),
        ("persistent_write_started", json!(progress.persistent_write_started)),
        ("saved_verified", json!(progress.saved_verified)),
        ("state", progress.last_state.clone().unwrap_or(Value::Null)),
        ("state_changed", json!(matches!(error, Error::StateChanged(_)))),
    ])
}

/// Run one request against the real headset: the `--worker device` entry point.
pub fn run(request: &Value) -> (Value, bool) {
    if get(request, "operation").and_then(Value::as_str).is_some_and(|op| crate::balance::OPERATIONS.contains(&op)) {
        return crate::balance::run(request);
    }
    if get(request, "operation").and_then(Value::as_str).is_some_and(|op| crate::controls::OPERATIONS.contains(&op)) {
        return crate::controls::run(request);
    }
    let mut progress = Progress::default();
    let mut open = |live, persistent, transport| crate::hid::open(transport, live, persistent);
    match execute(request, &mut open, &mut progress) {
        Ok(result) => (result, true),
        Err(error) => (failure(&error, &progress), false),
    }
}
