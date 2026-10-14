//! The device worker: reads the headset, saves EQ to its custom presets and
//! switches between them.
//!
//! It runs in its own process (`--worker device`) so a stalled HID call can be
//! killed. On failure it reports how far the write got, so the app knows
//! whether the headset may have changed.

use crate::compile::BLOB_BYTES;
use crate::dsp::{RATES, WORDS, decode_words};
use crate::hid::{Transport, connect, requested};
use crate::json::{get, object};
use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The headset operations a worker run needs.
pub trait Device {
    fn read_state(&mut self) -> Result<Value>;
    /// The EQ stored in custom preset `slot`, or `None` if it was never saved.
    fn read_slot(&mut self, slot: u8) -> Result<Option<Vec<u8>>>;
    /// Make preset `index` (7..10) active and confirm the headset switched.
    fn select_preset(&mut self, index: u8) -> Result<Value>;
    fn has_storage_space(&mut self) -> Result<bool>;
    fn save_coefficients(&mut self, slot: u8, coefficients: &[u8]) -> Result<Value>;
    fn apply_live(&mut self, coefficients: &[u8]) -> Result<Value>;
    /// After a save, make the headset play the stored preset at `index` where
    /// the model needs it (Maxwell 2 over USB). True when it reselected.
    fn reload_saved(&mut self, _index: u8) -> Result<bool> {
        Ok(false)
    }
}

/// The writes a session may send. Reads are always allowed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access {
    pub live: bool,
    pub persistent: bool,
    pub select: bool,
}

/// How far a run got; reported with any error.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Progress {
    pub write_started: bool,
    pub saved_verified: bool,
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

/// `open(access, transport)`; production uses [`crate::hid::open`].
pub type Open<'a> = dyn FnMut(Access, Transport) -> Result<Box<dyn Device>> + 'a;

fn custom_slot(state: &Value) -> Option<u8> {
    state["slot"].as_u64().filter(|slot| *slot < 4).map(|slot| slot as u8)
}

/// What each custom preset stores, as hex or null. The active one was read with the state.
fn stored_slots(device: &mut dyn Device, state: &Value) -> Result<Value> {
    let mut slots = Vec::new();
    for slot in 0..4 {
        slots.push(if custom_slot(state) == Some(slot) {
            state["saved_coefficients_hex"].clone()
        } else {
            device.read_slot(slot)?.map_or(Value::Null, |raw| json!(hex::encode(raw)))
        });
    }
    Ok(Value::Array(slots))
}

/// The custom preset `step` (1 or -1) away from the active one, wrapping
/// around. From a built-in preset, next is EQ 1 and previous is EQ 4.
fn stepped(state: &Value, step: i64) -> u8 {
    match custom_slot(state) {
        Some(slot) => 7 + (i64::from(slot) + step).rem_euclid(4) as u8,
        None if step > 0 => 7,
        None => 10,
    }
}

/// `read`: the state and what all four custom presets hold. `save`: store a
/// profile in custom preset `slot` (0..3), and play it if that preset is
/// active. `select`: make preset `index` (7..10) active, or with `step` the
/// next (1) or previous (-1) custom preset.
pub fn execute(request: &Value, open: &mut Open, progress: &mut Progress) -> Result<Value> {
    *progress = Progress::default();
    let operation = match get(request, "operation").and_then(Value::as_str) {
        Some(op @ ("read" | "save" | "select")) => op,
        _ => return Err(Error::value("Unsupported device operation.")),
    };
    let mut blob = Vec::new();
    let mut slot = 0;
    let mut index = 0;
    let mut step = 0;
    match operation {
        "save" => {
            blob = get(request, "blob_hex")
                .and_then(Value::as_str)
                .and_then(|text| hex::decode(text).ok())
                .ok_or_else(|| Error::value("Expected the verified 844-byte, four-rate coefficient format."))?;
            validate_blob(&blob)?;
            slot = get(request, "slot")
                .and_then(Value::as_u64)
                .filter(|slot| *slot < 4)
                .ok_or_else(|| Error::value("Choose custom preset 1, 2, 3 or 4."))? as u8;
        }
        "select" => match get(request, "step") {
            Some(value) => {
                step = value
                    .as_i64()
                    .filter(|step| step.abs() == 1)
                    .ok_or_else(|| Error::value("Step one preset forward or back."))?;
            }
            None => {
                index = get(request, "index")
                    .and_then(Value::as_u64)
                    .filter(|index| (7..=10).contains(index))
                    .ok_or_else(|| Error::value("Only custom presets 1 to 4 can be selected."))?
                    as u8;
            }
        },
        _ => {}
    }
    let access = Access { live: operation == "save", persistent: operation == "save", select: operation == "select" };
    let (mut device, transport) = connect(requested(request)?, |transport| open(access, transport))?;
    let mut state = device.read_state()?;
    state["transport"] = json!(transport.name());
    let mut result = serde_json::Map::new();
    match operation {
        "read" => {
            state["slots"] = stored_slots(device.as_mut(), &state)?;
            return Ok(state);
        }
        "save" => {
            if !device.has_storage_space()? {
                return Err(Error::value("Headset reports insufficient profile storage. No profile was saved."));
            }
            progress.write_started = true;
            if let Value::Object(saved) = device.save_coefficients(slot, &blob)? {
                result.extend(saved);
            }
            // Save returns only after a byte-for-byte read-back of the same key.
            progress.saved_verified = true;
            let active = custom_slot(&state) == Some(slot);
            if active {
                state["saved_coefficients_hex"] = json!(hex::encode(&blob));
                let live = device.apply_live(&blob)?;
                result.insert("ack_hex".into(), live.get("ack_hex").cloned().unwrap_or(Value::Null));
                // Custom slots 0..3 are presets 7..10.
                result.insert("preset_reloaded".into(), json!(device.reload_saved(7 + slot)?));
            }
            result.insert("slot".into(), json!(slot));
            result.insert("live_applied".into(), json!(active));
            result.insert("sha256".into(), json!(hex::encode(Sha256::digest(&blob))));
        }
        _ => {
            if step != 0 {
                index = stepped(&state, step);
            }
            let selected = state["index"].as_u64() != Some(u64::from(index));
            if selected {
                progress.write_started = true;
                if let Value::Object(ack) = device.select_preset(index)? {
                    result.extend(ack);
                }
                state = device.read_state()?;
                state["transport"] = json!(transport.name());
            }
            result.insert("selected".into(), json!(selected));
        }
    }
    result.insert("state".into(), state);
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
        ("saved_verified", json!(progress.saved_verified)),
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
    let mut open = |access, transport| crate::hid::open(transport, access);
    match execute(request, &mut open, &mut progress) {
        Ok(result) => (result, true),
        Err(error) => (failure(&error, &progress), false),
    }
}
