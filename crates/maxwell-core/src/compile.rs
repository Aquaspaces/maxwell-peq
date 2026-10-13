//! Preset validation and the four-rate coefficient compiler. No device I/O.

use crate::dsp::{Filter, Kind, MAX_BANDS, RATES, SHELF_Q, decode_words, rbj_biquad, response};
use crate::encoder::{encode_bank, substitute_bank};
use crate::fmt::{g, py_str};
use crate::json::{float, get, object};
use crate::vendor::{DLL_SHA256, Vendor, verified_dll, with_vendor};
use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::f64::consts::PI;

/// The blob's size: a u32 bank count, then four banks of id, length and 103 words.
pub const BLOB_BYTES: usize = 844;

/// The largest sampled difference from the ideal response a bank may have.
/// Narrow sub-bass peaks reach about 0.5 dB at 88.2 and 96 kHz on the 24-bit grid.
const MAX_ERROR_DB: f64 = 0.5;

/// `peq.number`: a finite number in range, never a bool.
pub fn number(value: Option<&Value>, name: &str, low: f64, high: f64) -> Result<f64> {
    let Some(Value::Number(value)) = value else {
        return Err(Error::value(format!("{name} must be a number.")));
    };
    let value = value.as_f64().unwrap_or(f64::NAN);
    if !value.is_finite() || !(low..=high).contains(&value) {
        return Err(Error::value(format!("{name} must be between {} and {}.", g(low), g(high))));
    }
    Ok(value)
}

fn is_close(a: f64, b: f64, abs_tol: f64) -> bool {
    a == b || (a - b).abs() <= (1e-9 * a.abs().max(b.abs())).max(abs_tol)
}

/// Check a preset request; returns the filters and the effective preamp in dB.
pub fn validate(request: &Value) -> Result<(Vec<Filter>, f64)> {
    let Value::Object(request) = request else {
        return Err(Error::value("A preset must be a JSON object."));
    };
    let rows = match request.get("filters") {
        Some(Value::Array(rows)) if (1..=MAX_BANDS).contains(&rows.len()) => rows,
        _ => return Err(Error::value("Use between 1 and 10 filters.")),
    };
    let mut filters = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if !row.is_object() {
            return Err(Error::value(format!("Filter {} must be an object.", i + 1)));
        }
        let kind = match get(row, "type") {
            None => Some(Kind::Peak),
            Some(Value::String(name)) if name == "PK" => Some(Kind::Peak),
            Some(Value::String(name)) => Kind::parse(name),
            Some(_) => None,
        }
        .ok_or_else(|| {
            Error::value(format!("Unsupported filter type: {}.", py_str(get(row, "type").unwrap_or(&Value::Null))))
        })?;
        let enabled = match get(row, "enabled") {
            None => true,
            Some(Value::Bool(enabled)) => *enabled,
            Some(_) => return Err(Error::value("Enabled must be true or false.")),
        };
        let zero = json!(0);
        let shelf_q = float(SHELF_Q);
        let gain = number(get(row, "gain").or(kind.is_pass().then_some(&zero)), "Gain", -12.0, 12.0)?;
        let q = number(get(row, "q").or((kind.is_shelf() || kind.is_pass()).then_some(&shelf_q)), "Q", 0.1, 20.0)?;
        if kind.is_pass() && gain != 0.0 {
            return Err(Error::value("Pass filters do not use gain; set gain to 0."));
        }
        if kind.is_shelf() && !is_close(q, SHELF_Q, 1e-5) {
            return Err(Error::value("Shelf filters use fixed slope S=1 (Q=0.70710678)."));
        }
        let frequency = number(get(row, "frequency"), "Frequency", 20.0, 20000.0)?;
        filters.push(Filter { kind, frequency, gain, q: if kind.is_shelf() { SHELF_Q } else { q }, enabled });
    }
    let headroom = headroom_db(&filters);
    let auto = if headroom > 0.0 { -headroom } else { 0.0 };
    let preamp = match request.get("preamp") {
        None => auto,
        Some(Value::String(text)) if text == "auto" => auto,
        requested => number(requested, "Preamp", -60.0, 0.0)?,
    };
    if preamp < -60.0 {
        return Err(Error::value("This filter set needs more than 60 dB of headroom; reduce the boosts."));
    }
    if preamp > -headroom + 1e-9 {
        return Err(Error::value(format!("Use Auto preamp or {} dB or lower for these boosts.", g(-headroom))));
    }
    Ok((filters, preamp))
}

