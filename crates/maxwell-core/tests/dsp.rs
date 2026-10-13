//! Filter math and compilation properties. Ports tests/test_peq.py,
//! test_extended_filters.py and test_filter_compilation.py; exact outputs are
//! pinned by golden.rs.

use maxwell_core::compile::{compile_preset, validate};
use maxwell_core::dsp::{Filter, Kind, RATES, SHELF_Q, biquad, decode_words, filter_headroom_db, rbj_biquad, response};
use maxwell_core::encoder::encode_bank;
use maxwell_core::vendor;
use serde_json::{Value, json};

fn preset(frequency: f64, gain: f64, q: f64) -> Value {
    json!({"filters": [{"frequency": frequency, "gain": gain, "q": q}], "preamp": "auto"})
}

fn row(kind: Kind, frequency: f64, gain: f64, q: f64) -> Filter {
    Filter { kind, frequency, gain, q, enabled: true }
}

fn row_json(kind: &str, frequency: f64, gain: f64, q: f64) -> Value {
    json!({"type": kind, "frequency": frequency, "gain": gain, "q": q})
}

fn close(a: f64, b: f64, places: i32) -> bool {
    (a - b).abs() < 0.5 * 10f64.powi(-places)
}

/// Compile tests need the inspected vendor DLL; skip where it is not installed.
fn vendor_available() -> bool {
    match vendor::verified_dll() {
        Ok(_) => true,
        Err(error) => {
            eprintln!("skipping: {error}");
            false
        }
    }
}

#[test]
fn invalid_values() {
    let mut cases = vec![
        preset(1000.0, -3.0, 0.0),
        preset(24000.0, -3.0, 1.0),
        preset(1000.0, 13.0, 1.0),
        json!({"filters": [{"frequency": 1000, "gain": 6, "q": 1}], "preamp": 0}),
        json!({"filters": [{"frequency": 1000, "gain": -3, "q": true}]}),
        json!({"filters": []}),
        json!({"filters": [{"type": "unknown", "frequency": 1000, "gain": 0, "q": 1}]}),
        json!({"filters": [row_json("low_pass", 1000.0, 3.0, SHELF_Q)]}),
        json!({"filters": [row_json("high_shelf", 1000.0, 0.0, 4.0)]}),
    ];
    // NaN cannot be written in JSON; Python rejected float('nan') in validation.
    cases.push(json!({"filters": [{"frequency": "nan", "gain": -3, "q": 1}]}));
    for request in cases {
        assert!(validate(&request).is_err(), "{request}");
    }
}

#[test]
fn headroom_bound() {
    let request = json!({"filters": [{"frequency": 1000, "gain": 6, "q": 1}, {"frequency": 3000, "gain": 4, "q": 2},
                                     {"frequency": 500, "gain": -3, "q": 1}]});
    assert_eq!(validate(&request).unwrap().1, -10.0);
}

#[test]
fn legacy_peak_and_shelf_defaults() {
    let (filters, _) = validate(&json!({"filters": [row_json("PK", 1000.0, 0.0, SHELF_Q)]})).unwrap();
    assert_eq!(filters[0].kind, Kind::Peak);
    let (filters, _) = validate(&json!({"filters": [{"type": "low_shelf", "frequency": 100, "gain": -3}]})).unwrap();
    assert_eq!(filters[0].q, SHELF_Q);
}

#[test]
fn resonant_pass_filters_require_headroom() {
    let f = row_json("high_pass", 1000.0, 0.0, 4.0);
    let (filters, preamp) = validate(&json!({"filters": [f]})).unwrap();
    assert!(close(preamp, -filter_headroom_db(&filters[0]), 7));
    assert!(preamp < -12.0);
    assert!(validate(&json!({"filters": [f], "preamp": 0})).is_err());
}

#[test]
fn independent_peak_properties() {
    for (_, rate) in RATES {
        let rate = f64::from(rate);
        for q in [0.5, 1.0, 4.0] {
            let (a, b) = (biquad(1000.0, 6.0, q, rate), biquad(1000.0, -6.0, q, rate));
            assert!(close(response(&[a], 1.0, 1000.0, rate), 6.0, 8));
            for f in [20.0, 200.0, 1000.0, 2000.0, 20000.0] {
                assert!(close(response(&[a, b], 1.0, f, rate), 0.0, 8));
            }
        }
    }
}

