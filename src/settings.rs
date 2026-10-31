//! Persisted user settings (GUI and context-menu behaviour).
//!
//! Stored as JSON: `%APPDATA%\freewipe\settings.json` on Windows,
//! `$XDG_CONFIG_HOME/freewipe/settings.json` (or `~/.config/freewipe/…`) elsewhere.
//! Unknown or missing fields fall back to defaults, so old files keep working.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::methods::Method;
use crate::wipe::WipeOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

/// What "Wipe permanently" in the Explorer menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MenuAction {
    /// Open a compact window and wipe right away (after confirmation, if enabled).
    #[default]
    WipeNow,
    /// Only add to the queue of the main window.
    Enqueue,
}

/// How Explorer hands the selection to freewipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// Shell extension (`freewipe_shell.dll`, `IExplorerCommand`): Explorer
    /// passes the whole selection directly and does not "open" the items
    /// (no zone/SmartScreen check, no "Open File – Security Warning").
    /// Settings saying `drop_target` (legacy pre-release mode) map here.
    #[default]
    #[serde(alias = "drop_target")]
    ExplorerCommand,
    /// Classic `command` line, one process per selected item.
    Command,
}

/// How a wipe is confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmMode {
    /// A dialog with "Wipe" / "Cancel" buttons.
    #[default]
    Click,
    /// The user must type the confirmation word.
    Typed,
    /// No confirmation at all.
    None,
}

/// Explorer context-menu configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Desired state of the integration (the registry is the source of truth
    /// for what is actually installed; see `shell::status`).
    pub enabled: bool,
    pub files: bool,
    pub dirs: bool,
    pub drives: bool,
    pub recycle_bin: bool,
    pub item_wipe: bool,
    pub item_method: bool,
    pub item_queue: bool,
    /// "Move to… and wipe original".
    pub item_move: bool,
    /// Group items under a "freewipe ▸" submenu (otherwise top-level items).
    pub submenu: bool,
    pub show_icon: bool,
    /// Only show the items when Shift is held (Explorer "extended" verbs).
    pub shift_only: bool,
    /// Ask for confirmation when started from the context menu (in the style
    /// chosen by `Settings::confirm_mode`).
    pub always_confirm: bool,
    /// What "Wipe permanently" does.
    pub menu_action: MenuAction,
    /// How Explorer launches freewipe.
    pub launch: LaunchMode,
    /// Install for all users (HKLM, needs elevation) instead of the current one.
    pub all_users: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig {
            enabled: false,
            files: true,
            dirs: true,
            drives: true,
            recycle_bin: false,
            item_wipe: true,
            item_method: true,
            item_queue: true,
            item_move: true,
            submenu: true,
            show_icon: true,
            shift_only: false,
            always_confirm: true,
            menu_action: MenuAction::WipeNow,
            launch: LaunchMode::ExplorerCommand,
            all_users: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language code; English by default.
    pub language: String,
    pub theme: Theme,
    /// Flash the taskbar button when a job finishes.
    pub notify_done: bool,
    /// Prevent sleep while wiping.
    pub keep_awake: bool,

    /// Default method key.
    pub method: String,
    pub random_passes: usize,
    pub rename_passes: u32,
    pub verify: bool,
    pub wipe_ads: bool,
    pub keep: bool,

    pub ssd_warning: bool,
    pub force: bool,
    /// How wipes are confirmed.
    pub confirm_mode: ConfirmMode,
    /// Close the small Explorer window this many seconds after a fully
    /// successful run (0 = keep it open).
    pub finish_close_secs: u32,
    /// Keep the small Explorer window above other windows.
    pub mini_on_top: bool,
    /// On SSD/flash use a single pass (NIST Clear) instead of the chosen method.
    pub ssd_single_pass: bool,
    /// Free space: also overwrite cluster tips of existing files.
    pub freespace_tips: bool,
    /// Free space (NTFS): also overwrite free file records.
    pub freespace_records: bool,
    /// Show an icon with the progress in the notification area while wiping.
    pub tray_icon: bool,
    /// Minimizing the window during a wipe hides it to that icon.
    pub minimize_to_tray: bool,
    /// Restart freewipe-gui with administrator rights on every start.
    pub always_admin: bool,
    /// Password protection.
    pub lock: crate::lock::Lock,
    /// Wiping methods defined by the user.
    pub custom_methods: Vec<crate::methods::CustomMethod>,
    /// Scheduled wipes (Windows Task Scheduler).
    pub schedules: Vec<crate::schedule::Schedule>,
    /// Edits to the protected paths (Settings → Safety).
    pub protection: crate::safety::Protection,
    /// Keep a debug log of everything freewipe does.
    pub debug_log: bool,

    pub save_reports: bool,
    /// `None` = the default reports folder.
    pub report_dir: Option<PathBuf>,
    pub redact_reports: bool,

    pub shell: ShellConfig,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: crate::i18n::DEFAULT_LANG.into(),
            theme: Theme::System,
            notify_done: true,
            keep_awake: true,
            method: Method::DodE.key().into(),
            random_passes: 3,
            rename_passes: 3,
            verify: true,
            wipe_ads: true,
            keep: false,
            ssd_warning: true,
            force: false,
            confirm_mode: ConfirmMode::Click,
            finish_close_secs: 5,
            mini_on_top: false,
            ssd_single_pass: false,
            freespace_tips: false,
            freespace_records: false,
            tray_icon: true,
            minimize_to_tray: true,
            always_admin: false,
            lock: crate::lock::Lock::default(),
            custom_methods: Vec::new(),
            schedules: Vec::new(),
            protection: Default::default(),
            debug_log: false,
            save_reports: true,
            report_dir: None,
            redact_reports: false,
            shell: ShellConfig::default(),
        }
    }
}

