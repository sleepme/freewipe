//! Installing / removing the Explorer integration from the GUI (with UAC
//! for "all users"), and finding entries left by other freewipe versions.

use freewipe::settings::ShellConfig;
use freewipe::shell::{self, ShellStatus};
#[allow(unused_imports)]
use freewipe::{i18n, tr};

#[cfg(windows)]
pub fn apply_impl(cfg: &ShellConfig, previous: Option<&ShellConfig>) -> Result<String, String> {
    let gui = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(prev) = previous {
        if prev.all_users != cfg.all_users {
            remove(prev.all_users, &gui)?;
        }
    }
    if !cfg.enabled {
        let n = remove(cfg.all_users, &gui)?;
        return Ok(tr!("shell.removed", n = n, scope = scope(cfg.all_users)));
    }
    if cfg.all_users {
        let tmp = std::env::temp_dir().join(format!("freewipe-shell-{}.json", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string(cfg).unwrap_or_default()).map_err(|e| e.to_string())?;
        let params = format!(
            "shell install --config \"{}\" --gui \"{}\" --lang {}",
            tmp.display(),
            gui.display(),
            i18n::current_language()
        );
        let r = elevated(&gui, &params);
        let _ = std::fs::remove_file(&tmp);
        r?;
        Ok(tr!("shell.installed", n = "✔", scope = scope(true)))
    } else {
        let n = shell::install(cfg, &gui).map_err(|e| tr!("shell.failed", err = e))?;
        Ok(tr!("shell.installed", n = n, scope = scope(false)))
    }
}

#[cfg(windows)]
pub fn remove(all_users: bool, gui: &std::path::Path) -> Result<usize, String> {
    if all_users {
        elevated(gui, "shell uninstall --all-users").map(|_| 0)
    } else {
        shell::uninstall(false).map_err(|e| tr!("shell.failed", err = e))
    }
}

/// Run `freewipe.exe <params>` with a UAC prompt.
#[cfg(windows)]
pub fn elevated(gui: &std::path::Path, params: &str) -> Result<(), String> {
    let cli = gui.with_file_name("freewipe.exe");
    if !cli.exists() {
        return Err(tr!("gui.cli_missing"));
    }
    match freewipe::sys::run_elevated(&cli, params) {
        Ok(0) => Ok(()),
        Ok(code) => Err(tr!("gui.elevated_failed", code = code)),
        Err(e) if e.raw_os_error() == Some(1223) => Err(tr!("gui.uac_cancelled")),
        Err(e) => Err(tr!("shell.failed", err = e)),
    }
}

#[cfg(not(windows))]
pub fn apply_impl(_cfg: &ShellConfig, _previous: Option<&ShellConfig>) -> Result<String, String> {
    Err(tr!("shell.windows_only"))
}

#[cfg(windows)]
pub fn scope(all_users: bool) -> String {
    if all_users {
        tr!("shell.scope_all")
    } else {
        tr!("shell.scope_user")
    }
}

/// Context-menu entries written by another freewipe version or copy:
/// `(all_users, status)` for each hive that has them.
pub fn foreign_entries() -> Vec<(bool, ShellStatus)> {
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    [false, true].into_iter().map(|all| (all, shell::status(all))).filter(|(_, st)| st.outdated(&exe)).collect()
}

/// Replace the entries in one hive with this version's (current settings).
pub fn update_scope(cfg: &ShellConfig, all_users: bool) -> Result<String, String> {
    let cfg = ShellConfig { enabled: true, all_users, ..cfg.clone() };
    apply_impl(&cfg, None)
}

#[cfg(windows)]
pub fn remove_scope(all_users: bool) -> Result<String, String> {
    let gui = std::env::current_exe().map_err(|e| e.to_string())?;
    let n = remove(all_users, &gui)?;
    Ok(tr!("shell.removed", n = n, scope = scope(all_users)))
}

#[cfg(not(windows))]
pub fn remove_scope(_all_users: bool) -> Result<String, String> {
    Err(tr!("shell.windows_only"))
}
