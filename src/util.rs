//! Small helpers: time, ids, text cleanup. Kept dependency-free on purpose.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Formats epoch milliseconds as an ISO-8601 UTC timestamp, e.g. `2026-10-27T14:03:09Z`.
pub fn iso_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// Compact human duration: `45s`, `3m04s`, `2h05m`, `3d04h`.
pub fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        3600..86_400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d{:02}h", s / 86_400, (s % 86_400) / 3600),
    }
}

/// A short random job id such as `k3f9a2`. Collisions are checked by the caller.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos
        ^ ((std::process::id() as u64) << 32)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // splitmix64 finaliser
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (0..6)
        .map(|i| ALPHABET[((x >> (i * 5)) & 31) as usize] as char)
        .collect()
}

/// 128 random bits as hex, for the web page's access token. std's hash keys
/// are seeded from the operating system's random source.
pub fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(now_ms());
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

/// Turns one raw output line into something worth showing as "last line":
/// keeps the text after the last carriage return (progress bars), strips
/// terminal colour codes, trims, and caps the length.
pub fn clean_line(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let segment = text
        .rsplit('\r')
        .map(strip_ansi)
        .find(|s| !s.trim().is_empty())?;
    let trimmed = segment.trim();
    Some(truncate(trimmed, 200))
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if !c.is_control() || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: ESC [ ... final byte in @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL or ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Truncates to at most `max` characters, adding an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Returns the last `n` lines of a file without reading all of it.
pub fn last_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut start = len;
    let mut buf: Vec<u8> = Vec::new();
    let chunk = 64 * 1024u64;
    loop {
        let new_start = start.saturating_sub(chunk);
        let mut piece = vec![0u8; (start - new_start) as usize];
        file.seek(SeekFrom::Start(new_start))?;
        file.read_exact(&mut piece)?;
        piece.extend_from_slice(&buf);
        buf = piece;
        start = new_start;
        let newlines = buf.iter().filter(|&&b| b == b'\n').count();
        if start == 0 || newlines > n {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(n);
    Ok(lines[skip..].iter().map(|l| l.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_formats_known_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_793_102_589_000), "2026-10-27T12:03:09Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(4_000), "4s");
        assert_eq!(fmt_duration(184_000), "3m04s");
        assert_eq!(fmt_duration(7_500_000), "2h05m");
        assert_eq!(fmt_duration(273_600_000), "3d04h");
    }

    #[test]
    fn clean_line_handles_progress_and_colour() {
        assert_eq!(
            clean_line(b"\x1b[32mok\x1b[0m test passed  ").as_deref(),
            Some("ok test passed")
        );
        assert_eq!(
            clean_line(b"10%\r50%\r100% done\r").as_deref(),
            Some("100% done")
        );
        assert_eq!(clean_line(b"   \r  "), None);
    }

    #[test]
    fn ids_are_short_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 6);
        assert_ne!(a, b);
    }
}
