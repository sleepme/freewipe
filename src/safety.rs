//! Protected-path rules.
//!
//! Without `--force` freewipe refuses to wipe:
//! * anything **inside** OS / program directories (`%SystemRoot%`,
//!   `Program Files`, `ProgramData`, `/usr`, `/etc`, …);
//! * a **drive or file-system root** itself, the users root, or the user's
//!   home folder itself;
//! * its **own executable**, or a directory that contains it.
//!
//! The system folders are a built-in list the user can edit (Settings →
//! Safety): entries can be switched off, and the user can add their own
//! files or folders ("this folder and everything inside" or "exactly this
//! path"). Drive roots and freewipe itself are always protected.
//!
//! Free-space wiping only creates new files, so it is never refused.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::tr;
use crate::util::strip_verbatim;

/// Why a target was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Inside (or equal to) a protected system directory.
    SystemDir(PathBuf),
    /// A drive / file-system root.
    Root,
    /// The users root or the home folder itself.
    Home(PathBuf),
    /// freewipe's own executable, or a folder containing it.
    OwnExe,
    /// On the user's own list of protected paths.
    User(PathBuf),
}

/// One protected path from the user's list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedPath {
    /// May contain `%VARIABLES%` (expanded when checked).
    pub path: String,
    /// The folder and everything inside it (otherwise exactly this path).
    #[serde(default = "yes")]
    pub inside: bool,
}

fn yes() -> bool {
    true
}

/// The user's edits to the protected paths (stored in the settings).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protection {
    /// Built-in entries switched off (as shown in the list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
    /// Extra files and folders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ProtectedPath>,
}

static USER: RwLock<Option<Protection>> = RwLock::new(None);

/// Apply the user's protection list (called when settings load or change).
pub fn set_protection(p: &Protection) {
    if let Ok(mut g) = USER.write() {
        *g = Some(p.clone());
    }
}

fn protection() -> Protection {
    USER.read().ok().and_then(|g| g.clone()).unwrap_or_default()
}

/// Expand `%NAME%` environment variables (unknown ones are kept as typed).
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::SystemDir(p) => tr!("safety.system_dir", dir = p.display()),
            Refusal::Root => tr!("safety.root"),
            Refusal::Home(p) => tr!("safety.home", dir = p.display()),
            Refusal::OwnExe => tr!("safety.own_exe"),
            Refusal::User(p) => tr!("safety.user", path = p.display()),
        }
    }
}

/// A set of rules, normally from [`system_rules`]; explicit for tests.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// Refuse the directory itself and everything under it.
    pub inside: Vec<PathBuf>,
    /// Refuse exactly this path (not its children).
    pub exact: Vec<PathBuf>,
    /// The user's own entries: `(path, inside)`.
    pub user: Vec<(PathBuf, bool)>,
    /// freewipe's own executable.
    pub exe: Option<PathBuf>,
    /// Compare case-insensitively (Windows).
    pub case_insensitive: bool,
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The rules in effect: the built-in list minus what the user switched off,
/// plus the user's own entries.
pub fn system_rules() -> Rules {
    let mut r = builtin_rules();
    let prot = protection();
    let ci = r.case_insensitive;
    let off = |p: &PathBuf| prot.disabled.iter().any(|d| same(Path::new(d), p, ci));
    r.inside.retain(|p| !off(p));
    r.exact.retain(|p| !off(p));
    r.user = prot
        .extra
        .iter()
        .filter(|e| !e.path.trim().is_empty())
        .map(|e| (PathBuf::from(expand_env(e.path.trim())), e.inside))
        .collect();
    r
}

fn same(a: &Path, b: &Path, ci: bool) -> bool {
    norm(a, ci) == norm(b, ci)
}

/// The built-in entries as shown in Settings → Safety (`inside` = with
/// everything below).
pub fn builtin_entries() -> Vec<ProtectedPath> {
    let r = builtin_rules();
    let mut v: Vec<ProtectedPath> =
        r.inside.iter().map(|p| ProtectedPath { path: p.display().to_string(), inside: true }).collect();
    v.extend(r.exact.iter().map(|p| ProtectedPath { path: p.display().to_string(), inside: false }));
    v
}

/// The built-in rules for the machine we run on (duplicates removed).
pub fn builtin_rules() -> Rules {
    let mut r = Rules { exe: std::env::current_exe().ok(), case_insensitive: cfg!(windows), ..Default::default() };
    if cfg!(windows) {
        for v in ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"] {
            r.inside.extend(env_path(v));
        }
        if let Some(sd) = env_path("SystemDrive") {
            let root = PathBuf::from(format!("{}\\", sd.display()));
            r.inside.push(root.join("System Volume Information"));
            r.inside.push(root.join("Recovery"));
            r.exact.push(root.join("Users"));
        }
        r.exact.extend(env_path("USERPROFILE"));
    } else {
        for d in [
            "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/run", "/sbin", "/snap", "/sys",
            "/usr", "/var",
        ] {
            r.inside.push(PathBuf::from(d));
        }
        r.exact.push(PathBuf::from("/home"));
        r.exact.push(PathBuf::from("/root"));
        r.exact.extend(env_path("HOME"));
    }
    let ci = r.case_insensitive;
    let dedup = |v: &mut Vec<PathBuf>| {
        let mut seen: Vec<PathBuf> = Vec::new();
        v.retain(|p| {
            let n = norm(p, ci);
            if seen.contains(&n) {
                false
            } else {
                seen.push(n);
                true
            }
        });
    };
    dedup(&mut r.inside);
    dedup(&mut r.exact);
    r
}

