//! The built-in encoder as a stand-in for the Audeze DLL: it must build every
//! peak set the DLL builds, with the same response.

use maxwell_core::compile::{Encoder, compile_with};
use maxwell_core::vendor;
use serde_json::{Value, json};

/// A small deterministic generator, so failures reproduce.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next()
    }
}

/// One to ten peaks, rounded the way the editor rounds them.
fn random_peaks(random: &mut Lcg) -> Value {
    let count = 1 + (random.next() * 10.0) as usize;
    let filters: Vec<Value> = (0..count)
        .map(|_| {
            let frequency = (20f64 * 1000f64.powf(random.next())).round();
            let gain = (random.range(-12.0, 12.0) * 10.0).round() / 10.0;
            let q = (2f64.powf(random.range(-2.0, 3.3)) * 100.0).round() / 100.0;
            json!({"type": "peak", "frequency": frequency, "gain": gain, "q": q})
        })
        .collect();
    json!({"filters": filters, "preamp": "auto"})
}

fn curves(result: &Value) -> Vec<(f64, f64)> {
    result["curves"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|curve| curve["points"].as_array().unwrap().iter())
        .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
        .collect()
}

#[test]
fn built_in_encoder_builds_peak_sets() {
    for preset in [
        json!({"filters": [{"frequency": 1000, "gain": -12, "q": 8}]}),
        json!({"filters": [{"frequency": 60, "gain": 6, "q": 0.7}, {"frequency": 8000, "gain": -3, "q": 2}]}),
        json!({"filters": [{"frequency": 3000, "gain": 4, "q": 1.4}], "preamp": -10}),
        // Ten narrow cuts the DLL rejects for precision.
        json!({"filters": ([40, 80, 150, 300, 600, 1200, 2400, 4800, 9600, 16000])
            .map(|f| json!({"frequency": f, "gain": -2, "q": 1.2}))
            .to_vec()}),
    ] {
        let result = compile_with(&preset, Encoder::BuiltIn).unwrap();
        assert_eq!(result["engine"], "extended-rbj");
        assert_eq!(result["experimental_filters"], false);
        assert_eq!(result["blob_hex"].as_str().unwrap().len(), 1688);
        assert!(result["max_error_db"].as_f64().unwrap() < 0.15);
    }
}

#[test]
fn built_in_encoder_matches_the_dll() {
    if let Err(error) = vendor::verified_dll() {
        eprintln!("skipping: {error}");
        return;
    }
    let mut random = Lcg(0x4d61_7877_656c_6c21);
    let (mut compared, mut worst, mut dll_only_failures) = (0, 0.0f64, 0);
    let mut built_in_failures = Vec::new();
    for _ in 0..400 {
        let preset = random_peaks(&mut random);
        let dll = compile_with(&preset, Encoder::Auto);
        let built_in = compile_with(&preset, Encoder::BuiltIn);
        match (dll, built_in) {
            (Ok(dll), Ok(built_in)) => {
                assert_eq!(dll["engine"], "vendor");
                assert_eq!(dll["preamp"], built_in["preamp"]);
                let (a, b) = (curves(&dll), curves(&built_in));
                assert_eq!(a.len(), b.len());
                for ((fa, da), (fb, db)) in a.iter().zip(&b) {
                    assert_eq!(fa, fb);
                    worst = worst.max((da - db).abs());
                }
                compared += 1;
            }
            (Ok(_), Err(error)) => built_in_failures.push(format!("{preset}: {error}")),
            (Err(_), Ok(_)) => dll_only_failures += 1,
            (Err(_), Err(_)) => {}
        }
    }
    eprintln!(
        "compared {compared} presets; worst response difference {worst:.4} dB; the built-in encoder alone built {dll_only_failures} more"
    );
    assert!(
        built_in_failures.is_empty(),
        "the DLL built these but the built-in encoder failed: {built_in_failures:#?}"
    );
    assert!(compared > 150);
    // Each curve is within 0.5 dB of the ideal response, so they can differ by up to 1.
    assert!(worst < 1.0, "worst response difference {worst} dB");
}