/// The ideal cascade's highest gain at any rate, rounded up to 0.01 dB; 0 for cuts.
///
/// Boosts at different frequencies don't stack, so this is the curve's real peak,
/// not the sum of every boost. It is sampled on the compiler's probe grid, which
/// includes each filter's centre and each resonant pass filter's peak.
fn headroom_db(filters: &[Filter]) -> f64 {
    let mut peak: f64 = 0.0;
    for (_, rate) in RATES {
        let rate = f64::from(rate);
        let stages: Vec<_> = filters.iter().filter(|f| f.enabled).map(|f| rbj_biquad(f, rate)).collect();
        for frequency in probes(filters, rate, true) {
            peak = peak.max(response(&stages, 1.0, frequency, rate));
        }
    }
    let rounded = (peak * 100.0 - 1e-6).ceil() / 100.0;
    if rounded > 0.0 { rounded } else { 0.0 }
}

pub fn filter_json(f: &Filter) -> Value {
    object([
        ("type", Value::from(f.kind.name())),
        ("frequency", float(f.frequency)),
        ("gain", float(f.gain)),
        ("q", float(f.q)),
        ("enabled", Value::Bool(f.enabled)),
    ])
}

/// Dense log grid plus centre and near-centre probes, to catch narrow-filter errors,
/// plus each resonant pass filter's peak and, with `endpoints`, DC and Nyquist.
fn probes(filters: &[Filter], rate: f64, endpoints: bool) -> Vec<f64> {
    let mut frequencies: Vec<f64> = (0..768).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 767.0)).collect();
    for f in filters {
        for multiplier in [0.5, 0.9, 0.99, 1.0, 1.01, 1.1, 2.0] {
            frequencies.push((f.frequency * multiplier).clamp(20.0, 20000.0));
        }
    }
    for f in filters {
        if f.enabled && f.kind.is_pass() && f.q > SHELF_Q {
            let ratio = (1.0 - 1.0 / (2.0 * f.q.powf(2.0))).sqrt();
            let ratio = if f.kind == Kind::LowPass { ratio } else { 1.0 / ratio };
            frequencies.push(rate / PI * ((PI * f.frequency / rate).tan() * ratio).atan());
        }
    }
    if endpoints {
        frequencies.extend([0.0, rate / 2.0]);
    }
    frequencies.sort_by(f64::total_cmp);
    frequencies.dedup();
    frequencies
}

/// Which coefficient encoder builds peak and pass filters. Shelves always use the built-in one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    /// The Audeze DLL when it is installed and verified, otherwise the built-in encoder.
    Auto,
    /// The built-in RBJ encoder, even when the DLL is available.
    BuiltIn,
}

impl Encoder {
    /// `MAXWELL_PEQ_ENCODER=built-in` forces the built-in encoder, to compare the two by ear.
    pub fn from_env() -> Self {
        match std::env::var("MAXWELL_PEQ_ENCODER").as_deref() {
            Ok("built-in") => Encoder::BuiltIn,
            _ => Encoder::Auto,
        }
    }
}

/// Compile a preset request into the four-rate blob, with per-rate response checks.
///
/// A bank that can't be encoded precisely gets the nearest values that can (see
/// [`nearest_bank`]), and an Auto preamp drops further if quantization lifts the
/// compiled response past 0 dB. The result's `adjustments` lists each change.
pub fn compile_preset(request: &Value) -> Result<Value> {
    compile(request, Encoder::from_env(), true)
}

/// Compile exactly what was asked for, failing instead of adjusting anything.
pub fn compile_with(request: &Value, encoder: Encoder) -> Result<Value> {
    compile(request, encoder, false)
}

fn compile(request: &Value, encoder: Encoder, adjust: bool) -> Result<Value> {
    let (filters, preamp) = validate(request)?;
    // validate() accepts no string but "auto".
    let adjust = adjust.then(|| matches!(request.get("preamp"), None | Some(Value::String(_))));
    let shelf = filters.iter().any(|f| f.enabled && f.kind.is_shelf());
    let dll = verified_dll().is_ok();
    if encoder == Encoder::Auto && dll && !shelf {
        with_vendor(|vendor| build(&filters, preamp, Some(vendor), dll, adjust))
    } else {
        build(&filters, preamp, None, dll, adjust)
    }
}

