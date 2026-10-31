//! Explorer context-menu integration (Windows registry).
//!
//! Verbs are registered under `Software\Classes` in HKCU (current user, no
//! admin needed) or HKLM (all users, elevated):
//!
//! | Where        | Class key                                              |
//! |--------------|--------------------------------------------------------|
//! | files        | `*\shell\…`                                            |
//! | folders      | `Directory\shell\…`                                    |
//! | drives       | `Drive\shell\…` (wipe free space)                      |
//! | Recycle Bin  | `CLSID\{645FF040-5081-101B-9F08-00AA002F954E}\shell\…` |
//!
//! Every key freewipe creates carries the value `FreewipeOwned = "1"`, and only
//! such keys are ever removed. Top-level verbs also record `FreewipeVersion`
//! and `FreewipeExe`, so a newer freewipe can recognise entries left by an older
//! one (or by a copy in another folder) and offer to replace them.
//!
//! Two launch modes:
//! * **ExplorerCommand** (default): each verb carries
//!   `ExplorerCommandHandler = {CLSID}` served by `freewipe_shell.dll` (an
//!   in-process `IExplorerCommand`). Explorer calls the handler directly with
//!   the whole selection instead of "opening" the items via `ShellExecute`,
//!   so Windows runs no zone / SmartScreen check on them: no "Open File –
//!   Security Warning" and no long busy cursor for large downloaded files.
//!   The DLL is copied to a per-version folder (`%LOCALAPPDATA%` or
//!   `%ProgramData%\freewipe\shell\<version>`) so Explorer never locks the
//!   copy you unpacked and updates can replace it.
//! * **Command**: the classic `"freewipe-gui.exe" --wipe "%1"` (one process per
//!   item), used when the DLL is missing.
//!
//! Note for Windows 11: classic registry verbs appear under
//! "Show more options" (Shift+F10). The compact top-level menu only accepts
//! handlers from packaged (MSIX) apps.
//!
//! The registry layout is computed by the pure [`plan_entries`], so it can be
//! unit-tested anywhere and exported as a `.reg` file.

use std::path::Path;

use crate::methods::Method;
use crate::settings::{LaunchMode, ShellConfig};
use crate::tr;

pub const CLASS_FILES: &str = "*";
pub const CLASS_DIRS: &str = "Directory";
pub const CLASS_DRIVES: &str = "Drive";
pub const CLASS_RECYCLE_BIN: &str = r"CLSID\{645FF040-5081-101B-9F08-00AA002F954E}";
pub const ALL_CLASSES: [&str; 4] = [CLASS_FILES, CLASS_DIRS, CLASS_DRIVES, CLASS_RECYCLE_BIN];
/// Marker value on every key freewipe owns.
pub const OWNED_MARKER: &str = "FreewipeOwned";
/// Version that wrote the entries.
pub const VERSION_MARKER: &str = "FreewipeVersion";
/// Executable the entries launch.
pub const EXE_MARKER: &str = "FreewipeExe";

/// What a context-menu verb does; each has its own COM class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    Wipe,
    Queue,
    FreeSpace,
    /// Move to another folder, then wipe the original.
    Move,
    Method(Method),
}

/// CLSIDs of the legacy COM drop targets (served by `freewipe-gui.exe
/// -Embedding`, used by pre-release builds); kept to recognise and remove them.
const LEGACY_CLSID_BASE: u128 = 0x7C4A_9E10_52B1_4D6F_9E3A_0A1B_2C3D_4E00;
/// CLSIDs of the `IExplorerCommand` handlers in `freewipe_shell.dll`
/// (must match `CLSID_BASE` in `shell/src/lib.rs`).
pub const HANDLER_CLSID_BASE: u128 = 0x7C4A_9E10_52B1_4D6F_9E3A_0A1B_2C3D_4F00;
/// File name of the shell extension next to the executables.
pub const SHELL_DLL: &str = "freewipe_shell.dll";

fn clsid_str(v: u128) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (v >> 96) as u32,
        (v >> 80) as u16,
        (v >> 64) as u16,
        (v >> 48) as u16,
        v & 0xFFFF_FFFF_FFFF
    )
}

impl DropAction {
    /// Every action that has a COM class.
    pub fn all() -> Vec<DropAction> {
        let mut v = vec![DropAction::Wipe, DropAction::Queue, DropAction::FreeSpace, DropAction::Move];
        v.extend(Method::ALL.iter().map(|m| DropAction::Method(*m)));
        v
    }

