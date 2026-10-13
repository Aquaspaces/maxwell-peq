//! Parity with the Python reference. Fixtures come from tools/make_golden.py.

use maxwell_core::{apo, compile, fmt, vendor};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Structural equality where numbers compare as f64 (`1 == 1.0`, `-0.0 == 0`).
/// Returns the first difference's path.
fn diff(actual: &Value, expected: &Value, path: &str) -> Option<String> {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            (a != b).then(|| format!("{path}: {a:?} != {b:?} (difference {:e})", a - b))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} != {}", a.len(), b.len()));
            }
            a.iter().zip(b).enumerate().find_map(|(i, (a, b))| diff(a, b, &format!("{path}[{i}]")))
        }
        (Value::Object(a), Value::Object(b)) => {
            let keys_a: Vec<_> = a.keys().collect();
            let keys_b: Vec<_> = b.keys().collect();
            if keys_a != keys_b {
                return Some(format!("{path}: keys {keys_a:?} != {keys_b:?}"));
            }
            a.iter().find_map(|(k, v)| diff(v, &b[k], &format!("{path}.{k}")))
        }
        _ => (actual != expected).then(|| format!("{path}: {actual} != {expected}")),
    }
}

fn outcome(result: maxwell_core::Result<Value>) -> Value {
    match result {
        Ok(value) => serde_json::json!({"result": value}),
        Err(error) => serde_json::json!({"error": error.to_string()}),
    }
}

fn expected_outcome(case: &Value) -> Value {
    match case.get("result") {
        Some(result) => serde_json::json!({"result": result}),
        None => serde_json::json!({"error": case["error"]}),
    }
}

#[test]
fn format_g_matches_python() {
    for pair in fixture("format_g.json").as_array().unwrap() {
        let value = pair[0].as_f64().unwrap();
        assert_eq!(fmt::g(value), pair[1].as_str().unwrap(), "format({value:?}, 'g')");
    }
}

#[test]
fn validation_errors_match_python() {
    for case in fixture("validate.json").as_array().unwrap() {
        let actual = outcome(compile::validate(&case["request"]).map(|(filters, preamp)| {
            serde_json::json!({"filters": filters.iter().map(compile::filter_json).collect::<Vec<_>>(), "preamp": preamp})
        }));
        if let Some(difference) = diff(&actual, &expected_outcome(case), "") {
            panic!("validate {}: {difference}", case["request"]);
        }
    }
}

#[test]
fn apo_parse_matches_python() {
    let fixture = fixture("apo.json");
    for case in fixture["parse"].as_array().unwrap() {
        let actual = outcome(apo::parse(&case["text"]));
        if let Some(difference) = diff(&actual, &expected_outcome(case), "") {
            panic!("parse {:?}: {difference}", case["text"]);
        }
    }
}

#[test]
fn apo_export_matches_python() {
    let fixture = fixture("apo.json");
    for case in fixture["export"].as_array().unwrap() {
        let text = apo::export(&case["preset"], &case["preamp_db"]).unwrap();
        assert_eq!(text, case["text"].as_str().unwrap(), "export {}", case["preset"]);
    }
}

#[test]
fn compile_matches_python() {
    if let Err(error) = vendor::verified_dll() {
        eprintln!("skipping compile parity: {error}");
        return;
    }
    let fixture = fixture("compile.json");
    assert_eq!(fixture["dll_sha256"], vendor::DLL_SHA256);
    let mut failures = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let mut actual = outcome(compile::compile_with(&case["request"], compile::Encoder::Auto));
        // The fixture keeps every 16th curve point plus the count.
        if let Some(curves) = actual.pointer_mut("/result/curves").and_then(Value::as_array_mut) {
            for curve in curves {
                let points = curve["points"].as_array().unwrap().clone();
                curve["points"] = Value::Array(points.iter().step_by(16).cloned().collect());
                curve["point_count"] = points.len().into();
            }
        }
        if let Some(difference) = diff(&actual, &expected_outcome(case), "") {
            failures.push(format!("{}: {difference}", case["name"].as_str().unwrap()));
        }
    }
    assert!(failures.is_empty(), "{} compile cases differ:\n{}", failures.len(), failures.join("\n"));
}
