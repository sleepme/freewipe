//! Launch requests: parsed from the command line, or passed from a second
//! instance to the first one through small JSON files in the inbox folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use freewipe::settings::inbox_dir;

/// Name of the mutex that marks the running window (per user session).
#[cfg(windows)]
pub const MUTEX_NAME: &str = "Local\\freewipe-gui-single-instance";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Just bring the window to the front.
    #[default]
    Focus,
    Enqueue,
    Wipe,
    FreeSpace,
    RecycleBin,
    /// Ask for a folder, move the items there, wipe the originals.
    Move,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Request {
    pub action: Action,
    pub method: Option<String>,
    pub paths: Vec<PathBuf>,
    /// The user already confirmed (relaunch as administrator).
    #[serde(default)]
    pub confirmed: bool,
    #[serde(skip)]
    pub lang: Option<String>,
    /// This process was started elevated by freewipe itself.
    #[serde(skip)]
    pub elevated: bool,
    /// One-time token proving the password was already entered.
    #[serde(skip)]
    pub unlock_token: Option<String>,
    /// The request already holds the whole selection (DropTarget launch):
    /// no need to wait for sibling processes.
    #[serde(default)]
    pub batched: bool,
    /// `--pending`: run the wipe scheduled for this sign-in.
    #[serde(skip)]
    pub pending: bool,
    /// Complete targets (`--targets <json file>`): keeps folder filters and
    /// move destinations, e.g. for "Retry as administrator".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<freewipe::plan::Target>,
    /// `--run-schedule <id>`: started by the Task Scheduler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
}

impl Request {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn focus() -> Self {
        Request::default()
    }

    pub fn is_empty(&self) -> bool {
        self.schedule.is_none()
            && self.targets.is_empty()
            && (self.action == Action::Focus || (self.paths.is_empty() && self.action != Action::RecycleBin))
    }

    pub fn from_args(args: impl Iterator<Item = OsString>) -> Self {
        let mut r = Request::default();
        let mut it = args.peekable();
        let mut explicit = false;
        while let Some(a) = it.next() {
            match a.to_string_lossy().as_ref() {
                "--enqueue" => {
                    r.action = Action::Enqueue;
                    explicit = true;
                }
                "--wipe" => {
                    r.action = Action::Wipe;
                    explicit = true;
                }
                "--freespace" => {
                    r.action = Action::FreeSpace;
                    explicit = true;
                }
                "--recycle-bin" => {
                    r.action = Action::RecycleBin;
                    explicit = true;
                }
                "--move" => {
                    r.action = Action::Move;
                    explicit = true;
                }
                // From the shell extension: the action number and a file
                // with the selected paths (one per line).
                "--shell" => {
                    let n = it.next().and_then(|v| v.to_string_lossy().parse::<u32>().ok());
                    if let Some(a) = n.and_then(freewipe::shell::DropAction::from_index) {
                        let sub = Request::from_args(a.args().into_iter().map(OsString::from));
                        r.action = sub.action;
                        r.method = sub.method.or(r.method.take());
                        r.batched = true;
                        explicit = true;
                    }
                }
                "--list" => {
                    if let Some(f) = it.next() {
                        r.paths.extend(read_list(Path::new(&f)));
                    }
                }
                "--targets" => {
                    if let Some(f) = it.next() {
                        let p = PathBuf::from(&f);
                        if let Some(v) = std::fs::read(&p).ok().and_then(|d| serde_json::from_slice(&d).ok()) {
                            r.targets = v;
                        }
                        if p.file_name().map(|n| n.to_string_lossy().starts_with("freewipe-targets-")).unwrap_or(false)
                        {
                            let _ = std::fs::remove_file(&p);
                        }
                        r.action = Action::Wipe;
                        explicit = true;
                    }
                }
                "--run-schedule" => {
                    r.schedule = it.next().map(|m| m.to_string_lossy().to_string());
                    explicit = true;
                }
                "--confirmed" => r.confirmed = true,
                "--pending" => {
                    r.pending = true;
                    explicit = true;
                }
                "--elevated" => r.elevated = true,
                "--unlock-token" => r.unlock_token = it.next().map(|m| m.to_string_lossy().to_string()),
                "--method" => r.method = it.next().map(|m| m.to_string_lossy().to_string()),
                "--lang" => r.lang = it.next().map(|m| m.to_string_lossy().to_string()),
                _ => r.paths.push(clean_path(a)),
            }
        }
        // Plain `freewipe-gui file.txt` (e.g. a file dropped on the exe) = enqueue.
        if !explicit && !r.paths.is_empty() {
            r.action = Action::Enqueue;
        }
        r
    }
}