    /// Stable number of the action (`--shell <n>`, low byte of the CLSID).
    pub fn index(self) -> u32 {
        match self {
            DropAction::Wipe => 0,
            DropAction::Queue => 1,
            DropAction::FreeSpace => 2,
            DropAction::Move => 3,
            DropAction::Method(m) => 0x10 + Method::ALL.iter().position(|x| *x == m).unwrap_or(0) as u32,
        }
    }

    pub fn from_index(i: u32) -> Option<DropAction> {
        DropAction::all().into_iter().find(|a| a.index() == i)
    }

    /// The handler CLSID as a 128-bit value.
    pub fn clsid_u128(self) -> u128 {
        HANDLER_CLSID_BASE + u128::from(self.index())
    }

    /// The handler CLSID in registry form: `{7C4A9E10-52B1-4D6F-9E3A-0A1B2C3D4F00}`.
    pub fn clsid(self) -> String {
        clsid_str(self.clsid_u128())
    }

    /// CLSID of the legacy drop target for this action.
    pub fn legacy_clsid(self) -> String {
        clsid_str(LEGACY_CLSID_BASE + u128::from(self.index()))
    }

    pub fn legacy_clsid_u128(self) -> u128 {
        LEGACY_CLSID_BASE + u128::from(self.index())
    }

    /// Path of the shell extension the entries would use for `gui_exe`
    /// (for previews and `.reg` export; `install` deploys its own copy).
    pub fn dll_next_to(gui_exe: &Path) -> Option<std::path::PathBuf> {
        let p = gui_exe.with_file_name(SHELL_DLL);
        p.is_file().then_some(p)
    }

    pub fn legacy_from_clsid_u128(v: u128) -> Option<DropAction> {
        DropAction::all().into_iter().find(|a| LEGACY_CLSID_BASE + u128::from(a.index()) == v)
    }

    /// Equivalent command-line arguments (without the paths).
    pub fn args(self) -> Vec<String> {
        match self {
            DropAction::Wipe => vec!["--wipe".into()],
            DropAction::Queue => vec!["--enqueue".into()],
            DropAction::FreeSpace => vec!["--freespace".into()],
            DropAction::Move => vec!["--move".into()],
            DropAction::Method(m) => vec!["--wipe".into(), "--method".into(), m.key().into()],
        }
    }
}

/// One registry key to create, relative to `Software\Classes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegEntry {
    pub key: String,
    /// `(None, v)` is the key's default value.
    pub values: Vec<(Option<String>, String)>,
}

fn entry(key: String, values: Vec<(Option<&str>, String)>) -> RegEntry {
    RegEntry { key, values: values.into_iter().map(|(n, v)| (n.map(str::to_string), v)).collect() }
}

/// Values for a visible top-level verb key.
fn verb_values(
    cfg: &ShellConfig,
    label: String,
    icon: &str,
    submenu: bool,
    exe: &str,
) -> Vec<(Option<&'static str>, String)> {
    let mut v = vec![
        (Some("MUIVerb"), label),
        (Some(OWNED_MARKER), "1".into()),
        (Some(VERSION_MARKER), crate::VERSION.into()),
        (Some(EXE_MARKER), exe.to_string()),
    ];
    if cfg.show_icon {
        v.push((Some("Icon"), icon.to_string()));
    }
    if cfg.shift_only {
        v.push((Some("Extended"), String::new()));
    }
    if submenu {
        v.push((Some("SubCommands"), String::new()));
    }
    v
}

