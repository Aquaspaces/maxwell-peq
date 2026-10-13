//! Equalizer APO text <-> Maxwell preset conversion. No device I/O.
//!
//! Import is lenient: anything the headset cannot reproduce is skipped or
//! clamped, and each change is reported as a warning so the user can see what
//! differs from the source file.

use crate::dsp::{MAX_BANDS, SHELF_Q};
use crate::fmt::{g, round};
use crate::json::{float, get, object};
use crate::{Error, Result};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

const TYPES: [(&str, &str); 11] = [
    ("PK", "peak"),
    ("PEQ", "peak"),
    ("MODAL", "peak"),
    ("LP", "low_pass"),
    ("LPQ", "low_pass"),
    ("HP", "high_pass"),
    ("HPQ", "high_pass"),
    ("LS", "low_shelf"),
    ("LSC", "low_shelf"),
    ("HS", "high_shelf"),
    ("HSC", "high_shelf"),
];
const UNSUPPORTED: [(&str, &str); 7] = [
    ("BP", "band-pass"),
    ("NO", "notch"),
    ("AP", "all-pass"),
    ("LP1", "first-order low-pass"),
    ("HP1", "first-order high-pass"),
    ("LS1", "first-order low shelf"),
    ("HS1", "first-order high shelf"),
];
const EXPORT: [(&str, &str); 5] =
    [("peak", "PK"), ("low_pass", "LPQ"), ("high_pass", "HPQ"), ("low_shelf", "LSC"), ("high_shelf", "HSC")];
const NUMBER: &str = r"([-+]?\d+(?:[.,]\d+)?)";

static FILTER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^filter\s*\d*\s*:\s*(?P<state>on|off)\s+(?P<type>[a-z]+\d?)(?P<rest>.*)$").unwrap()
});
static SLOPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)^\s*{NUMBER}\s*dB")).unwrap());
static LABELS: LazyLock<[(&str, Regex); 5]> = LazyLock::new(|| {
    [r"preamp\s*:", "fc", "gain", r"\bq", r"bw\s+oct"]
        .map(|label| (label, Regex::new(&format!(r"(?i){label}\s+{NUMBER}")).unwrap()))
});

fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn parse_number(text: &str) -> Option<f64> {
    text.replace(',', ".").parse().ok()
}

fn value(rest: &str, label: &str) -> Option<f64> {
    let (_, pattern) = LABELS.iter().find(|(l, _)| *l == label).expect("known label");
    pattern.captures(rest).and_then(|c| parse_number(&c[1]))
}

fn clamp(value: f64, low: f64, high: f64, what: &str, line: usize, warnings: &mut Vec<String>) -> f64 {
    if value < low || value > high {
        let clamped = high.min(low.max(value));
        warnings.push(format!(
            "Line {line}: {what} {} is outside {} to {}; using {}.",
            g(value),
            g(low),
            g(high),
            g(clamped)
        ));
        return clamped;
    }
    value
}