#[test]
fn shelf_endpoints_midpoint_and_monotonicity() {
    for kind in [Kind::LowShelf, Kind::HighShelf] {
        for gain in [-12.0, -6.0, 0.0, 6.0, 12.0] {
            for (_, rate) in RATES {
                let rate = f64::from(rate);
                let stage = rbj_biquad(&row(kind, 1000.0, gain, SHELF_Q), rate);
                let low = if kind == Kind::LowShelf { gain } else { 0.0 };
                let high = if kind == Kind::HighShelf { gain } else { 0.0 };
                assert!(close(response(&[stage], 1.0, 0.0, rate), low, 8));
                assert!(close(response(&[stage], 1.0, rate / 2.0, rate), high, 8));
                assert!(close(response(&[stage], 1.0, 1000.0, rate), gain / 2.0, 8));
                for i in 0..=200 {
                    let y = response(&[stage], 1.0, rate * f64::from(i) / 400.0, rate);
                    assert!(gain.min(0.0) - 1e-8 <= y && y <= gain.max(0.0) + 1e-8);
                }
            }
        }
    }
}

#[test]
fn pass_cutoff_resonance_and_peak_compatibility() {
    for (_, rate) in RATES {
        let rate = f64::from(rate);
        for kind in [Kind::LowPass, Kind::HighPass] {
            for q in [0.1, SHELF_Q, 1.0, 4.0, 20.0] {
                let f = row(kind, 1000.0, 0.0, q);
                let stage = rbj_biquad(&f, rate);
                assert!(close(response(&[stage], 1.0, 1000.0, rate), 20.0 * q.log10(), 8));
                if q > SHELF_Q {
                    let mut u = (1.0 - 1.0 / (2.0 * q * q)).sqrt();
                    if kind == Kind::HighPass {
                        u = 1.0 / u;
                    }
                    let peak = rate / std::f64::consts::PI * (u * (std::f64::consts::PI * 1000.0 / rate).tan()).atan();
                    assert!(close(response(&[stage], 1.0, peak, rate), filter_headroom_db(&f), 8));
                }
            }
        }
        let rbj = rbj_biquad(&row(Kind::Peak, 1000.0, 6.0, 4.0), rate);
        for (a, b) in rbj.iter().zip(biquad(1000.0, 6.0, 4.0, rate)) {
            assert!(close(*a, b, 14));
        }
    }
}

#[test]
fn mixed_cascade_quantization_and_stage_headroom() {
    let filters = [
        row(Kind::LowShelf, 120.0, 6.0, SHELF_Q),
        row(Kind::Peak, 1500.0, -5.0, 3.0),
        row(Kind::HighShelf, 6000.0, -4.0, SHELF_Q),
        row(Kind::LowPass, 16000.0, 0.0, SHELF_Q),
        row(Kind::HighPass, 45.0, 0.0, SHELF_Q),
    ];
    let preamp = -filters.iter().map(filter_headroom_db).sum::<f64>();
    for (_, rate) in RATES {
        let rate = f64::from(rate);
        let words = encode_bank(&filters, preamp, rate).unwrap();
        let (stages, scale) = decode_words(&words).unwrap();
        assert_eq!(words.len(), 103);
        assert!((2..103).step_by(2).all(|i| words[i] & 255 == 0));
        let ideal: Vec<_> = filters.iter().map(|f| rbj_biquad(f, rate)).collect();
        for i in 0..768 {
            let frequency = 20.0 * 1000f64.powf(f64::from(i) / 767.0);
            let expected = response(&ideal, 10f64.powf(preamp / 20.0), frequency, rate);
            let actual = response(&stages, scale, frequency, rate);
            assert!((actual - expected).abs() < 0.15);
            for stage in &stages {
                assert!(response(&[*stage], 1.0, frequency, rate) <= 0.05);
            }
        }
    }
}

#[test]
fn peak_payload_unchanged_from_listening_tested_build() {
    if !vendor_available() {
        return;
    }
    let result = compile_preset(&preset(1000.0, -3.0, 1.0)).unwrap();
    assert_eq!(result["sha256"], "2d02f080ad302fc731623d0e4ce430d47cb2daa88dc2b1ce9d7ef33900a5f635");
    assert_eq!(result["experimental_filters"], false);
}