/// Compute every key/value to create for `cfg`.
///
/// `gui_exe` is the full path of `freewipe-gui.exe`.
pub fn plan_entries(cfg: &ShellConfig, gui_exe: &Path, dll: Option<&Path>) -> Vec<RegEntry> {
    // Never write `\\?\` paths: Explorer and comparisons expect plain ones.
    let exe = crate::util::strip_verbatim(&gui_exe.display().to_string());
    let icon = format!("{exe},0");
    let cmd = |args: &str| format!("\"{exe}\" {args}");
    let handler = if cfg.launch == LaunchMode::ExplorerCommand { dll } else { None };
    let mut out = Vec::new();
    let mut used: Vec<DropAction> = Vec::new();
    // How a verb key `key` is launched. In handler mode the verb's own key
    // gets the CLSID (the values are merged into the entry already pushed).
    let mut launch = |out: &mut Vec<RegEntry>, key: &str, action: DropAction, args: String| {
        if handler.is_some() {
            if let Some(e) = out.iter_mut().rev().find(|e| e.key == key) {
                e.values.push((Some("ExplorerCommandHandler".into()), action.clsid()));
                e.values.push((Some("MultiSelectModel".into()), "Player".into()));
            }
            if !used.contains(&action) {
                used.push(action);
            }
        } else {
            out.push(entry(format!(r"{key}\command"), vec![(None, cmd(&args))]));
        }
    };

    // Files and folders share the same item set.
    let mut item_classes = Vec::new();
    if cfg.files {
        item_classes.push(CLASS_FILES);
    }
    if cfg.dirs {
        item_classes.push(CLASS_DIRS);
    }

    for class in item_classes {
        let shell = format!(r"{class}\shell");
        // (key suffix, label, action, is method submenu)
        let mut items: Vec<(&str, String, DropAction, bool)> = Vec::new();
        if cfg.item_wipe {
            items.push(("01wipe", tr!("shell.item_wipe"), DropAction::Wipe, false));
        }
        if cfg.item_method {
            items.push(("02method", tr!("shell.item_method"), DropAction::Wipe, true));
        }
        if cfg.item_queue {
            items.push(("03queue", tr!("shell.item_queue"), DropAction::Queue, false));
        }
        if cfg.item_move {
            items.push(("04move", tr!("shell.item_move"), DropAction::Move, false));
        }
        if items.is_empty() {
            continue;
        }

        let base = if cfg.submenu {
            let root = format!(r"{shell}\freewipe");
            out.push(entry(root.clone(), verb_values(cfg, "freewipe".into(), &icon, true, &exe)));
            format!(r"{root}\shell")
        } else {
            shell.clone()
        };

        for (suffix, label, action, is_method) in items {
            let key =
                if cfg.submenu { format!(r"{base}\{suffix}") } else { format!(r"{base}\freewipe.{}", &suffix[2..]) };
            let mut vals = if cfg.submenu {
                vec![(Some("MUIVerb"), label.clone()), (Some(OWNED_MARKER), "1".into())]
            } else {
                verb_values(cfg, format!("freewipe: {label}"), &icon, is_method, &exe)
            };
            if is_method && cfg.submenu {
                vals.push((Some("SubCommands"), String::new()));
            }
            out.push(entry(key.clone(), vals));
            if is_method {
                for (i, m) in Method::ALL.iter().enumerate() {
                    let mk = format!(r"{key}\shell\{:02}{}", i + 1, m.key());
                    let label = format!("{} · {}", m.name(), tr!("cli.passes_n", n = m.pass_count(None)));
                    out.push(entry(mk.clone(), vec![(Some("MUIVerb"), label), (Some(OWNED_MARKER), "1".into())]));
                    launch(&mut out, &mk, DropAction::Method(*m), format!("--wipe --method {} \"%1\"", m.key()));
                }
            } else {
                let args = format!("{} \"%1\"", action.args().join(" "));
                launch(&mut out, &key, action, args);
            }
        }
    }

    if cfg.drives {
        let key = format!(r"{CLASS_DRIVES}\shell\freewipe.freespace");
        out.push(entry(key.clone(), verb_values(cfg, tr!("shell.item_freespace"), &icon, false, &exe)));
        // Unquoted on purpose: `"D:\"` would make the backslash escape the
        // closing quote under Windows command-line rules. Drive roots never
        // contain spaces.
        launch(&mut out, &key, DropAction::FreeSpace, "--freespace %1".into());
    }
    if cfg.recycle_bin {
        let key = format!(r"{CLASS_RECYCLE_BIN}\shell\freewipe.recyclebin");
        out.push(entry(key.clone(), verb_values(cfg, tr!("shell.item_recycle"), &icon, false, &exe)));
        // No selection to hand over: always a plain command.
        out.push(entry(format!(r"{key}\command"), vec![(None, cmd("--recycle-bin"))]));
    }

    // COM classes of the handlers, served by freewipe_shell.dll.
    if let Some(dll) = handler {
        for a in used {
            let ck = format!(r"CLSID\{}", a.clsid());
            out.push(entry(
                ck.clone(),
                vec![
                    (None, format!("freewipe {a:?}")),
                    (Some(OWNED_MARKER), "1".into()),
                    (Some(EXE_MARKER), exe.clone()),
                ],
            ));
            out.push(entry(
                format!(r"{ck}\InprocServer32"),
                vec![
                    (None, crate::util::strip_verbatim(&dll.display().to_string())),
                    (Some("ThreadingModel"), "Apartment".into()),
                ],
            ));
        }
    }
    out
}

