//! Atomic JSON files, random tokens, and UTC timestamps.

use crate::Result;
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes
}

/// `secrets.token_hex(n)` for n = 8.
pub fn token_hex8() -> String {
    hex::encode(random_bytes::<8>())
}

/// `secrets.token_urlsafe(32)`: 43 characters of unpadded base64url.
pub fn token_urlsafe() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = random_bytes::<32>();
    let mut text = String::with_capacity(43);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            text.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    text
}

/// Compare secrets without an early exit on the first differing byte.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Write pretty JSON next to `path`, flush it to disk, then rename it into place.
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!("{name}.{}.tmp", token_hex8()));
    let result = (|| {
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        serde_json::to_writer_pretty(&mut output, value)?;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// UTC now as `%Y%m%dT%H%M%S.%fZ`, for audit file names.
pub fn utc_stamp() -> String {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    stamp(since.as_secs(), since.subsec_micros())
}

fn stamp(seconds: u64, micros: u32) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's days-to-civil conversion.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}.{micros:06}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_match_python() {
        assert_eq!(stamp(0, 0), "19700101T000000.000000Z");
        // datetime(2026, 2, 28, 23, 59, 59, 123456, timezone.utc)
        assert_eq!(stamp(1_772_323_199, 123_456), "20260228T235959.123456Z");
        assert_eq!(stamp(951_782_400, 5), "20000229T000000.000005Z");
    }

    #[test]
    fn tokens_have_python_lengths() {
        assert_eq!(token_urlsafe().len(), 43);
        assert!(token_urlsafe().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_eq!(token_hex8().len(), 16);
    }
}
