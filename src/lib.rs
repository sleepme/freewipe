//! freewipe — lightweight, open-source secure data wiping utility.
//!
//! Layering (top to bottom):
//!
//! * [`wipe`] — orchestration: files (all streams) → verify → rename →
//!   truncate → delete; directory trees; free space; the Recycle Bin.
//! * [`methods`] / [`pattern`] — the catalogue of sanitization standards and
//!   the buffer-fill primitive. Pure logic, identical on every platform.
//! * [`filesystem`], [`media`], [`safety`], [`report`], [`plan`] — volume
//!   and medium detection, protected-path rules, JSON reports, dry-run plans.
//! * [`i18n`] — every user-visible string goes through here (en default,
//!   uk, ru built in; more via JSON files).
//! * [`sys`] — the OS backend. On Windows this is [`windows_io`], the *only*
//!   module allowed to contain `unsafe`. Elsewhere a std-only portable
//!   backend is used (handy for tests and for ext4/btrfs volumes on Linux).
//! * [`shell`] / [`settings`] — Explorer context-menu integration and the
//!   persisted GUI settings.

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod app;
pub mod debuglog;
pub mod filesystem;
pub mod i18n;
pub mod lock;
pub mod media;
pub mod methods;
pub mod pattern;
pub mod plan;
pub mod progress;
pub mod report;
pub mod safety;
pub mod schedule;
pub mod settings;
pub mod shell;
pub mod util;
pub mod wipe;

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod windows_io;
#[cfg(windows)]
pub use windows_io as sys;

#[cfg(not(windows))]
#[allow(unsafe_code)]
pub mod portable_io;
#[cfg(not(windows))]
pub use portable_io as sys;

pub use methods::Method;
pub use pattern::{fill, Pass};

/// Crate version, shown in the GUI and written into reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Project home page (source code, releases, issues).
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// Author's profile.
pub const AUTHOR_URL: &str = "https://github.com/sleepme";
/// Where to support the project.
pub const DONATE_URL: &str = "https://ko-fi.com/tovld";

/// The application icon (also written next to the registry entries so the
/// context menu can show it).
pub const ICON_ICO: &[u8] = include_bytes!("../assets/freewipe.ico");
