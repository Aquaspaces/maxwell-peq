//! The headset's 24-bit coefficient serialization, for filter sets the vendor
//! DLL cannot build (shelves) and in place of the DLL where it is unavailable.
//!
//! This encoder intentionally does not reproduce the vendor's stage sorting and
//! normalization. It uses the same numeric layout with conservative stage gain
//! bounds; callers must check decoded stability and response before live use.

use crate::dsp::{Filter, Kind, MAX_BANDS, filter_headroom_db, rbj_biquad};
use crate::{Error, Result};

const POSITIVE_24_MAX: i64 = (1 << 23) - 1;

/// Round on the signed 24-bit grid and place it in the high 24 of 32 bits.
///
/// Vendor code rounds with floor(x + .5), not bankers' rounding. Coefficients
/// use fractional bits 23/22; the global gain uses 18. The wire representation
/// is the resulting integer shifted left 8, high 16-bit word then low word.
fn packed_fixed(value: f64, bits: i32) -> Result<[u16; 2]> {
    if !value.is_finite() {
        return Err(Error::value("Nonfinite filter coefficient."));
    }
    let scaled = (value * f64::from(1u32 << bits) + 0.5).floor();
    if !(-(1i64 << 23) as f64..=POSITIVE_24_MAX as f64).contains(&scaled) {
        return Err(Error::value("Filter coefficient exceeds the signed 24-bit range."));
    }
    let packed = ((scaled as i64) << 8) as u32;
    Ok([(packed >> 16) as u16, (packed & 0xffff) as u16])
}

/// `value` rounded to the coefficient grid, as [`packed_fixed`] rounds it.
fn grid(value: f64, bits: i32) -> f64 {
    let scale = f64::from(1u32 << bits);
    (value * scale + 0.5).floor() / scale
}

/// Encode ten stages and a global gain as 103 uint16 words.
///
/// Normalize each numerator so its ideal frequency gain cannot exceed unity,
/// then compensate the accumulated scaling in the final gain. The first stage
/// also gets the vendor's 1.19 attenuation factor. Positive unity is not
/// exactly representable in Q23, so reserve one LSB before rounding.
pub fn encode_bank(filters: &[Filter], preamp: f64, rate: f64) -> Result<Vec<u16>> {
    encode(filters, preamp, rate, false)
}

/// Stand in for the vendor DLL on sets without shelves: like [`encode_bank`],
/// but peak stages are scaled and rounded the way the vendor's are.
pub fn substitute_bank(filters: &[Filter], preamp: f64, rate: f64) -> Result<Vec<u16>> {
    encode(filters, preamp, rate, true)
}

fn encode(filters: &[Filter], preamp: f64, rate: f64, vendor_peaks: bool) -> Result<Vec<u16>> {
    let active: Vec<&Filter> = filters.iter().filter(|f| f.enabled).collect();
    if active.len() > MAX_BANDS {
        return Err(Error::value("Use no more than 10 filters."));
    }
    let mut words = vec![MAX_BANDS as u16];
    let mut scale = 10f64.powf(preamp / 20.0);
    let positive_limit = POSITIVE_24_MAX as f64 / f64::from(1u32 << 23);
    for index in 0..MAX_BANDS {
        let peak = vendor_peaks && active.get(index).is_some_and(|f| f.kind == Kind::Peak);
        let ([b0, b1, b2, a1, a2], bound) = match active.get(index) {
            // Peaks follow the vendor: b0 becomes unity and the gain moves to the
            // final scale. A response bound would cost up to 2 bits at +12 dB.
            Some(f) if peak => (rbj_biquad(f, rate), 0.0),
            Some(f) => (rbj_biquad(f, rate), 10f64.powf(filter_headroom_db(f) / 20.0)),
            None => ([1.0, 0.0, 0.0, 0.0, 0.0], 1.0),
        };
        let mut normalization = bound.max(b0.abs()).max(b1.abs() / 2.0).max(b2.abs()) / positive_limit;
        if index == 0 {
            normalization *= 1.19;
        }
        let mut stage = [b0 / normalization, b1 / normalization, b2 / normalization, a1, a2];
        // Those peaks also keep their DC gain exact on the coefficient grid, as
        // the vendor's do. Low, narrow peaks put poles and zeros next to z = 1,
        // where one LSB is a large relative error.
        if peak {
            let dc = (stage[0] + stage[1] + stage[2]) / (1.0 + a1 + a2);
            stage[2] = dc * (1.0 + grid(a1, 22) + grid(a2, 23)) - grid(stage[0], 23) - grid(stage[1], 22);
        }
        for (value, bits) in stage.into_iter().zip([23, 22, 23, 22, 23]) {
            words.extend(packed_fixed(value, bits)?);
        }
        scale *= normalization;
    }
    // Q18 global gain has a practical positive range below 32, encoded as Q26.
    if scale <= 0.0 {
        return Err(Error::value("Invalid global filter gain."));
    }
    words.extend(packed_fixed(scale, 18)?);
    if words[words.len() - 2..] == [0, 0] {
        return Err(Error::value("Preamp is below the global gain resolution."));
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::packed_fixed;

    #[test]
    fn numeric_grid_matches_vendor_representation() {
        assert_eq!(packed_fixed(0.5, 23).unwrap(), [0x4000, 0]);
        assert_eq!(packed_fixed(-1.0, 23).unwrap(), [0x8000, 0]);
        assert_eq!(packed_fixed(1.0, 18).unwrap(), [0x0400, 0]);
        // Half-way cases round toward positive infinity, as floor(x + .5).
        assert_eq!(packed_fixed(-0.5 / f64::from(1u32 << 23), 23).unwrap(), [0, 0]);
        assert!(packed_fixed(1.0, 23).is_err());
        assert!(packed_fixed(32.0, 18).is_err());
        assert!(packed_fixed(f64::NAN, 23).is_err());
    }
}