/// One encoded bank and how its decoded response compares with the ideal one.
struct Bank {
    words: Vec<u16>,
    points: Vec<Value>,
    error: f64,
    peak: f64,
}

impl Bank {
    fn precise(&self) -> bool {
        self.error <= MAX_ERROR_DB
    }
}

/// Encode one rate with `vendor`, or with the built-in encoder when it is `None`.
fn bank(filters: &[Filter], preamp: f64, rate: f64, vendor: Option<&Vendor>) -> Result<Bank> {
    let experimental = filters.iter().any(|f| f.enabled && f.kind != Kind::Peak);
    let shelf = filters.iter().any(|f| f.enabled && f.kind.is_shelf());
    // The built-in encoder gets the same endpoint checks as the experimental filter types.
    let checked = experimental || vendor.is_none();
    let words = match vendor {
        Some(vendor) => vendor.bank(filters, preamp, rate)?,
        None if shelf => encode_bank(filters, preamp, rate)?,
        None => substitute_bank(filters, preamp, rate)?,
    };
    let (stages, scale) = decode_words(&words)?;
    let ideal: Vec<_> = filters.iter().filter(|f| f.enabled).map(|f| rbj_biquad(f, rate)).collect();
    let expected_scale = 10f64.powf(preamp / 20.0);
    let (mut points, mut error, mut peak) = (Vec::new(), 0.0f64, f64::NEG_INFINITY);
    // Endpoints check shelf plateaus and passband gain; the floor makes deep
    // stopband nulls comparable without dividing by a nearly-zero response.
    for frequency in probes(filters, rate, checked) {
        let actual = response(&stages, scale, frequency, rate);
        let expected = response(&ideal, expected_scale, frequency, rate);
        let difference =
            if checked { (actual.max(-90.0) - expected.max(-90.0)).abs() } else { (actual - expected).abs() };
        error = error.max(difference);
        peak = peak.max(actual);
        if (20.0..=20000.0).contains(&frequency) {
            points.push(json!([float(frequency), float(actual)]));
        }
    }
    Ok(Bank { words, points, error, peak })
}

/// How far [`nearest_bank`] looks: frequency rises up to an octave in 1/96-octave
/// steps while Q falls to a quarter in 1/48-octave steps.
const ADJUST_STEPS: i32 = 96;

/// The smallest change that makes one rate's bank precise: the built-in encoder in
/// place of the DLL, else one filter's frequency raised or Q lowered, rounded to what
/// the editor shows. Low, narrow filters are the ones that lose precision at 88.2
/// and 96 kHz, so at each step the lowest filters are tried first.
fn nearest_bank(filters: &[Filter], preamp: f64, rate: f64, vendor: Option<&Vendor>) -> Option<(Bank, Vec<Value>)> {
    if vendor.is_some()
        && let Ok(found) = bank(filters, preamp, rate, None)
        && found.precise()
    {
        return Some((found, vec![json!({"rate": rate as u32, "encoder": "built-in"})]));
    }
    let mut order: Vec<usize> = (0..filters.len()).filter(|&i| filters[i].enabled).collect();
    order.sort_by(|&a, &b| filters[a].frequency.total_cmp(&filters[b].frequency));
    // The last value tried per filter and field; rounding repeats values at small steps.
    let mut tried = vec![[f64::NAN; 2]; filters.len()];
    for step in 1..=ADJUST_STEPS {
        let step = f64::from(step);
        for &index in &order {
            let f = filters[index];
            let frequency = ((f.frequency * 2f64.powf(step / 96.0) * 10.0).round() / 10.0).min(20000.0);
            let q = ((f.q * 2f64.powf(-step / 48.0) * 100.0).round() / 100.0).max(0.1);
            let candidates =
                [("frequency", f.frequency, frequency, Filter { frequency, ..f }), ("q", f.q, q, Filter { q, ..f })];
            for (field, (name, from, to, candidate)) in candidates.into_iter().enumerate() {
                if to == from || to == tried[index][field] || (name == "q" && f.kind.is_shelf()) {
                    continue;
                }
                tried[index][field] = to;
                let mut adjusted = filters.to_vec();
                adjusted[index] = candidate;
                if let Ok(found) = bank(&adjusted, preamp, rate, vendor)
                    && found.precise()
                {
                    let change = json!({
                        "rate": rate as u32, "filter": index, "field": name, "from": float(from), "to": float(to)
                    });
                    return Some((found, vec![change]));
                }
            }
        }
    }
    None
}