/// Render entries as a `.reg` file (for manual import / review).
pub fn to_reg_file(entries: &[RegEntry], all_users: bool) -> String {
    let hive = if all_users { "HKEY_LOCAL_MACHINE" } else { "HKEY_CURRENT_USER" };
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut s = String::from("Windows Registry Editor Version 5.00\r\n");
    for e in entries {
        s.push_str(&format!("\r\n[{hive}\\Software\\Classes\\{}]\r\n", e.key));
        for (n, v) in &e.values {
            match n {
                None => s.push_str(&format!("@=\"{}\"\r\n", esc(v))),
                Some(n) => s.push_str(&format!("\"{}\"=\"{}\"\r\n", esc(n), esc(v))),
            }
        }
    }
    s
}

/// What is currently registered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellStatus {
    /// Top-level freewipe verbs found (per class).
    pub verbs: Vec<String>,
    /// The executable the first found command points at.
    pub command: Option<String>,
    /// `FreewipeVersion` of the entries (`None` = written by an early pre-release build).
    pub version: Option<String>,
    /// Executable the entries launch.
    pub exe: Option<String>,
    /// Entries use the legacy COM drop target.
    pub drop_target: bool,
    /// Entries use the shell extension (freewipe_shell.dll).
    pub handler: bool,
}

impl ShellStatus {
    pub fn installed(&self) -> bool {
        !self.verbs.is_empty()
    }

    /// Installed by another freewipe version or another copy of freewipe.
    pub fn outdated(&self, current_exe: &Path) -> bool {
        if !self.installed() {
            return false;
        }
        let same_version = self.version.as_deref() == Some(crate::VERSION);
        let same_exe = self
            .exe
            .as_deref()
            .map(|e| {
                crate::util::strip_verbatim(e.trim_matches('"'))
                    .eq_ignore_ascii_case(&crate::util::strip_verbatim(&current_exe.display().to_string()))
            })
            .unwrap_or(false);
        !(same_version && same_exe)
    }

    /// Human-readable "version" for messages.
    pub fn version_label(&self) -> String {
        self.version.clone().unwrap_or_else(|| "(unknown version)".into())
    }
}

/// Executable path from a `command` value such as `"C:\x\freewipe-gui.exe" --wipe "%1"`.
pub fn exe_from_command(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().map(str::to_string)
    } else {
        c.split_whitespace().next().map(str::to_string)
    }
}

