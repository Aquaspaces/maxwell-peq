//! Channel balance and the source profile, from kats1123's
//! maxwell-firmware-research (CC BY 4.0) and maxwell-balance-and-firmware-tool (MIT).
//!
//! Unlike EQ, these writes have no temporary form: the firmware applies a gain
//! and stores it in NVDM in the same step. Every write is read back, and runs
//! report the values they found first so they can be put back.

use crate::hid::Transport;
use crate::json::{get, object};
use crate::protocol::{LEFT_GAIN, RIGHT_GAIN, SOURCE_PROFILE, USB_C_PROFILE, WIRELESS_PROFILE, check_gain};
use crate::{Error, Result};
use serde_json::{Value, json};

pub const OPERATIONS: [&str; 3] = ["balance_read", "balance_write", "source_write"];

/// The headset settings a balance run needs.
pub trait Settings {
    /// A value that two reads in a row agree on.
    fn read_setting(&mut self, sub: u8) -> Result<u8>;
    /// Write, then read back until the headset holds `value`. Returns the raw
    /// write reply when one arrived.
    fn write_setting(&mut self, sub: u8, value: u8) -> Result<Option<Vec<u8>>>;
}

/// `open(allow_write, transport)`; production uses [`crate::hid::open_settings`].
pub type OpenSettings<'a> = dyn FnMut(bool, Transport) -> Result<Box<dyn Settings>> + 'a;

/// How far a run got; reported with any error.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Progress {
    pub write_started: bool,
    pub previous: Option<Value>,
}

pub fn source_name(value: u8) -> &'static str {
    if value == USB_C_PROFILE { "usb-c" } else { "wireless" }
}

fn gain(request: &Value, key: &str) -> Result<u8> {
    let value = get(request, key)
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| Error::value(format!("Expected a whole-number {key} gain.")))?;
    check_gain(value)?;
    Ok(value)
}

fn read_all(device: &mut dyn Settings) -> Result<Value> {
    let left = device.read_setting(LEFT_GAIN)?;
    let right = device.read_setting(RIGHT_GAIN)?;
    let source = device.read_setting(SOURCE_PROFILE)?;
    Ok(object([
        ("left", json!(left)),
        ("right", json!(right)),
        ("source", json!(source)),
        ("source_profile", json!(source_name(source))),
    ]))
}

fn transport(request: &Value) -> Result<Transport> {
    match get(request, "transport") {
        None => Ok(Transport::Usb),
        Some(value) => {
            value.as_str().and_then(Transport::parse).ok_or_else(|| Error::value("Choose usb or dongle transport."))
        }
    }
}

pub fn execute(request: &Value, open: &mut OpenSettings, progress: &mut Progress) -> Result<Value> {
    *progress = Progress::default();
    let operation = get(request, "operation").and_then(Value::as_str).unwrap_or_default();
    let writes: Vec<(u8, u8)> = match operation {
        "balance_read" => Vec::new(),
        "balance_write" => vec![(LEFT_GAIN, gain(request, "left")?), (RIGHT_GAIN, gain(request, "right")?)],
        "source_write" => match get(request, "source").and_then(Value::as_str) {
            Some("usb-c") => vec![(SOURCE_PROFILE, USB_C_PROFILE)],
            Some("wireless") => vec![(SOURCE_PROFILE, WIRELESS_PROFILE)],
            _ => return Err(Error::value("Choose the usb-c or wireless source profile.")),
        },
        _ => return Err(Error::value("Unsupported device operation.")),
    };
    let transport = transport(request)?;
    let mut device = open(!writes.is_empty(), transport)?;
    let before = read_all(device.as_mut())?;
    if writes.is_empty() {
        return Ok(before);
    }
    progress.previous = Some(before.clone());
    let mut replies = Vec::new();
    for (sub, value) in writes {
        let current = match sub {
            LEFT_GAIN => &before["left"],
            RIGHT_GAIN => &before["right"],
            _ => &before["source"],
        };
        if current == &json!(value) {
            continue;
        }
        progress.write_started = true;
        if let Some(reply) = device.write_setting(sub, value)? {
            replies.push(json!(hex::encode(reply)));
        }
    }
    let mut after = read_all(device.as_mut())?;
    after["previous"] = before;
    after["write_replies_hex"] = Value::Array(replies);
    Ok(after)
}

/// The worker's stdout on failure.
pub fn failure(error: &Error, progress: &Progress) -> Value {
    let mut message = error.to_string();
    if !progress.write_started && !message.contains("was changed") {
        message.push_str(" Nothing on the headset was changed.");
    }
    object([
        ("error", json!(message)),
        ("write_started", json!(progress.write_started)),
        ("previous", progress.previous.clone().unwrap_or(Value::Null)),
    ])
}

/// Run one request against the real headset.
pub fn run(request: &Value) -> (Value, bool) {
    let mut progress = Progress::default();
    let mut open = |write, transport| crate::hid::open_settings(transport, write);
    match execute(request, &mut open, &mut progress) {
        Ok(result) => (result, true),
        Err(error) => (failure(&error, &progress), false),
    }
}
