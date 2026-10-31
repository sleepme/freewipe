//! Identity of the program using the library.
//!
//! freewipe's engine can be shared with companion tools. The name chosen
//! here decides the settings and data folders (`%APPDATA%\<name>`,
//! `%LOCALAPPDATA%\<name>`), the debug-log file names and the `tool` field of
//! reports. Call [`set_identity`] once at start-up, before anything else;
//! without it everything stays `freewipe`.

use std::sync::OnceLock;

static IDENTITY: OnceLock<(&'static str, &'static str)> = OnceLock::new();

/// Set the program name and version (the first call wins).
pub fn set_identity(name: &'static str, version: &'static str) {
    let _ = IDENTITY.set((name, version));
}

/// Program name (`freewipe` unless set).
pub fn name() -> &'static str {
    IDENTITY.get().map(|i| i.0).unwrap_or("freewipe")
}

/// Program version (freewipe's own unless set).
pub fn version() -> &'static str {
    IDENTITY.get().map(|i| i.1).unwrap_or(crate::VERSION)
}
