//! Equalizer APO import/export and the profile library. Ports tests/test_apo_profiles.py.

use maxwell_core::apo;
use maxwell_core::dsp::SHELF_Q;
use maxwell_core::profiles::Profiles;
use serde_json::{Value, json};
use std::path::PathBuf;

const SAMPLE: &str = "# AutoEQ style preset
Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70
Filter 2: ON PK Fc 180 Hz Gain -2.4 dB Q 1.10
Filter 3: OFF PK Fc 6000 Hz Gain 1 dB Q 4
Filter 4: ON BP Fc 500 Hz Q 2
Filter 5: ON HPQ Fc 30 Hz Q 0.7
Device: Speakers
";

fn parse(text: &str) -> maxwell_core::Result<Value> {
    apo::parse(&json!(text))
}

#[test]
fn shelf_q_and_slope_round_trip() {
    let text = "Filter 1: ON LSC Fc 105 Hz Gain 5 dB Q 1.5
Filter 2: ON HSC Fc 8000 Hz Gain -3 dB Q 0.7071
                Filter 3: ON HSC 6 dB Fc 4000 Hz Gain 2 dB
Filter 4: ON LSC 72 dB Fc 100 Hz Gain 12 dB
";
    let result = parse(text).unwrap();
    let qs: Vec<_> = result["filters"].as_array().unwrap().iter().map(|f| f["q"].as_f64().unwrap()).collect();
    // 6 dB/octave is S = 0.5: 1/Q = sqrt((A + 1/A)(1/S - 1) + 2) with A = 10^(2/40).
    assert_eq!(qs, [1.5, SHELF_Q, 0.499, SHELF_Q]);
    assert_eq!(result["warnings"], json!(["Line 4: shelf slope 72 dB can't be used with its gain; using Q 0.707."]));
    let exported = apo::export(&result, &json!(-5)).unwrap();
    assert!(
        exported.contains(
            "Filter 1: ON LSC Fc 105 Hz Gain 5 dB Q 1.5
"
        ),
        "{exported}"
    );
    assert_eq!(parse(&exported).unwrap()["filters"], result["filters"]);
}

