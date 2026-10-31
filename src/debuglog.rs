//! Optional debug log (Settings → General → Diagnostics, or the
//! `FREEWIPE_DEBUG_LOG` environment variable).
//!
//! When enabled, every freewipe process appends lines to
//! `%LOCALAPPDATA%\freewipe\logs\freewipe-YYYYMMDD.log`:
//!
//! ```text
//! 2026-10-30T12:00:01.123Z [4242] gui    | job start: 3 targets, method dod-e
//! ```
//!
//! The marker file `logs\enabled` tells the Explorer extension
//! (`freewipe_shell.dll`, which does not read the settings) to log to
//! `logs\shell.log`. Files over 10 MB are rotated, logs older than 14 days
//! are removed. Passwords are never logged.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_SIZE: u64 = 10 * 1024 * 1024;
const KEEP_DAYS: u64 = 14;

static ON: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<(String, File)>> = Mutex::new(None);
static COMPONENT: Mutex<&'static str> = Mutex::new("lib");

/// The folder holding the logs.
pub fn dir() -> PathBuf {
    std::env::var_os("FREEWIPE_LOG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::settings::data_dir().join("logs"))
}

/// Name the running program in log lines (`gui`, `cli`, …).
pub fn set_component(name: &'static str) {
    if let Ok(mut c) = COMPONENT.lock() {
        *c = name;
    }
}

/// `true` while logging is on.
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Switch logging on or off (also creates/removes the marker for the
/// Explorer extension).
pub fn set_enabled(on: bool) {
    let was = ON.swap(on, Ordering::Relaxed);
    let marker = dir().join("enabled");
    if on {
        let _ = fs::create_dir_all(dir());
        if !marker.exists() {
            let _ = fs::write(&marker, format!("{} debug log is on\r\n", crate::app::name()));
        }
        if !was {
            cleanup();
            install_panic_hook();
            log(&format!(
                "log on: {} {} (freewipe {}) {} exe={} args={:?}",
                crate::app::name(),
                crate::app::version(),
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH,
                std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
                std::env::args().skip(1).collect::<Vec<_>>()
            ));
        }
    } else {
        if was {
            log_forced("log off");
        }
        let _ = fs::remove_file(marker);
        if let Ok(mut f) = FILE.lock() {
            *f = None;
        }
    }
}

/// Write one line (no-op when logging is off). Prefer the [`dlog!`] macro.
///
/// [`dlog!`]: crate::dlog
pub fn log(msg: &str) {
    if enabled() {
        log_forced(msg);
    }
}

fn log_forced(msg: &str) {
    let now = SystemTime::now();
    let ms = now.duration_since(UNIX_EPOCH).map(|d| d.subsec_millis()).unwrap_or(0);
    let stamp = crate::util::rfc3339(now);
    let day: String = stamp[..10].replace('-', "");
    let comp = COMPONENT.lock().map(|c| *c).unwrap_or("?");
    let line = format!(
        "{}.{ms:03}Z [{}] {comp:<6} | {}\r\n",
        stamp.trim_end_matches('Z'),
        std::process::id(),
        msg.replace('\n', "\n    ")
    );
    let Ok(mut g) = FILE.lock() else { return };
    let path = dir().join(format!("{}-{day}.log", crate::app::name()));
    let reopen = match &*g {
        Some((d, _)) => *d != day,
        None => true,
    };
    if reopen {
        *g = None;
        let _ = fs::create_dir_all(dir());
        rotate(&path);
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
            *g = Some((day, f));
        }
    } else if fs::metadata(&path).map(|m| m.len() > MAX_SIZE).unwrap_or(false) {
        *g = None;
        rotate(&path);
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
            *g = Some((day, f));
        }
    }
    if let Some((_, f)) = g.as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Rename a full log to `….1.log` (keeping one older copy).
fn rotate(path: &std::path::Path) {
    if fs::metadata(path).map(|m| m.len() > MAX_SIZE).unwrap_or(false) {
        let old = path.with_extension("1.log");
        let _ = fs::remove_file(&old);
        let _ = fs::rename(path, old);
    }
}

/// Remove logs older than [`KEEP_DAYS`].
fn cleanup() {
    let Ok(rd) = fs::read_dir(dir()) else { return };
    let limit = Duration::from_secs(KEEP_DAYS * 86_400);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".log") {
            continue;
        }
        let old =
            e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Delete every log file (the setting itself is unchanged). Returns how many
/// files were removed.
pub fn delete_all() -> usize {
    if let Ok(mut f) = FILE.lock() {
        *f = None;
    }
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(dir()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".log") && fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Total size of the log files, in bytes.
pub fn size() -> u64 {
    fs::read_dir(dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".log"))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let bt = std::backtrace::Backtrace::force_capture();
            log(&format!("PANIC: {info}\n{bt}"));
            prev(info);
        }));
    });
}

/// `dlog!("job start: {} targets", n)` — formats only when logging is on.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        if $crate::debuglog::enabled() {
            $crate::debuglog::log(&format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_deletes() {
        let d = std::env::temp_dir().join(format!("freewipe-log-test-{}", std::process::id()));
        std::env::set_var("FREEWIPE_LOG_DIR", &d);
        set_component("test");
        set_enabled(true);
        assert!(d.join("enabled").exists());
        crate::dlog!("hello {}", 42);
        let text: String = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".log"))
            .map(|e| fs::read_to_string(e.path()).unwrap())
            .collect();
        assert!(text.contains("test   | hello 42"), "{text}");
        set_enabled(false);
        assert!(!d.join("enabled").exists());
        assert!(delete_all() >= 1);
        let _ = fs::remove_dir_all(&d);
    }
}
