//! RBJ biquads, frequency response, and the headset's coefficient layout.
//!
//! Arithmetic follows CPython's evaluation order (left to right, Smith complex
//! division, `hypot` for magnitude) so curves match the Python reference.
//! Shelves use the W3C Audio EQ Cookbook's Q form; Q = 1/sqrt(2) is the
//! monotonic S=1 slope: <https://www.w3.org/TR/audio-eq-cookbook/>

use crate::{Error, Result};
use std::f64::consts::PI;

/// The default shelf and pass Q: S=1 shelves and Butterworth pass filters.
pub const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
pub const MAX_BANDS: usize = 10;
pub const WORDS: usize = 103;
/// Rate ID and sample rate for each coefficient bank, in blob order.
pub const RATES: [(u16, u32); 4] = [(1, 44100), (2, 48000), (5, 88200), (6, 96000)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Peak, Kind::LowShelf, Kind::HighShelf, Kind::LowPass, Kind::HighPass];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Peak => "peak",
            Kind::LowShelf => "low_shelf",
            Kind::HighShelf => "high_shelf",
            Kind::LowPass => "low_pass",
            Kind::HighPass => "high_pass",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub fn is_pass(self) -> bool {
        matches!(self, Kind::LowPass | Kind::HighPass)
    }

    pub fn is_shelf(self) -> bool {
        matches!(self, Kind::LowShelf | Kind::HighShelf)
    }
}

/// One validated filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    pub kind: Kind,
    pub frequency: f64,
    pub gain: f64,
    pub q: f64,
    pub enabled: bool,
}

impl Filter {
    pub fn peak(frequency: f64, gain: f64, q: f64) -> Filter {
        Filter { kind: Kind::Peak, frequency, gain, q, enabled: true }
    }
}

/// b0, b1, b2, a1, a2 with a0 = 1.
pub type Stage = [f64; 5];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const fn new(re: f64, im: f64) -> Complex {
        Complex { re, im }
    }

    /// `float + complex`.
    fn plus(self, real: f64) -> Complex {
        Complex::new(real + self.re, self.im)
    }

    /// `float * complex`.
    fn scaled(self, real: f64) -> Complex {
        Complex::new(real * self.re, real * self.im)
    }

    fn add(self, other: Complex) -> Complex {
        Complex::new(self.re + other.re, self.im + other.im)
    }

    fn mul(self, b: Complex) -> Complex {
        Complex::new(self.re * b.re - self.im * b.im, self.re * b.im + self.im * b.re)
    }

    /// CPython's `_Py_c_quot` (Smith's algorithm).
    fn div(self, b: Complex) -> Complex {
        let (a, abs_re, abs_im) = (self, b.re.abs(), b.im.abs());
        if abs_re >= abs_im {
            if abs_re == 0.0 {
                return Complex::new(f64::NAN, f64::NAN);
            }
            let ratio = b.im / b.re;
            let denom = b.re + b.im * ratio;
            Complex::new((a.re + a.im * ratio) / denom, (a.im - a.re * ratio) / denom)
        } else if abs_im >= abs_re {
            let ratio = b.re / b.im;
            let denom = b.re * ratio + b.im;
            Complex::new((a.re * ratio + a.im) / denom, (a.im * ratio - a.re) / denom)
        } else {
            Complex::new(f64::NAN, f64::NAN)
        }
    }

    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
}

/// Magnitude in dB of `scale` times the cascade at `frequency`.
pub fn response(stages: &[Stage], scale: f64, frequency: f64, rate: f64) -> f64 {
    let theta = -2.0 * PI * frequency / rate;
    let z = Complex::new(theta.cos(), theta.sin());
    let mut h = Complex::new(scale, 0.0);
    for &[b0, b1, b2, a1, a2] in stages {
        let numerator = z.scaled(b1).plus(b0).add(z.scaled(b2).mul(z));
        let denominator = z.scaled(a1).plus(1.0).add(z.scaled(a2).mul(z));
        h = h.mul(numerator.div(denominator));
    }
    20.0 * h.abs().max(1e-15).log10()
}

/// The vendor's peak biquad (`peq.biquad`): width is frequency / Q.
pub fn biquad(frequency: f64, gain: f64, q: f64, rate: f64) -> Stage {
    let a = 10f64.powf(gain / 40.0);
    let omega = 2.0 * PI * frequency / rate;
    let alpha = omega.sin() / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    [
        (1.0 + alpha * a) / a0,
        -2.0 * omega.cos() / a0,
        (1.0 - alpha * a) / a0,
        -2.0 * omega.cos() / a0,
        (1.0 - alpha / a) / a0,
    ]
}

/// Upper bound on one filter's gain from DC through Nyquist.
pub fn filter_headroom_db(f: &Filter) -> f64 {
    if !f.enabled {
        return 0.0;
    }
    match f.kind {
        Kind::Peak => f.gain.max(0.0),
        // Above Q = 1/sqrt(2) a shelf overshoots on both sides of its midpoint.
        Kind::LowShelf | Kind::HighShelf => shelf_turning_ratios(f)
            .into_iter()
            .flatten()
            .map(|ratio| shelf_db(f, ratio))
            .fold(f.gain.max(0.0), f64::max),
        // BLT maps the analog prototype's whole axis onto DC..Nyquist. A resonant
        // second-order pass filter peaks at Q / sqrt(1 - 1/(4Q^2)).
        Kind::LowPass | Kind::HighPass if f.q > SHELF_Q => {
            20.0 * (f.q / (1.0 - 1.0 / (4.0 * f.q * f.q)).sqrt()).log10()
        }
        Kind::LowPass | Kind::HighPass => 0.0,
    }
}

