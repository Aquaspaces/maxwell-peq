//! Preset validation and the four-rate coefficient compiler. No device I/O.

use crate::dsp::{Filter, Kind, MAX_BANDS, RATES, SHELF_Q, decode_words, filter_headroom_db, rbj_biquad, response};
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
    // Analytical per-stage bounds include resonant pass filters; no sampled estimate.
    let enabled: Vec<f64> = filters.iter().filter(|f| f.enabled).map(filter_headroom_db).collect();
    let headroom = py_sum(&enabled);
    // Python's sum() of nothing is the integer 0, so "auto" is +0 rather than -0 then.
    let auto = if enabled.is_empty() { 0.0 } else { -headroom };
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

/// Python 3.12+ `sum()` of floats: Neumaier-compensated, left to right.
fn py_sum(values: &[f64]) -> f64 {
    let (mut sum, mut compensation) = (0.0f64, 0.0f64);
    for &x in values {
        let t = sum + x;
        compensation += if sum.abs() >= x.abs() { (sum - t) + x } else { (x - t) + sum };
        sum = t;
    }
    if compensation != 0.0 && compensation.is_finite() { sum + compensation } else { sum }
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

/// Dense log grid plus centre and near-centre probes, to catch narrow-filter errors.
fn probe_grid(filters: &[Filter]) -> Vec<f64> {
    let mut frequencies: Vec<f64> = (0..768).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 767.0)).collect();
    for f in filters {
        for multiplier in [0.5, 0.9, 0.99, 1.0, 1.01, 1.1, 2.0] {
            frequencies.push((f.frequency * multiplier).clamp(20.0, 20000.0));
        }
    }
    frequencies
}

fn sorted_unique(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values.dedup();
    values
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
pub fn compile_preset(request: &Value) -> Result<Value> {
    compile_with(request, Encoder::from_env())
}

pub fn compile_with(request: &Value, encoder: Encoder) -> Result<Value> {
    let (filters, preamp) = validate(request)?;
    let shelf = filters.iter().any(|f| f.enabled && f.kind.is_shelf());
    let dll = verified_dll().is_ok();
    if encoder == Encoder::Auto && dll && !shelf {
        with_vendor(|vendor| build(&filters, preamp, Some(vendor), dll))
    } else {
        build(&filters, preamp, None, dll)
    }
}

/// Encode every rate with `vendor`, or with the built-in encoder when it is `None`.
fn build(filters: &[Filter], preamp: f64, vendor: Option<&Vendor>, dll: bool) -> Result<Value> {
    let extended = vendor.is_none();
    let experimental = filters.iter().any(|f| f.enabled && f.kind != Kind::Peak);
    let shelf = filters.iter().any(|f| f.enabled && f.kind.is_shelf());
    // The built-in encoder gets the same endpoint checks as the experimental filter types.
    let checked = experimental || extended;
    let grid = probe_grid(filters);
    let (mut banks, mut curves) = (Vec::new(), Vec::new());
    let mut blob = (RATES.len() as u32).to_le_bytes().to_vec();
    let mut max_error: f64 = 0.0;
    for (rate_id, rate) in RATES {
        let rate = f64::from(rate);
        let words = match vendor {
            Some(vendor) => vendor.bank(filters, preamp, rate)?,
            None if shelf => encode_bank(filters, preamp, rate)?,
            None => substitute_bank(filters, preamp, rate)?,
        };
        let (stages, scale) = decode_words(&words)?;
        let ideal: Vec<_> = filters.iter().filter(|f| f.enabled).map(|f| rbj_biquad(f, rate)).collect();
        let mut probes = grid.clone();
        for f in filters {
            if f.enabled && f.kind.is_pass() && f.q > SHELF_Q {
                let ratio = (1.0 - 1.0 / (2.0 * f.q.powf(2.0))).sqrt();
                let ratio = if f.kind == Kind::LowPass { ratio } else { 1.0 / ratio };
                probes.push(rate / PI * ((PI * f.frequency / rate).tan() * ratio).atan());
            }
        }
        // Endpoints check shelf plateaus and passband gain; the floor makes deep
        // stopband nulls comparable without dividing by a nearly-zero response.
        if checked {
            probes.extend([0.0, rate / 2.0]);
        }
        let (mut points, mut error) = (Vec::new(), 0.0f64);
        let expected_scale = 10f64.powf(preamp / 20.0);
        for frequency in sorted_unique(probes) {
            let actual = response(&stages, scale, frequency, rate);
            let expected = response(&ideal, expected_scale, frequency, rate);
            let difference =
                if checked { (actual.max(-90.0) - expected.max(-90.0)).abs() } else { (actual - expected).abs() };
            error = error.max(difference);
            if actual > 0.15 {
                return Err(Error::value("Compiled response exceeds available headroom."));
            }
            if (20.0..=20000.0).contains(&frequency) {
                points.push(json!([float(frequency), float(actual)]));
            }
        }
        if error > 0.15 {
            return Err(Error::value(format!(
                "Coefficient precision error is {error:.3} dB at {rate} Hz; try a higher frequency, smaller gain, or lower Q where adjustable."
            )));
        }
        max_error = max_error.max(error);
        banks.push(object([
            ("id", Value::from(rate_id)),
            ("rate", Value::from(rate as u32)),
            ("words", Value::from(words.clone())),
        ]));
        curves.push(object([
            ("rate", Value::from(rate as u32)),
            ("points", Value::Array(points)),
            ("max_error_db", float(error)),
        ]));
        blob.extend(rate_id.to_le_bytes());
        blob.extend((words.len() as u16).to_le_bytes());
        blob.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    }
    if blob.len() != BLOB_BYTES {
        return Err(Error::value("Unexpected multi-rate payload size."));
    }
    Ok(object([
        ("filters", Value::Array(filters.iter().map(filter_json).collect())),
        ("preamp", float(preamp)),
        ("banks", Value::Array(banks)),
        ("curves", Value::Array(curves)),
        ("max_error_db", float(max_error)),
        ("blob_hex", Value::from(hex::encode(&blob))),
        ("sha256", Value::from(hex::encode(Sha256::digest(&blob)))),
        ("dll_sha256", if dll { Value::from(DLL_SHA256) } else { Value::Null }),
        ("hardware_verified", Value::Bool(false)),
        ("experimental_filters", Value::Bool(experimental)),
        ("engine", Value::from(if extended { "extended-rbj" } else { "vendor" })),
        ("error_floor_db", if checked { json!(-90) } else { Value::Null }),
    ]))
}