/// Encode and check every rate, then assemble the blob.
///
/// `adjust` is `None` to fail on the first imprecise bank, or `Some(auto)` to use
/// the nearest precise one instead; with an Auto preamp, the preamp may also drop
/// to keep the compiled response under 0 dB.
fn build(
    filters: &[Filter],
    mut preamp: f64,
    vendor: Option<&Vendor>,
    dll: bool,
    adjust: Option<bool>,
) -> Result<Value> {
    let auto = adjust == Some(true);
    for attempt in 0.. {
        let (mut banks, mut adjustments) = (Vec::new(), Vec::new());
        for (rate_id, rate) in RATES {
            let rate = f64::from(rate);
            let mut result = bank(filters, preamp, rate, vendor);
            if adjust.is_some()
                && !result.as_ref().is_ok_and(Bank::precise)
                && let Some((found, changes)) = nearest_bank(filters, preamp, rate, vendor)
            {
                result = Ok(found);
                adjustments.extend(changes);
            }
            let bank = result?;
            if !auto && bank.peak > 0.15 {
                return Err(Error::value("Compiled response exceeds available headroom."));
            }
            if !bank.precise() {
                return Err(Error::value(format!(
                    "Coefficient precision error is {:.3} dB at {rate} Hz; try a higher frequency, smaller gain, or lower Q where adjustable.",
                    bank.error
                )));
            }
            banks.push((rate_id, rate, bank));
        }
        let peak = banks.iter().map(|(_, _, bank)| bank.peak).fold(f64::NEG_INFINITY, f64::max);
        if peak > 0.15 {
            // Quantization can lift a boost a little past the ideal curve's peak.
            if auto && attempt < 3 && preamp > -60.0 {
                preamp = (preamp - (peak * 10.0).ceil() / 10.0).max(-60.0);
                continue;
            }
            return Err(Error::value("Compiled response exceeds available headroom."));
        }
        return assemble(filters, preamp, banks, adjust.map(|_| adjustments), vendor.is_none(), dll);
    }
    unreachable!("the attempt loop always returns")
}

fn assemble(
    filters: &[Filter],
    preamp: f64,
    banks: Vec<(u16, f64, Bank)>,
    adjustments: Option<Vec<Value>>,
    extended: bool,
    dll: bool,
) -> Result<Value> {
    let experimental = filters.iter().any(|f| f.enabled && f.kind != Kind::Peak);
    let checked = experimental || extended;
    let mut blob = (RATES.len() as u32).to_le_bytes().to_vec();
    let (mut bank_json, mut curves, mut max_error) = (Vec::new(), Vec::new(), 0.0f64);
    for (rate_id, rate, bank) in banks {
        max_error = max_error.max(bank.error);
        blob.extend(rate_id.to_le_bytes());
        blob.extend((bank.words.len() as u16).to_le_bytes());
        blob.extend(bank.words.iter().flat_map(|word| word.to_le_bytes()));
        bank_json.push(object([
            ("id", Value::from(rate_id)),
            ("rate", Value::from(rate as u32)),
            ("words", Value::from(bank.words)),
        ]));
        curves.push(object([
            ("rate", Value::from(rate as u32)),
            ("points", Value::Array(bank.points)),
            ("max_error_db", float(bank.error)),
        ]));
    }
    if blob.len() != BLOB_BYTES {
        return Err(Error::value("Unexpected multi-rate payload size."));
    }
    let mut result = object([
        ("filters", Value::Array(filters.iter().map(filter_json).collect())),
        ("preamp", float(preamp)),
        ("banks", Value::Array(bank_json)),
        ("curves", Value::Array(curves)),
        ("max_error_db", float(max_error)),
        ("blob_hex", Value::from(hex::encode(&blob))),
        ("sha256", Value::from(hex::encode(Sha256::digest(&blob)))),
        ("dll_sha256", if dll { Value::from(DLL_SHA256) } else { Value::Null }),
        ("hardware_verified", Value::Bool(false)),
        ("experimental_filters", Value::Bool(experimental)),
        ("engine", Value::from(if extended { "extended-rbj" } else { "vendor" })),
        ("error_floor_db", if checked { json!(-90) } else { Value::Null }),
    ]);
    if let Some(adjustments) = adjustments {
        result["adjustments"] = Value::Array(adjustments);
    }
    Ok(result)
}