/// The analog frequencies, as ratios to the midpoint, where a shelf with
/// Q > 1/sqrt(2) peaks and dips. The two are reciprocals.
///
/// With x = (w/w0)^2, the low shelf's |H|^2 is A^2 ((A - x)^2 + kx) / ((1 - Ax)^2 + kx)
/// with k = A/Q^2. Its turning points solve c x^2 + 2(1 + A^2) x + c = 0 with
/// c = k - 2A, which has positive roots only when c < 0. A high shelf is the
/// same response mirrored about w0, so it turns at the same pair.
pub fn shelf_turning_ratios(f: &Filter) -> Option<[f64; 2]> {
    if !f.kind.is_shelf() || f.gain == 0.0 || f.q <= SHELF_Q {
        return None;
    }
    let a = 10f64.powf(f.gain / 40.0);
    let c = a / (f.q * f.q) - 2.0 * a;
    let half = 1.0 + a * a;
    let x = (-half + (half * half - c * c).max(0.0).sqrt()) / c;
    Some([x.sqrt(), 1.0 / x.sqrt()])
}

/// A low shelf's analog gain in dB at `ratio` times its midpoint.
fn shelf_db(f: &Filter, ratio: f64) -> f64 {
    let a = 10f64.powf(f.gain / 40.0);
    let (k, x) = (a / (f.q * f.q), ratio * ratio);
    10.0 * (a * a * ((a - x).powi(2) + k * x) / ((1.0 - a * x).powi(2) + k * x)).log10()
}

/// a0-normalized b0, b1, b2, a1, a2 for one validated filter.
pub fn rbj_biquad(f: &Filter, rate: f64) -> Stage {
    if !f.enabled {
        return [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    let omega = 2.0 * PI * f.frequency / rate;
    let (cosine, sine) = (omega.cos(), omega.sin());
    let alpha = sine / (2.0 * f.q);
    let (b0, b1, b2, a0, a1, a2);
    match f.kind {
        Kind::Peak => {
            let a = 10f64.powf(f.gain / 40.0);
            (b0, b1, b2) = (1.0 + alpha * a, -2.0 * cosine, 1.0 - alpha * a);
            (a0, a1, a2) = (1.0 + alpha / a, -2.0 * cosine, 1.0 - alpha / a);
        }
        Kind::LowPass => {
            b0 = (1.0 - cosine) / 2.0;
            b2 = b0;
            b1 = 1.0 - cosine;
            (a0, a1, a2) = (1.0 + alpha, -2.0 * cosine, 1.0 - alpha);
        }
        Kind::HighPass => {
            b0 = (1.0 + cosine) / 2.0;
            b2 = b0;
            b1 = -(1.0 + cosine);
            (a0, a1, a2) = (1.0 + alpha, -2.0 * cosine, 1.0 - alpha);
        }
        Kind::LowShelf => {
            let a = 10f64.powf(f.gain / 40.0);
            let beta = 2.0 * a.sqrt() * alpha;
            b0 = a * ((a + 1.0) - (a - 1.0) * cosine + beta);
            b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cosine);
            b2 = a * ((a + 1.0) - (a - 1.0) * cosine - beta);
            a0 = (a + 1.0) + (a - 1.0) * cosine + beta;
            a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cosine);
            a2 = (a + 1.0) + (a - 1.0) * cosine - beta;
        }
        Kind::HighShelf => {
            let a = 10f64.powf(f.gain / 40.0);
            let beta = 2.0 * a.sqrt() * alpha;
            b0 = a * ((a + 1.0) + (a - 1.0) * cosine + beta);
            b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cosine);
            b2 = a * ((a + 1.0) + (a - 1.0) * cosine - beta);
            a0 = (a + 1.0) - (a - 1.0) * cosine + beta;
            a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cosine);
            a2 = (a + 1.0) - (a - 1.0) * cosine - beta;
        }
    }
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Decode 103 words into ten stages and the global gain, rejecting unstable poles.
pub fn decode_words(words: &[u16]) -> Result<(Vec<Stage>, f64)> {
    if words.len() != WORDS || usize::from(words[0]) != MAX_BANDS {
        return Err(Error::value("Unexpected vendor coefficient layout; refusing this payload."));
    }
    let fixed = |offset: usize, bits: i32| {
        let value = (u32::from(words[offset]) << 16 | u32::from(words[offset + 1])) as i32;
        f64::from(value) / 2f64.powi(bits)
    };
    let mut stages = Vec::with_capacity(MAX_BANDS);
    for band in 0..MAX_BANDS {
        let mut stage = [0.0; 5];
        for (j, value) in stage.iter_mut().enumerate() {
            *value = fixed(1 + 10 * band + 2 * j, if j == 1 || j == 3 { 30 } else { 31 });
        }
        let [_, _, _, a1, a2] = stage;
        // Roots of z^2 + a1 z + a2, as cmath.sqrt would give them.
        let discriminant = a1 * a1 - 4.0 * a2;
        let largest = if discriminant >= 0.0 {
            let root = discriminant.sqrt();
            ((-a1 + root) / 2.0).abs().max(((-a1 - root) / 2.0).abs())
        } else {
            (-a1 / 2.0).hypot((-discriminant).sqrt() / 2.0)
        };
        if largest.is_nan() || largest >= 1.0 {
            return Err(Error::value("Quantized filter is unstable; choose a lower Q or higher frequency."));
        }
        stages.push(stage);
    }
    let scale = fixed(101, 26);
    if scale <= 0.0 || !scale.is_finite() {
        return Err(Error::value("Invalid coefficient gain scale."));
    }
    Ok((stages, scale))
}