#[test]
fn parses_supported_filters_and_reports_the_rest() {
    let result = parse(SAMPLE).unwrap();
    let kinds: Vec<_> = result["filters"].as_array().unwrap().iter().map(|f| f["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["low_shelf", "peak", "peak", "high_pass"]);
    assert_eq!(result["filters"][0]["q"], 0.7);
    assert_eq!(result["filters"][2]["enabled"], false);
    assert_eq!(result["filters"][3]["gain"].as_f64(), Some(0.0));
    assert_eq!(result["preamp"], "auto");
    assert_eq!(result["source_preamp"], -6.2);
    let warnings: Vec<_> = result["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    let warnings = warnings.join(" ");
    for needle in ["band-pass", "Device", "Preamp -6.2"] {
        assert!(warnings.contains(needle), "{needle} not in {warnings}");
    }
}

#[test]
fn clamps_out_of_range_values() {
    let result = parse("Filter: ON PK Fc 25000 Hz Gain 20 dB Q 50").unwrap();
    let f = &result["filters"][0];
    assert_eq!((f["frequency"].as_f64(), f["gain"].as_f64(), f["q"].as_f64()), (Some(20000.0), Some(12.0), Some(20.0)));
    assert_eq!(result["warnings"].as_array().unwrap().len(), 3);
}

#[test]
fn bandwidth_converts_to_q() {
    let q = parse("Filter 1: ON PK Fc 1000 Hz Gain -3 dB BW Oct 1").unwrap()["filters"][0]["q"].as_f64().unwrap();
    assert!((q - 1.414).abs() < 0.0005, "{q}");
}

#[test]
fn keeps_first_ten_filters() {
    let text: Vec<String> =
        (1..13).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1", 100 + i * 50)).collect();
    let result = parse(&text.join("\n")).unwrap();
    assert_eq!(result["filters"].as_array().unwrap().len(), 10);
    assert!(result["warnings"].to_string().contains("first 10 of 12"));
}

#[test]
fn rejects_unusable_text() {
    for text in ["", "Preamp: -3 dB", "GraphicEQ: 20 0; 40 1"] {
        assert!(parse(text).is_err(), "{text:?}");
    }
    assert!(apo::parse(&json!(42)).is_err());
}

#[test]
fn export_round_trips() {
    let preset = json!({"preamp": "auto", "filters": [
        {"type": "low_shelf", "frequency": 105, "gain": 5.5, "q": SHELF_Q, "enabled": true},
        {"type": "peak", "frequency": 1450, "gain": 2.1, "q": 2, "enabled": false},
        {"type": "low_pass", "frequency": 16000, "gain": 0, "q": 0.7, "enabled": true}]});
    let text = apo::export(&preset, &json!(-7.6)).unwrap();
    assert!(text.starts_with("Preamp: -7.6 dB\n"), "{text}");
    assert!(text.contains("Filter 2: OFF PK Fc 1450 Hz Gain 2.1 dB Q 2"), "{text}");
    let again = parse(&text).unwrap();
    let summary = |filters: &Value| -> Vec<(String, f64, f64, bool)> {
        filters
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["type"].as_str().unwrap().to_owned(),
                    f["frequency"].as_f64().unwrap(),
                    f["gain"].as_f64().unwrap(),
                    f["enabled"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(summary(&again["filters"]), summary(&preset["filters"]));
}

struct Library {
    directory: PathBuf,
    path: PathBuf,
    preset: Value,
}

impl Library {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("maxwell-peq-test-{}", maxwell_core::util::token_hex8()));
        std::fs::create_dir_all(&directory).unwrap();
        let preset = json!({"filters": [{"type": "peak", "frequency": 1000, "gain": -3, "q": 1, "enabled": true}], "preamp": "auto"});
        Library { path: directory.join("profiles.json"), directory, preset }
    }

    fn open(&self) -> Profiles {
        Profiles::load(Some(self.path.clone())).unwrap()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn names(listing: &Value) -> Vec<String> {
    listing["profiles"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn starts_with_flat_and_persists_changes() {
    let library = Library::new();
    let mut profiles = library.open();
    assert_eq!(names(&profiles.listing()), ["Flat"]);
    let made = profiles
        .perform("/api/profiles/save", &json!({"name": "  Bass  ", "preset": library.preset, "activate": true}))
        .unwrap();
    assert_eq!(made["active"], made["saved"]);
    let reloaded = library.open().listing();
    assert_eq!(names(&reloaded), ["Flat", "Bass"]);
    assert_eq!(reloaded["active"], made["saved"]);
}

#[test]
fn rename_keeps_preset_and_delete_moves_active() {
    let library = Library::new();
    let mut profiles = library.open();
    let saved = profiles
        .perform("/api/profiles/save", &json!({"name": "Bass", "preset": library.preset, "activate": true}))
        .unwrap()["saved"]
        .clone();
    let renamed = profiles.perform("/api/profiles/save", &json!({"id": saved, "name": "Warm"})).unwrap();
    let profile = renamed["profiles"].as_array().unwrap().iter().find(|p| p["id"] == saved).unwrap();
    assert_eq!(profile["name"], "Warm");
    assert_eq!(profile["preset"]["filters"][0]["gain"].as_f64(), Some(-3.0));
    let result = profiles.perform("/api/profiles/delete", &json!({"id": saved})).unwrap();
    assert_eq!(result["active"], result["profiles"][0]["id"]);
    assert!(profiles.perform("/api/profiles/delete", &json!({"id": result["active"]})).is_err());
}

#[test]
fn rejects_bad_input() {
    let library = Library::new();
    let mut profiles = library.open();
    let mut loud = library.preset.clone();
    loud["preamp"] = json!("loud");
    for request in [
        json!({"name": "", "preset": library.preset}),
        json!({"name": "x".repeat(61), "preset": library.preset}),
        json!({"name": "Bad", "preset": {"filters": []}}),
        json!({"name": "Bad", "preset": {"filters": [{"type": "notch", "frequency": 1, "gain": 0, "q": 1}]}}),
        json!({"name": "Bad", "preset": loud}),
    ] {
        assert!(profiles.perform("/api/profiles/save", &request).is_err(), "{request}");
    }
}

#[test]
fn order_must_match_library() {
    let library = Library::new();
    let mut profiles = library.open();
    let second =
        profiles.perform("/api/profiles/save", &json!({"name": "B", "preset": library.preset})).unwrap()["saved"]
            .clone();
    let first = profiles.listing()["profiles"][0]["id"].clone();
    let ordered = profiles.perform("/api/profiles/order", &json!({"ids": [second, first]})).unwrap();
    let ids: Vec<_> = ordered["profiles"].as_array().unwrap().iter().map(|p| p["id"].clone()).collect();
    assert_eq!(ids, [second.clone(), first]);
    assert!(profiles.perform("/api/profiles/order", &json!({"ids": [second]})).is_err());
}

#[test]
fn drops_damaged_entries_on_load() {
    let library = Library::new();
    let data = json!({"active": "gone", "profiles": [
        {"id": "a", "name": "Good", "preset": library.preset},
        {"id": "b", "name": "Broken", "preset": {"filters": "x"}}]});
    std::fs::write(&library.path, data.to_string()).unwrap();
    let listing = library.open().listing();
    assert_eq!(names(&listing), ["Good"]);
    assert_eq!(listing["active"], "a");
}

#[test]
fn reads_a_python_written_library() {
    // server.py wrote json.dumps(indent=2) with ensure_ascii; keep reading those files.
    let library = Library::new();
    let text = "{\n  \"active\": \"a\",\n  \"profiles\": [\n    {\n      \"id\": \"a\",\n      \"name\": \"Caf\\u00e9\",\n      \"preset\": {\"filters\": [{\"type\": \"peak\", \"frequency\": 1000, \"gain\": -3, \"q\": 1, \"enabled\": true}], \"preamp\": \"auto\"}\n    }\n  ]\n}\n";
    std::fs::write(&library.path, text).unwrap();
    assert_eq!(names(&library.open().listing()), ["Café"]);
}
