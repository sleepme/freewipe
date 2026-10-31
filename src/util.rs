//! Small helpers: human-readable sizes and durations, RFC 3339 timestamps,
//! random names, aligned buffers.

use rand::RngCore;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `1536` → `"1.5 KiB"`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// `3725 s` → `"1:02:05"`, `65 s` → `"1:05"`.
pub fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` without pulling in a date crate.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

/// Compact local-agnostic stamp for file names: `20261028-152233`.
pub fn file_stamp(t: SystemTime) -> String {
    rfc3339(t).replace(['-', ':'], "").replace('T', "-").trim_end_matches('Z').to_string()
}

/// Howard Hinnant's days-from-civil inverse.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Random lowercase hex string of `len` characters.
pub fn random_hex(rng: &mut dyn RngCore, len: usize) -> String {
    (0..len).map(|_| std::char::from_digit(rng.next_u32() & 0xF, 16).unwrap_or('0')).collect()
}

/// Drop Windows' verbatim prefix: `\\?\C:\x` → `C:\x`,
/// `\\?\UNC\srv\share` → `\\srv\share`. Other strings pass through.
pub fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// Quote one argument for a Windows command line so that
/// `CommandLineToArgvW` / the MSVC runtime read it back unchanged.
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// A heap buffer whose usable slice starts at an `align`-byte boundary and
/// whose length is a multiple of `align` (required by unbuffered I/O).
pub struct AlignedBuf {
    raw: Vec<u8>,
    off: usize,
    len: usize,
}

impl AlignedBuf {
    pub fn new(len: usize, align: usize) -> Self {
        let len = len.div_ceil(align).max(1) * align;
        let raw = vec![0u8; len + align];
        let addr = raw.as_ptr() as usize;
        let off = (align - addr % align) % align;
        AlignedBuf { raw, off, len }
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.raw[self.off..self.off + self.len]
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_793_200_000);
        assert_eq!(rfc3339(t), "2026-10-28T15:06:40Z");
        assert_eq!(file_stamp(t), "20261028-150640");
    }

    #[test]
    fn verbatim_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users"), r"C:\Users");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share"), r"\\srv\share");
        assert_eq!(strip_verbatim("/home/x"), "/home/x");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(r"C:\a b\x.txt"), r#""C:\a b\x.txt""#);
        assert_eq!(quote_arg(r"D:\"), r"D:\");
        assert_eq!(quote_arg(r"C:\my dir\"), r#""C:\my dir\\""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(""), r#""""#);
    }

    #[test]
    fn aligned_buffer() {
        let mut b = AlignedBuf::new(10_000, 4096);
        assert_eq!(b.len(), 12_288);
        assert_eq!(b.as_mut_slice().as_ptr() as usize % 4096, 0);
    }
}