/// Canonical, comparable form of a path.
fn norm(p: &Path, ci: bool) -> PathBuf {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = strip_verbatim(&abs.to_string_lossy());
    let s = s.trim_end_matches(['\\', '/']).to_string();
    let s = if s.is_empty() || s.ends_with(':') { format!("{s}{}", std::path::MAIN_SEPARATOR) } else { s };
    PathBuf::from(if ci { s.to_lowercase() } else { s })
}

/// `true` if `p` is a drive or file-system root (`C:\`, `/`, `\\srv\share\`).
fn is_root(p: &Path) -> bool {
    p.parent().is_none()
}

/// Check `target` against `rules`. `None` means allowed.
pub fn check_with(target: &Path, rules: &Rules) -> Option<Refusal> {
    let ci = rules.case_insensitive;
    let t = norm(target, ci);
    if is_root(&t) {
        return Some(Refusal::Root);
    }
    if let Some(exe) = &rules.exe {
        let e = norm(exe, ci);
        if t == e || e.starts_with(&t) {
            return Some(Refusal::OwnExe);
        }
    }
    for d in &rules.inside {
        let d = norm(d, ci);
        if t.starts_with(&d) {
            return Some(Refusal::SystemDir(d));
        }
    }
    for d in &rules.exact {
        let d = norm(d, ci);
        if t == d {
            return Some(Refusal::Home(d));
        }
    }
    for (d, inside) in &rules.user {
        let n = norm(d, ci);
        // A protected path also protects the folders that contain it (wiping
        // the parent would wipe it too).
        if t == n || (*inside && t.starts_with(&n)) || n.starts_with(&t) {
            return Some(Refusal::User(d.clone()));
        }
    }
    None
}

/// Check against the system rules; `force` disables the check entirely.
pub fn check(target: &Path, force: bool) -> Result<(), Refusal> {
    if force {
        return Ok(());
    }
    match check_with(target, &system_rules()) {
        Some(r) => {
            crate::dlog!("protected: {} refused ({r:?})", target.display());
            Err(r)
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            inside: vec![PathBuf::from("/sys_dir")],
            exact: vec![PathBuf::from("/home_dir")],
            exe: Some(PathBuf::from("/apps/freewipe/freewipe")),
            case_insensitive: false,
            user: vec![(PathBuf::from("/data/keep"), true), (PathBuf::from("/data/one.txt"), false)],
        }
    }

    #[test]
    fn user_rules() {
        let r = rules();
        assert!(matches!(check_with(Path::new("/data/keep/x/y.txt"), &r), Some(Refusal::User(_))));
        assert!(matches!(check_with(Path::new("/data/one.txt"), &r), Some(Refusal::User(_))));
        // The folder holding a protected item is protected too.
        assert!(matches!(check_with(Path::new("/data"), &r), Some(Refusal::User(_))));
        assert_eq!(check_with(Path::new("/data/other.txt"), &r), None);
        std::env::set_var("FREEWIPE_TEST_VAR", "/x");
        assert_eq!(expand_env("%FREEWIPE_TEST_VAR%/a %UNKNOWN_FREEWIPE% 5%"), "/x/a %UNKNOWN_FREEWIPE% 5%");
    }

    #[test]
    fn refuses_protected() {
        let r = rules();
        assert!(matches!(check_with(Path::new("/"), &r), Some(Refusal::Root)));
        assert!(matches!(check_with(Path::new("/sys_dir"), &r), Some(Refusal::SystemDir(_))));
        assert!(matches!(check_with(Path::new("/sys_dir/a/b.txt"), &r), Some(Refusal::SystemDir(_))));
        assert!(matches!(check_with(Path::new("/home_dir"), &r), Some(Refusal::Home(_))));
        assert_eq!(check_with(Path::new("/apps/freewipe/freewipe"), &r), Some(Refusal::OwnExe));
        assert_eq!(check_with(Path::new("/apps"), &r), Some(Refusal::OwnExe));
    }

    #[test]
    fn allows_ordinary() {
        let r = rules();
        assert_eq!(check_with(Path::new("/home_dir/notes.txt"), &r), None);
        assert_eq!(check_with(Path::new("/sys_dir_other/x"), &r), None);
        assert_eq!(check_with(Path::new("/apps/other"), &r), None);
    }

    #[test]
    fn force_skips_checks() {
        assert!(check(Path::new("/"), true).is_ok());
    }
}