#[test]
fn quantized_response_and_multirate_packet() {
    if !vendor_available() {
        return;
    }
    for (frequency, gain, q) in
        [(1000.0, -3.0, 1.0), (80.0, -6.0, 0.7), (8000.0, -8.0, 4.0), (1200.0, 6.0, 2.0), (20000.0, -3.0, 2.0)]
    {
        let result = compile_preset(&preset(frequency, gain, q)).unwrap();
        let raw = hex::decode(result["blob_hex"].as_str().unwrap()).unwrap();
        assert_eq!(raw.len(), 844);
        assert_eq!(raw[..4], 4u32.to_le_bytes());
        let word = |offset: usize| u16::from_le_bytes([raw[offset], raw[offset + 1]]);
        let mut offset = 4;
        for bank in result["banks"].as_array().unwrap() {
            assert_eq!((word(offset), word(offset + 2)), (bank["id"].as_u64().unwrap() as u16, 103));
            let words: Vec<u16> = (0..103).map(|i| word(offset + 4 + 2 * i)).collect();
            assert_eq!(json!(words), bank["words"]);
            offset += 210;
            let (stages, scale) = decode_words(&words).unwrap();
            let rate = bank["rate"].as_f64().unwrap();
            let center = response(&stages, scale, frequency, rate);
            assert!((center - (gain + result["preamp"].as_f64().unwrap())).abs() < 0.03);
        }
        assert!(result["max_error_db"].as_f64().unwrap() < 0.15);
    }
}

#[test]
fn flat_disabled_and_compiler_reuse() {
    if !vendor_available() {
        return;
    }
    let flat = compile_preset(&preset(1000.0, 0.0, 1.0)).unwrap();
    let worst = flat["curves"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|curve| curve["points"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap().abs()))
        .fold(0.0, f64::max);
    assert!(worst < 0.02);
    let mut disabled = preset(1000.0, 12.0, 1.0);
    disabled["filters"][0]["enabled"] = json!(false);
    assert_eq!(compile_preset(&disabled).unwrap()["blob_hex"], flat["blob_hex"]);

    let first = compile_preset(&preset(1000.0, -3.0, 1.0)).unwrap();
    let ten: Vec<Value> = [100, 180, 300, 500, 800, 1500, 2500, 4500, 9000, 16000]
        .iter()
        .map(|f| json!({"frequency": f, "gain": -2, "q": 1.2}))
        .collect();
    compile_preset(&json!({"filters": ten})).unwrap();
    assert_eq!(compile_preset(&preset(1000.0, -3.0, 1.0)).unwrap()["blob_hex"], first["blob_hex"]);
}

#[test]
fn all_filter_types_compile() {
    if !vendor_available() {
        return;
    }
    for f in [
        row_json("low_pass", 4000.0, 0.0, SHELF_Q),
        row_json("high_pass", 100.0, 0.0, SHELF_Q),
        row_json("low_shelf", 100.0, 6.0, SHELF_Q),
        row_json("high_shelf", 4000.0, -6.0, SHELF_Q),
        row_json("low_pass", 1000.0, 0.0, 4.0),
        row_json("high_pass", 1000.0, 0.0, 4.0),
    ] {
        let result = compile_preset(&json!({"filters": [f]})).unwrap();
        assert_eq!(result["blob_hex"].as_str().unwrap().len(), 1688);
        assert_eq!(result["experimental_filters"], true);
        assert!(result["max_error_db"].as_f64().unwrap() < 0.15);
        let shelf = f["type"].as_str().unwrap().contains("shelf");
        assert_eq!(result["engine"], if shelf { "extended-rbj" } else { "vendor" });
    }
}

#[test]
fn precision_guards() {
    if !vendor_available() {
        return;
    }
    let ten: Vec<Value> = [40, 80, 150, 300, 600, 1200, 2400, 4800, 9600, 16000]
        .iter()
        .map(|f| json!({"frequency": f, "gain": -2, "q": 1.2}))
        .collect();
    for request in [json!({"filters": ten}), json!({"filters": [row_json("low_shelf", 30.0, -6.0, SHELF_Q)]})] {
        let error = compile_preset(&request).unwrap_err().to_string();
        assert!(error.contains("precision error"), "{error}");
    }
}