impl Request {
    /// Is this a request that wipes something (not just enqueue/focus)?
    pub fn is_wipe(&self) -> bool {
        matches!(self.action, Action::Wipe | Action::FreeSpace | Action::RecycleBin | Action::Move) && !self.is_empty()
    }

    /// Command-line arguments that reproduce this request.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn to_args(&self) -> Vec<String> {
        let mut v = Vec::new();
        match self.action {
            Action::Focus => {}
            Action::Enqueue => v.push("--enqueue".into()),
            Action::Wipe => v.push("--wipe".into()),
            Action::FreeSpace => v.push("--freespace".into()),
            Action::RecycleBin => v.push("--recycle-bin".into()),
            Action::Move => v.push("--move".into()),
        }
        if let Some(m) = &self.method {
            v.push("--method".into());
            v.push(m.clone());
        }
        if self.confirmed {
            v.push("--confirmed".into());
        }
        if !self.targets.is_empty() {
            let f = std::env::temp_dir().join(format!(
                "freewipe-targets-{}-{}.json",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
            ));
            if std::fs::write(&f, serde_json::to_vec(&self.targets).unwrap_or_default()).is_ok() {
                v.push("--targets".into());
                v.push(f.display().to_string());
            }
        }
        v.extend(self.paths.iter().map(|p| p.display().to_string()));
        v
    }
}

/// Write a one-time unlock token (valid 2 minutes) and return its value.
/// Used so an elevated relaunch does not ask for the password again.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn issue_unlock_token() -> Option<String> {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let dir = freewipe::settings::data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(format!("unlock-{token}.token")), b"1").ok()?;
    Some(token)
}

/// Accept and delete a token issued by [`issue_unlock_token`].
pub fn consume_unlock_token(token: &str) -> bool {
    if token.len() != 32 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let p = freewipe::settings::data_dir().join(format!("unlock-{token}.token"));
    let fresh = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age < std::time::Duration::from_secs(120))
        .unwrap_or(false);
    let _ = std::fs::remove_file(&p);
    fresh
}

/// Paths from a list file written by the shell extension (deleted after
/// reading; only files in the temp folder named `freewipe-sel-*` are deleted).
fn read_list(f: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(f).unwrap_or_default();
    let ours = f.file_name().map(|n| n.to_string_lossy().starts_with("freewipe-sel-")).unwrap_or(false);
    if ours {
        let _ = std::fs::remove_file(f);
    }
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| clean_path(l.into())).collect()
}

fn pending_file() -> PathBuf {
    freewipe::settings::data_dir().join("pending.json")
}

/// Remember `paths` to be wiped at the next sign-in (merged with any
/// already scheduled) and register the one-time start with Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn schedule_pending(targets: &[freewipe::plan::Target], method: &str) -> std::io::Result<()> {
    let mut r = take_pending().unwrap_or_default();
    r.action = Action::Wipe;
    r.method = Some(method.to_string());
    r.confirmed = true;
    for t in targets {
        if !r.targets.contains(t) {
            r.targets.push(t.clone());
        }
    }
    let f = pending_file();
    std::fs::create_dir_all(f.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&f, serde_json::to_vec_pretty(&r).unwrap_or_default())?;
    #[cfg(windows)]
    {
        let exe = std::env::current_exe()?;
        let key = freewipe::sys::reg_create(
            freewipe::sys::RegRoot::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
        )?;
        key.set_string(Some("freewipe-pending"), &format!("\"{}\" --pending", exe.display()))?;
    }
    Ok(())
}

