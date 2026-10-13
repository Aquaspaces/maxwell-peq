//! Python's float formatting, so messages and exported text match the
//! reference implementation character for character.

use serde_json::Value;

/// `format(x, 'g')`: six significant digits, trailing zeros removed.
pub fn g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    // Rust and CPython both round exactly, so the exponent after rounding agrees.
    let scientific = format!("{x:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("exponent");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    if (-4..6).contains(&exponent) {
        strip_zeros(&format!("{x:.*}", (5 - exponent) as usize)).to_string()
    } else {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip_zeros(mantissa), exponent.abs())
    }
}

/// `float(f'{x:.{places}f}')`.
pub fn round(x: f64, places: usize) -> f64 {
    format!("{x:.places$}").parse().unwrap_or(x)
}

fn strip_zeros(text: &str) -> &str {
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { text }
}

/// Python's `str()` of a JSON value, for messages that echo user input.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(text) => text.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(x) if !n.is_i64() && !n.is_u64() => py_repr_float(x),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// `repr(float)`: shortest round-trip digits, always with a decimal point.
pub fn py_repr_float(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else if x.is_finite() && (x.abs() >= 1e16 || (x != 0.0 && x.abs() < 1e-4)) {
        let text = format!("{x:e}");
        let (mantissa, exponent) = text.split_once('e').expect("exponent");
        let exponent: i32 = exponent.parse().expect("integer exponent");
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        x.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_values() {
        assert_eq!(g(-12.0), "-12");
        assert_eq!(g(0.1), "0.1");
        assert_eq!(g(20000.0), "20000");
        assert_eq!(g(-6.020599913279624), "-6.0206");
        assert_eq!(g(1234567.0), "1.23457e+06");
        assert_eq!(g(0.00001234), "1.234e-05");
        assert_eq!(round(1.23456, 3), 1.235);
        assert_eq!(py_repr_float(24000.0), "24000.0");
        assert_eq!(py_repr_float(0.5), "0.5");
    }
}