/// Folder for settings.
pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("APPDATA") {
            return PathBuf::from(d).join(crate::app::name());
        }
    } else if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(d).join(crate::app::name());
    } else if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".config").join(crate::app::name());
    }
    std::env::temp_dir().join(crate::app::name())
}

/// Folder for reports, the context-menu inbox and the menu icon.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(d).join(crate::app::name());
        }
    } else if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(d).join(crate::app::name());
    } else if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".local").join("share").join(crate::app::name());
    }
    std::env::temp_dir().join(crate::app::name())
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn default_report_dir() -> PathBuf {
    data_dir().join("reports")
}

/// Where extra GUI instances (one per selected file) drop their arguments.
pub fn inbox_dir() -> PathBuf {
    data_dir().join("inbox")
}

impl Settings {
    /// Load the settings (defaults if missing or unreadable) and register
    /// the custom methods they define.
    pub fn load() -> Settings {
        let s: Settings = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        crate::methods::set_custom(&s.custom_methods);
        crate::safety::set_protection(&s.protection);
        crate::debuglog::set_enabled(s.debug_log || std::env::var_os("FREEWIPE_DEBUG_LOG").is_some());
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = settings_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write a temporary file and rename it, so a crash never leaves a
        // half-written settings.json behind.
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &p)
    }

    pub fn method(&self) -> Method {
        Method::from_key(&self.method).unwrap_or(Method::DodE)
    }

    pub fn report_dir(&self) -> PathBuf {
        self.report_dir.clone().unwrap_or_else(default_report_dir)
    }

    /// Engine options derived from these settings.
    pub fn wipe_options(&self) -> WipeOptions {
        WipeOptions {
            method: self.method(),
            passes_override: Some(self.random_passes.max(1)),
            verify: self.verify,
            rename_passes: self.rename_passes,
            keep: self.keep,
            include_ads: self.wipe_ads,
            flash_single_pass: self.ssd_single_pass,
            force: self.force,
            cluster_tips: self.freespace_tips,
            file_records: self.freespace_records,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_english_and_roundtrip() {
        let s = Settings::default();
        assert_eq!(s.language, "en");
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"language":"uk"}"#).unwrap();
        assert_eq!(s.language, "uk");
        assert_eq!(s.method, "dod-e");
        assert!(s.shell.submenu);
    }
}