/// Path of `freewipe-gui(.exe)` next to the running executable.
pub fn sibling_gui_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) { "freewipe-gui.exe" } else { "freewipe-gui" };
    if exe.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false) {
        return Some(exe);
    }
    let p = exe.with_file_name(name);
    p.exists().then_some(p)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::sys::{self, RegRoot};
    use std::io;

    fn root(all_users: bool) -> RegRoot {
        if all_users {
            RegRoot::LocalMachine
        } else {
            RegRoot::CurrentUser
        }
    }

    /// Remove every freewipe-owned verb from `hive`. Returns how many were removed.
    pub fn uninstall(all_users: bool) -> io::Result<usize> {
        let r = root(all_users);
        let mut removed = 0;
        for class in ALL_CLASSES {
            let shell = format!(r"Software\Classes\{class}\shell");
            let Some(k) = sys::reg_open(r, &shell)? else { continue };
            for sub in k.subkeys() {
                if !(sub.eq_ignore_ascii_case("freewipe") || sub.to_ascii_lowercase().starts_with("freewipe.")) {
                    continue;
                }
                let full = format!(r"{shell}\{sub}");
                let owned = sys::reg_open(r, &full)?.and_then(|k| k.get_string(Some(OWNED_MARKER))).is_some();
                if owned {
                    sys::reg_delete_tree(r, &full)?;
                    removed += 1;
                }
            }
        }
        for a in DropAction::all() {
            for clsid in [a.clsid(), a.legacy_clsid()] {
                let ck = format!(r"Software\Classes\CLSID\{clsid}");
                if sys::reg_open(r, &ck)?.and_then(|k| k.get_string(Some(OWNED_MARKER))).is_some() {
                    sys::reg_delete_tree(r, &ck)?;
                }
            }
        }
        sys::notify_assoc_changed();
        crate::dlog!("shell uninstall (all_users={all_users}): {removed} verbs removed");
        Ok(removed)
    }

    /// Copy `freewipe_shell.dll` (next to `gui_exe`) into a per-version folder
    /// that Explorer may keep locked without blocking later updates.
    fn deploy_dll(gui_exe: &Path, all_users: bool) -> Option<std::path::PathBuf> {
        let src = gui_exe.with_file_name(SHELL_DLL);
        if !src.is_file() {
            return None;
        }
        let base = if all_users {
            std::env::var_os("ProgramData").map(std::path::PathBuf::from)?
        } else {
            crate::settings::data_dir().parent()?.to_path_buf()
        };
        let root = base.join("freewipe").join("shell");
        let dir = root.join(crate::VERSION);
        let dst = dir.join(SHELL_DLL);
        // Copies from other versions: removed unless Explorer still has
        // them loaded (then they go on a later install).
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy() != crate::VERSION {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let same =
            |a: &Path, b: &Path| -> bool { matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y) };
        if dst.is_file() && same(&src, &dst) {
            return Some(dst);
        }
        std::fs::create_dir_all(&dir).ok()?;
        match std::fs::copy(&src, &dst) {
            Ok(_) => Some(dst),
            // Loaded by Explorer already (same version, other build): keep
            // a uniquely named copy next to it.
            Err(_) => {
                let alt = dir.join(format!("freewipe_shell-{}.dll", std::process::id()));
                std::fs::copy(&src, &alt).ok().map(|_| alt)
            }
        }
    }

    /// Replace any existing freewipe verbs with the ones for `cfg`.
    pub fn install(cfg: &ShellConfig, gui_exe: &Path) -> io::Result<usize> {
        uninstall(cfg.all_users)?;
        let r = root(cfg.all_users);
        let dll = if cfg.launch == LaunchMode::ExplorerCommand { deploy_dll(gui_exe, cfg.all_users) } else { None };
        let entries = plan_entries(cfg, gui_exe, dll.as_deref());
        crate::dlog!(
            "shell install: all_users={} launch={:?} exe={} dll={:?} entries={}",
            cfg.all_users,
            cfg.launch,
            gui_exe.display(),
            dll,
            entries.len()
        );
        for e in &entries {
            let k = sys::reg_create(r, &format!(r"Software\Classes\{}", e.key)).inspect_err(|err| {
                crate::dlog!("shell install: create {} failed: {err}", e.key);
            })?;
            for (n, v) in &e.values {
                k.set_string(n.as_deref(), v)?;
            }
        }
        sys::notify_assoc_changed();
        Ok(entries.len())
    }

    pub fn status(all_users: bool) -> ShellStatus {
        let r = root(all_users);
        let mut st = ShellStatus::default();
        for class in ALL_CLASSES {
            let shell = format!(r"Software\Classes\{class}\shell");
            let Ok(Some(k)) = sys::reg_open(r, &shell) else { continue };
            for sub in k.subkeys() {
                if !(sub.eq_ignore_ascii_case("freewipe") || sub.to_ascii_lowercase().starts_with("freewipe.")) {
                    continue;
                }
                let full = format!(r"{shell}\{sub}");
                if let Ok(Some(vk)) = sys::reg_open(r, &full) {
                    if vk.get_string(Some(OWNED_MARKER)).is_some() {
                        st.verbs.push(format!(r"{class}\shell\{sub}"));
                        if st.version.is_none() {
                            st.version = vk.get_string(Some(VERSION_MARKER));
                        }
                        if st.exe.is_none() {
                            st.exe = vk.get_string(Some(EXE_MARKER));
                        }
                        if st.command.is_none() {
                            st.command = find_command(r, &full);
                        }
                        st.drop_target |= find_drop_target(r, &full);
                        st.handler |= find_value(r, &full, "ExplorerCommandHandler");
                    }
                }
            }
        }
        if st.exe.is_none() {
            st.exe = st.command.as_deref().and_then(exe_from_command);
        }
        st
    }

    /// Is there a `DropTarget` at or below `key`?
    fn find_drop_target(r: RegRoot, key: &str) -> bool {
        if matches!(sys::reg_open(r, &format!(r"{key}\DropTarget")), Ok(Some(_))) {
            return true;
        }
        let Ok(Some(k)) = sys::reg_open(r, &format!(r"{key}\shell")) else { return false };
        k.subkeys().into_iter().any(|s| find_drop_target(r, &format!(r"{key}\shell\{s}")))
    }

    /// Does `key` or a verb below it have the value `name`?
    fn find_value(r: RegRoot, key: &str, name: &str) -> bool {
        if matches!(sys::reg_open(r, key), Ok(Some(k)) if k.get_string(Some(name)).is_some()) {
            return true;
        }
        let Ok(Some(k)) = sys::reg_open(r, &format!(r"{key}\shell")) else { return false };
        k.subkeys().into_iter().any(|s| find_value(r, &format!(r"{key}\shell\{s}"), name))
    }

    /// First `command` default value at or below `key`.
    fn find_command(r: RegRoot, key: &str) -> Option<String> {
        if let Ok(Some(c)) = sys::reg_open(r, &format!(r"{key}\command")) {
            return c.get_string(None);
        }
        let k = sys::reg_open(r, &format!(r"{key}\shell")).ok()??;
        k.subkeys().into_iter().find_map(|s| find_command(r, &format!(r"{key}\shell\{s}")))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, tr!("shell.windows_only"))
    }
    pub fn uninstall(_all_users: bool) -> io::Result<usize> {
        Err(unsupported())
    }
    pub fn install(_cfg: &ShellConfig, _gui_exe: &Path) -> io::Result<usize> {
        Err(unsupported())
    }
    pub fn status(_all_users: bool) -> ShellStatus {
        ShellStatus::default()
    }
}