/// Take (and delete) the wipe scheduled for this sign-in.
pub fn take_pending() -> Option<Request> {
    let f = pending_file();
    let data = std::fs::read(&f).ok()?;
    let _ = std::fs::remove_file(&f);
    let r: Request = serde_json::from_slice(&data).ok()?;
    (!r.paths.is_empty() || !r.targets.is_empty()).then_some(r)
}

/// Undo the classic `"D:\"` quoting accident (`D:"`) and stray quotes.
fn clean_path(a: OsString) -> PathBuf {
    let s = a.to_string_lossy();
    let t = s.trim().trim_matches('"');
    if t.len() == 2 && t.ends_with(':') {
        return PathBuf::from(format!("{t}\\"));
    }
    PathBuf::from(t)
}

/// Drop a request for the running instance.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn post(r: &Request) -> std::io::Result<()> {
    let dir = inbox_dir();
    std::fs::create_dir_all(&dir)?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!("{nanos}-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(r).unwrap_or_default())?;
    // Rename makes the file appear atomically for the reader.
    std::fs::rename(&tmp, tmp.with_extension("json"))
}

/// Wake the UI when a request file appears (checked every 250 ms by a
/// small thread, so the window itself does not have to keep repainting).
pub fn watch(ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let dir = inbox_dir();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let any = std::fs::read_dir(&dir)
                .map(|rd| rd.flatten().any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json")))
                .unwrap_or(false);
            if any {
                ctx.request_repaint();
            }
        }
    });
}

/// Collect (and delete) pending requests, oldest first.
pub fn drain() -> Vec<Request> {
    let Ok(rd) = std::fs::read_dir(inbox_dir()) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let data = std::fs::read(&p).ok();
            let _ = std::fs::remove_file(&p);
            data.and_then(|d| serde_json::from_slice(&d).ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Request {
        Request::from_args(v.iter().map(OsString::from))
    }

    #[test]
    fn parses_context_menu_commands() {
        let r = args(&["--wipe", "--method", "gutmann", "C:\\a b\\x.txt"]);
        assert_eq!(r.action, Action::Wipe);
        assert_eq!(r.method.as_deref(), Some("gutmann"));
        assert_eq!(r.paths, vec![PathBuf::from("C:\\a b\\x.txt")]);

        let r = args(&["--freespace", "D:\""]);
        assert_eq!(r.action, Action::FreeSpace);
        assert_eq!(r.paths, vec![PathBuf::from("D:\\")]);

        assert_eq!(args(&["x.txt"]).action, Action::Enqueue);
        assert!(args(&[]).is_empty());
        assert!(!args(&["--recycle-bin"]).is_empty());
        assert!(args(&["--pending"]).pending);
        let list = std::env::temp_dir().join(format!("freewipe-sel-test-{}.txt", std::process::id()));
        std::fs::write(&list, "C:\\a b\\x.txt\r\nD:\\\r\n").unwrap();
        let r = args(&["--shell", "16", "--list", &list.display().to_string()]);
        assert_eq!(r.action, Action::Wipe);
        assert_eq!(r.method.as_deref(), Some("zero"));
        assert_eq!(r.paths, vec![PathBuf::from("C:\\a b\\x.txt"), PathBuf::from("D:\\")]);
        assert!(r.batched && !list.exists());
        assert_eq!(args(&["--shell", "3", "x"]).action, Action::Move);
        assert_eq!(args(&["--run-schedule", "ab12"]).schedule.as_deref(), Some("ab12"));
    }
}