/// Python's `str.splitlines()`.
fn split_lines(text: &str) -> Vec<&str> {
    let breaks = ['\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}'];
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if breaks.contains(&c) {
            lines.push(&text[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                chars.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `str.strip()` whitespace.
pub fn py_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

fn from_json(text: &str) -> Result<Value> {
    let data: Value = serde_json::from_str(text)?;
    let Some(rows) = get(&data, "filters").and_then(Value::as_array) else {
        return Err(Error::value("JSON presets need a \"filters\" list."));
    };
    let mut filters = Vec::new();
    for (i, row) in rows.iter().take(MAX_BANDS).enumerate() {
        if !row.is_object() {
            return Err(Error::value(format!("Filter {} must be an object.", i + 1)));
        }
        let kind = match get(row, "type") {
            None => "peak",
            Some(Value::String(name)) if name == "PK" => "peak",
            Some(Value::String(name)) => lookup(&EXPORT, name).map(|_| name.as_str()).unwrap_or(""),
            Some(_) => "",
        };
        if kind.is_empty() {
            return Err(Error::value(format!("Filter {}: unknown filter type.", i + 1)));
        }
        let is_pass = kind == "low_pass" || kind == "high_pass";
        let is_shelf = kind == "low_shelf" || kind == "high_shelf";
        let field = |key| get(row, key).cloned().unwrap_or(Value::Null);
        let q = if is_shelf {
            float(SHELF_Q)
        } else {
            get(row, "q").cloned().unwrap_or(if is_pass { float(SHELF_Q) } else { Value::Null })
        };
        filters.push(object([
            ("type", Value::from(kind)),
            ("frequency", field("frequency")),
            ("gain", if is_pass { json!(0) } else { field("gain") }),
            ("q", q),
            ("enabled", Value::Bool(get(row, "enabled") != Some(&Value::Bool(false)))),
        ]));
    }
    if filters.is_empty() {
        return Err(Error::value("The preset has no filters."));
    }
    let mut warnings = Vec::new();
    if rows.len() > MAX_BANDS {
        warnings.push(format!("Only the first {MAX_BANDS} of {} filters were imported.", rows.len()));
    }
    Ok(object([
        ("filters", Value::Array(filters)),
        ("preamp", get(&data, "preamp").cloned().unwrap_or_else(|| Value::from("auto"))),
        ("source_preamp", Value::Null),
        ("warnings", Value::from(warnings)),
    ]))
}

/// Filters, a suggested preamp, the file's own preamp, and warnings.
pub fn parse(text: &Value) -> Result<Value> {
    let Value::String(text) = text else {
        return Err(Error::value("Paste Equalizer APO text or choose a .txt file."));
    };
    if text.chars().count() > 200_000 {
        return Err(Error::value("That file is too large for an Equalizer APO preset."));
    }
    if py_trim(text).starts_with('{') || text.trim_start().starts_with('{') {
        return from_json(text);
    }
    let (mut filters, mut warnings) = (Vec::new(), Vec::new());
    let mut source_preamp: Option<f64> = None;
    for (index, raw) in split_lines(text).into_iter().enumerate() {
        let number = index + 1;
        let line = py_trim(raw.split('#').next().unwrap_or(""));
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        if lower.starts_with("preamp") {
            if let Some(value) = value(line, r"preamp\s*:") {
                source_preamp = Some(source_preamp.unwrap_or(0.0) + value);
            }
            continue;
        }
        if lower.starts_with("graphiceq") {
            return Err(Error::value(
                "GraphicEQ presets are not parametric. Export a parametric (PK filter) preset instead.",
            ));
        }
        let Some(found) = FILTER_LINE.captures(line) else {
            let command = py_trim(line.split(':').next().unwrap_or(""));
            let command = if command.is_empty() { line } else { command };
            let command: String = command.chars().take(40).collect();
            warnings.push(format!("Line {number}: ignored \"{command}\"; only Preamp and Filter lines are used."));
            continue;
        };
        let code = found["type"].to_uppercase();
        let mut rest = found.name("rest").map_or("", |m| m.as_str());
        if let Some(name) = lookup(&UNSUPPORTED, &code) {
            warnings.push(format!("Line {number}: skipped a {name} filter; the headset cannot use it."));
            continue;
        }
        let Some(kind) = lookup(&TYPES, &code) else {
            warnings.push(format!("Line {number}: skipped unknown filter type {code}."));
            continue;
        };
        if let Some(slope) = SLOPE.captures(rest) {
            let degrees = slope.get(1).expect("group").as_str();
            if code == "LS" || code == "HS" {
                warnings.push(format!(
                    "Line {number}: skipped a {code} {degrees} dB corner-frequency shelf; only center-frequency S=1 shelves are supported."
                ));
                continue;
            }
            rest = &rest[slope.get(0).expect("match").end()..];
            if parse_number(degrees) != Some(12.0) {
                warnings
                    .push(format!("Line {number}: shelf slope {degrees} dB changed to the headset's fixed S=1 slope."));
            }
        }
        let Some(frequency) = value(rest, "fc") else {
            warnings.push(format!("Line {number}: skipped a filter with no Fc frequency."));
            continue;
        };
        let frequency = clamp(frequency, 20.0, 20000.0, "Frequency", number, &mut warnings);
        let mut gain = value(rest, "gain");
        let mut q = value(rest, r"\bq");
        let bandwidth = value(rest, r"bw\s+oct");
        if let (None, Some(bandwidth)) = (q, bandwidth)
            && bandwidth > 0.0
        {
            q = Some(2f64.powf(bandwidth).sqrt() / (2f64.powf(bandwidth) - 1.0));
        }
        let q = match kind {
            "low_pass" | "high_pass" => {
                if let Some(ignored) = gain.filter(|&gain| gain != 0.0) {
                    warnings.push(format!("Line {number}: pass filters have no gain; ignored {} dB.", g(ignored)));
                }
                gain = Some(0.0);
                match q {
                    None => SHELF_Q,
                    Some(q) => clamp(q, 0.1, 20.0, "Q", number, &mut warnings),
                }
            }
            "low_shelf" | "high_shelf" => {
                gain = Some(clamp(gain.unwrap_or(0.0), -12.0, 12.0, "Gain", number, &mut warnings));
                if let Some(q) = q.filter(|q| (q - SHELF_Q).abs() > 0.02) {
                    warnings.push(format!(
                        "Line {number}: shelf Q {} changed to 0.707; the headset uses a fixed S=1 slope.",
                        g(q)
                    ));
                }
                SHELF_Q
            }
            _ => {
                if gain.is_none() {
                    warnings.push(format!("Line {number}: peak had no gain; using 0 dB."));
                }
                gain = Some(clamp(gain.unwrap_or(0.0), -12.0, 12.0, "Gain", number, &mut warnings));
                let q = q.unwrap_or_else(|| {
                    warnings.push(format!("Line {number}: peak had no Q; using 1."));
                    1.0
                });
                clamp(q, 0.1, 20.0, "Q", number, &mut warnings)
            }
        };
        filters.push(object([
            ("type", Value::from(kind)),
            ("frequency", float(round(frequency, 1))),
            ("gain", float(round(gain.unwrap_or(0.0), 2))),
            ("q", float(if q == SHELF_Q { q } else { round(q, 3) })),
            ("enabled", Value::Bool(found["state"].eq_ignore_ascii_case("on"))),
        ]));
    }
    if filters.is_empty() {
        return Err(Error::value(
            "No usable filters found. Expected lines like \"Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 1\".",
        ));
    }
    if filters.len() > MAX_BANDS {
        warnings.push(format!(
            "The headset holds {MAX_BANDS} filters; imported the first {MAX_BANDS} of {}.",
            filters.len()
        ));
        filters.truncate(MAX_BANDS);
    }
    if let Some(preamp) = source_preamp {
        warnings
            .push(format!("The file sets Preamp {} dB. Auto preamp is used instead so boosts cannot clip.", g(preamp)));
    }
    Ok(object([
        ("filters", Value::Array(filters)),
        ("preamp", Value::from("auto")),
        ("source_preamp", source_preamp.map_or(Value::Null, float)),
        ("warnings", Value::from(warnings)),
    ]))
}

fn number_text(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".into() } else { text.into() }
}

/// Python's `float(x)` for the values a checked preset can hold.
fn to_float(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::String(text) => {
            py_trim(text).parse().map_err(|_| Error::value(format!("could not convert string to float: '{text}'")))
        }
        _ => Err(Error::value("Filter values must be numbers.")),
    }
}

/// Equalizer APO text for a checked preset; `preamp_db` stands in for an "auto" preamp.
pub fn export(preset: &Value, preamp_db: &Value) -> Result<String> {
    let empty = Vec::new();
    let filters = get(preset, "filters").and_then(Value::as_array).unwrap_or(&empty);
    let mut lines = Vec::new();
    match get(preset, "preamp") {
        Some(preamp) if preamp != "auto" => lines.push(format!("Preamp: {} dB", number_text(to_float(preamp)?))),
        _ if !preamp_db.is_null() => lines.push(format!("Preamp: {} dB", number_text(to_float(preamp_db)?))),
        _ => {}
    }
    for (i, f) in filters.iter().enumerate() {
        let kind = get(f, "type").and_then(Value::as_str).unwrap_or("peak");
        let code = lookup(&EXPORT, kind).ok_or_else(|| Error::value(format!("Unsupported filter type: {kind}.")))?;
        let state = if get(f, "enabled").is_none_or(crate::json::truthy) { "ON" } else { "OFF" };
        let field = |key| to_float(get(f, key).unwrap_or(&Value::Null));
        let mut line = format!("Filter {}: {state} {code} Fc {} Hz", i + 1, number_text(field("frequency")?));
        if kind != "low_pass" && kind != "high_pass" {
            line += &format!(" Gain {} dB", number_text(field("gain")?));
        }
        let q = if kind == "low_shelf" || kind == "high_shelf" { SHELF_Q } else { field("q")? };
        line += &format!(" Q {}", number_text(q));
        lines.push(line);
    }
    Ok(lines.join("\n") + "\n")
}