pub use imp::{install, status, uninstall};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\freewipe\freewipe-gui.exe")
    }

    #[test]
    fn submenu_layout() {
        let cfg = ShellConfig { launch: LaunchMode::Command, ..Default::default() };
        let e = plan_entries(&cfg, &exe(), None);
        let keys: Vec<&str> = e.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&r"*\shell\freewipe"));
        assert!(keys.contains(&r"*\shell\freewipe\shell\01wipe\command"));
        assert!(keys.contains(&r"Directory\shell\freewipe\shell\03queue\command"));
        assert!(keys.contains(&r"Drive\shell\freewipe.freespace\command"));
        assert!(!keys.iter().any(|k| k.starts_with("CLSID")), "recycle bin is off by default");
        // Method submenu lists every method.
        let methods = keys
            .iter()
            .filter(|k| k.starts_with(r"*\shell\freewipe\shell\02method\shell\") && k.ends_with(r"\command"))
            .count();
        assert_eq!(methods, Method::ALL.len());
        // Commands quote the exe and the argument.
        let wipe = e.iter().find(|e| e.key == r"*\shell\freewipe\shell\01wipe\command").unwrap();
        assert_eq!(wipe.values[0].1, "\"C:\\Program Files\\freewipe\\freewipe-gui.exe\" --wipe \"%1\"");
        // Every visible verb is marked as ours.
        assert!(e
            .iter()
            .filter(|e| !e.key.ends_with("command"))
            .all(|e| e.values.iter().any(|(n, _)| n.as_deref() == Some(OWNED_MARKER))));
    }

    #[test]
    fn flat_shift_only_and_recycle() {
        let cfg = ShellConfig {
            submenu: false,
            shift_only: true,
            recycle_bin: true,
            item_method: false,
            ..Default::default()
        };
        let e = plan_entries(&cfg, &exe(), None);
        let wipe = e.iter().find(|e| e.key == r"*\shell\freewipe.wipe").expect("flat verb");
        assert!(wipe.values.iter().any(|(n, _)| n.as_deref() == Some("Extended")));
        assert!(e.iter().any(|e| e.key.ends_with(r"freewipe.recyclebin\command")));
        assert!(!e.iter().any(|e| e.key.contains("method")));
    }

    #[test]
    fn nothing_selected_means_nothing_registered() {
        let cfg = ShellConfig { files: false, dirs: false, drives: false, recycle_bin: false, ..Default::default() };
        assert!(plan_entries(&cfg, &exe(), None).is_empty());
    }

    #[test]
    fn reg_file_escapes() {
        let e = plan_entries(&ShellConfig { launch: LaunchMode::Command, ..Default::default() }, &exe(), None);
        let reg = to_reg_file(&e, false);
        assert!(reg.starts_with("Windows Registry Editor Version 5.00"));
        assert!(reg.contains(r"[HKEY_CURRENT_USER\Software\Classes\*\shell\freewipe]"));
        assert!(reg.contains(r#"@="\"C:\\Program Files\\freewipe\\freewipe-gui.exe\" --wipe \"%1\"""#));
    }

    #[test]
    fn handler_layout() {
        let dll = PathBuf::from(r"C:\Users\me\AppData\Local\freewipe\shell\1.0.0\freewipe_shell.dll");
        let e = plan_entries(&ShellConfig::default(), &exe(), Some(&dll));
        let find = |k: &str| e.iter().find(|x| x.key == k);
        let val = |k: &str, n: &str| {
            find(k).and_then(|x| x.values.iter().find(|(m, _)| m.as_deref() == Some(n)).map(|(_, v)| v.clone()))
        };
        // Verbs name their handler class instead of a command line…
        assert_eq!(val(r"*\shell\freewipe\shell\01wipe", "ExplorerCommandHandler"), Some(DropAction::Wipe.clsid()));
        assert_eq!(val(r"*\shell\freewipe\shell\01wipe", "MultiSelectModel").as_deref(), Some("Player"));
        assert!(!e.iter().any(|x| x.key.ends_with(r"\command") && !x.key.contains("recyclebin")));
        assert!(val(r"Drive\shell\freewipe.freespace", "ExplorerCommandHandler").is_some());
        assert!(find(r"*\shell\freewipe\shell\04move").is_some());
        // …served in-process by the DLL, which learns where freewipe-gui is.
        let ck = format!(r"CLSID\{}", DropAction::Wipe.clsid());
        assert_eq!(val(&format!(r"{ck}\InprocServer32"), "ThreadingModel").as_deref(), Some("Apartment"));
        assert_eq!(find(&format!(r"{ck}\InprocServer32")).unwrap().values[0].1, dll.display().to_string());
        assert_eq!(val(&ck, EXE_MARKER), Some(exe().display().to_string()));
        assert!(find(&format!(r"CLSID\{}", DropAction::Method(Method::Gutmann).clsid())).is_some());
        // Top-level verbs carry version and exe markers.
        let top = find(r"*\shell\freewipe").unwrap();
        assert!(top.values.iter().any(|(n, v)| n.as_deref() == Some(VERSION_MARKER) && v == crate::VERSION));
        assert!(top.values.iter().any(|(n, _)| n.as_deref() == Some(EXE_MARKER)));
        // Without the DLL: plain commands.
        let e = plan_entries(&ShellConfig::default(), &exe(), None);
        assert!(e.iter().any(|x| x.key == r"*\shell\freewipe\shell\01wipe\command"));
    }

    #[test]
    fn clsids_are_unique_and_formatted() {
        let all = DropAction::all();
        let mut ids: Vec<String> = all.iter().map(|a| a.clsid()).collect();
        assert_eq!(DropAction::Wipe.clsid(), "{7C4A9E10-52B1-4D6F-9E3A-0A1B2C3D4F00}");
        assert_eq!(DropAction::Wipe.legacy_clsid(), "{7C4A9E10-52B1-4D6F-9E3A-0A1B2C3D4E00}");
        for a in &all {
            assert_eq!(DropAction::legacy_from_clsid_u128(a.legacy_clsid_u128()), Some(*a));
            assert_eq!(DropAction::from_index(a.index()), Some(*a));
            assert!(a.index() < 0x40, "fits the DLL's slot range");
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn outdated_detection() {
        let cur = exe();
        let mut st = ShellStatus { verbs: vec!["x".into()], ..Default::default() };
        assert!(st.outdated(&cur), "no version marker = older freewipe");
        st.version = Some(crate::VERSION.into());
        st.exe = Some(r"c:\program files\freewipe\FREEWIPE-GUI.EXE".into());
        assert!(!st.outdated(&cur));
        st.exe = Some(r"D:\old\freewipe-gui.exe".into());
        assert!(st.outdated(&cur), "another copy");
        assert!(!ShellStatus::default().outdated(&cur));
        assert_eq!(
            exe_from_command(r#""C:\a b\freewipe-gui.exe" --wipe "%1""#).as_deref(),
            Some(r"C:\a b\freewipe-gui.exe")
        );
    }
}
