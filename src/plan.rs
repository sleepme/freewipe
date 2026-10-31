//! Targets and pre-flight scanning.
//!
//! Before anything is written, every target is scanned: does it exist, is it
//! allowed by the safety rules, how many bytes and files are involved, which
//! file system and medium does it live on. The same scan drives `--dry-run`,
//! the confirmation prompt, the GUI queue and progress-bar totals.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::filesystem::{self, VolumeInfo};
use crate::media::{self, MediaInfo};
use crate::safety;
use crate::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    File,
    Dir,
    FreeSpace,
    RecycleBin,
    /// Move a file or folder to `Target::dest`, then wipe the original.
    Move,
}

impl TargetKind {
    pub fn key(&self) -> &'static str {
        match self {
            TargetKind::File => "file",
            TargetKind::Dir => "dir",
            TargetKind::FreeSpace => "freespace",
            TargetKind::RecycleBin => "recycle_bin",
            TargetKind::Move => "move",
        }
    }
    pub fn label(&self) -> String {
        match self {
            TargetKind::File => tr!("kind.file"),
            TargetKind::Dir => tr!("kind.dir"),
            TargetKind::FreeSpace => tr!("kind.freespace"),
            TargetKind::RecycleBin => tr!("kind.recycle_bin"),
            TargetKind::Move => tr!("kind.move"),
        }
    }
}

/// Which files of a folder are wiped.
///
/// Masks use `*` and `?`, are case-insensitive and are separated by `;`,
/// `,` or spaces (`*.tmp; *.log`). An empty include list means "all files".
/// Exclude masks also skip whole sub-folders whose name matches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirFilter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Keep the folder itself (and, with masks, every folder): wipe contents only.
    #[serde(default)]
    pub contents_only: bool,
}

/// `*`/`?` wildcard match, ASCII case-insensitive.
pub fn mask_match(mask: &str, name: &str) -> bool {
    let m: Vec<char> = mask.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut mi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if mi < m.len() && (m[mi] == '?' || m[mi] == n[ni]) {
            mi += 1;
            ni += 1;
        } else if mi < m.len() && m[mi] == '*' {
            star = Some(mi);
            mark = ni;
            mi += 1;
        } else if let Some(s) = star {
            mi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while mi < m.len() && m[mi] == '*' {
        mi += 1;
    }
    mi == m.len()
}

impl DirFilter {
    /// Split a user-typed mask list.
    pub fn parse_masks(s: &str) -> Vec<String> {
        s.split([';', ',', ' ', '\t', '\n']).map(str::trim).filter(|m| !m.is_empty()).map(str::to_string).collect()
    }

    /// Masks narrow the selection (folders may stay behind).
    pub fn has_masks(&self) -> bool {
        !self.include.is_empty() || !self.exclude.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        !self.has_masks() && !self.contents_only
    }

    /// Is a file with this name wiped?
    pub fn wants_file(&self, name: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|m| mask_match(m, name)))
            && !self.exclude.iter().any(|m| mask_match(m, name))
    }

    /// Is a sub-folder with this name entered?
    pub fn wants_dir(&self, name: &str) -> bool {
        !self.exclude.iter().any(|m| mask_match(m, name))
    }

    /// Short description for lists: `*.tmp; *.log − *.keep`.
    pub fn summary(&self) -> String {
        let mut s = if self.include.is_empty() { "*".to_string() } else { self.include.join("; ") };
        if !self.exclude.is_empty() {
            s.push_str(" − ");
            s.push_str(&self.exclude.join("; "));
        }
        if self.contents_only {
            s.push_str(&format!(" · {}", tr!("filter.contents_only")));
        }
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub kind: TargetKind,
    pub path: PathBuf,
    /// Folders: which files are wiped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<DirFilter>,
    /// Move: destination folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<PathBuf>,
}

impl Target {
    fn new(kind: TargetKind, path: PathBuf) -> Self {
        Target { kind, path, filter: None, dest: None }
    }
    pub fn file(p: impl Into<PathBuf>) -> Self {
        Target::new(TargetKind::File, p.into())
    }
    pub fn dir(p: impl Into<PathBuf>) -> Self {
        Target::new(TargetKind::Dir, p.into())
    }
    /// A folder with a filter (an empty filter is dropped).
    pub fn dir_filtered(p: impl Into<PathBuf>, filter: DirFilter) -> Self {
        Target { filter: (!filter.is_empty()).then_some(filter), ..Target::dir(p) }
    }
    pub fn free_space(p: impl Into<PathBuf>) -> Self {
        Target::new(TargetKind::FreeSpace, p.into())
    }
    pub fn recycle_bin() -> Self {
        Target::new(TargetKind::RecycleBin, PathBuf::new())
    }
    /// Move `p` into the folder `dest`, then wipe `p`.
    pub fn move_to(p: impl Into<PathBuf>, dest: impl Into<PathBuf>) -> Self {
        Target { dest: Some(dest.into()), ..Target::new(TargetKind::Move, p.into()) }
    }
    /// File or directory, decided by what is on disk.
    pub fn auto(p: impl Into<PathBuf>) -> Self {
        let p = p.into();
        let is_dir = fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            Target::dir(p)
        } else {
            Target::file(p)
        }
    }
    /// Name for lists: path, or "Recycle Bin".
    pub fn display(&self) -> String {
        match self.kind {
            TargetKind::RecycleBin => tr!("kind.recycle_bin"),
            TargetKind::Move => format!(
                "{} → {}",
                self.path.display(),
                self.dest.as_deref().map(|d| d.display().to_string()).unwrap_or_default()
            ),
            _ => self.path.display().to_string(),
        }
    }
}

/// Result of scanning one target.
#[derive(Clone, Debug)]
pub struct PlanItem {
    pub target: Target,
    /// Data bytes to overwrite (free bytes for free space).
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub volume: Option<VolumeInfo>,
    pub media: Option<MediaInfo>,
    /// Safety refusal or "not found"; the item will be skipped.
    pub blocked: Option<String>,
    /// A symbolic link / junction (only the link is removed).
    pub link: bool,
    /// A Windows shortcut (`.lnk`): only the shortcut file is wiped.
    pub shortcut: bool,
}

impl PlanItem {
    /// Extra line for lists ("only the link is removed", …).
    pub fn note(&self) -> Option<String> {
        if self.link {
            Some(tr!("plan.link"))
        } else if self.shortcut {
            Some(tr!("plan.shortcut"))
        } else {
            None
        }
    }
}

impl PlanItem {
    pub fn is_ok(&self) -> bool {
        self.blocked.is_none()
    }
    /// Total bytes the engine will write (+ read back when verifying).
    pub fn work_bytes(&self, passes: usize, verify: bool) -> u64 {
        // A move also writes the copy and reads it back.
        let extra = if self.target.kind == TargetKind::Move { 2 } else { 0 };
        self.bytes.saturating_mul(passes as u64 + u64::from(verify) + extra)
    }
    /// All file-system and medium warnings for this item.
    pub fn warnings(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(m) = self.media.as_ref().and_then(|m| m.warning()) {
            v.push(m);
        }
        if let Some(vol) = &self.volume {
            v.extend(vol.fs.warnings());
        }
        v
    }
}

/// Folders holding the current user's deleted items.
pub fn recycle_bin_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let Ok(sid) = crate::sys::current_user_sid() else { return Vec::new() };
        crate::sys::list_volumes()
            .into_iter()
            .map(|root| root.join("$Recycle.Bin").join(&sid))
            .filter(|p| p.is_dir())
            .collect()
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        base.map(|b| vec![b.join("Trash/files"), b.join("Trash/info")])
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.is_dir())
            .collect()
    }
}

/// Recursively count files and bytes without following links.
pub fn tree_size(p: &Path) -> (u64, u64, u64) {
    tree_size_filtered(p, None)
}

/// Like [`tree_size`], counting only what `filter` selects.
pub fn tree_size_filtered(p: &Path, filter: Option<&DirFilter>) -> (u64, u64, u64) {
    let (mut bytes, mut files, mut dirs) = (0u64, 0u64, 0u64);
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        dirs += 1;
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_symlink() {
                continue;
            } else if ft.is_dir() {
                if filter.map(|f| f.wants_dir(&name)).unwrap_or(true) {
                    stack.push(e.path());
                }
            } else if filter.map(|f| f.wants_file(&name)).unwrap_or(true) {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (bytes, files, dirs)
}

/// Scan one target. Never fails: problems end up in `blocked`.
pub fn scan(target: &Target, force: bool) -> PlanItem {
    let mut item = PlanItem {
        target: target.clone(),
        bytes: 0,
        files: 0,
        dirs: 0,
        volume: None,
        media: None,
        blocked: None,
        link: false,
        shortcut: false,
    };

    match target.kind {
        TargetKind::RecycleBin => {
            for d in recycle_bin_dirs() {
                let (b, f, _) = tree_size(&d);
                item.bytes += b;
                item.files += f;
                if item.volume.is_none() {
                    item.volume = filesystem::probe(&d).ok();
                    item.media = Some(media::probe(&d));
                }
            }
            return item;
        }
        TargetKind::FreeSpace => {
            if !target.path.exists() {
                item.blocked = Some(tr!("err.not_found", path = target.path.display()));
                return item;
            }
            item.volume = filesystem::probe(&target.path).ok();
            item.media = Some(media::probe(&target.path));
            item.bytes = item.volume.as_ref().map(|v| v.free).unwrap_or(0);
            return item;
        }
        TargetKind::File | TargetKind::Dir | TargetKind::Move => {}
    }

    let meta = match fs::symlink_metadata(&target.path) {
        Ok(m) => m,
        Err(_) => {
            item.blocked = Some(tr!("err.not_found", path = target.path.display()));
            return item;
        }
    };
    if let Err(r) = safety::check(&target.path, force) {
        item.blocked = Some(r.message());
        return item;
    }
    item.link = meta.file_type().is_symlink();
    item.shortcut =
        !item.link && meta.is_file() && target.path.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false);
    if item.link {
        // Nothing is overwritten: the link itself is removed.
        item.files = 1;
        item.volume = filesystem::probe(target.path.parent().unwrap_or(&target.path)).ok();
        return item;
    }
    match target.kind {
        TargetKind::Dir if !meta.is_dir() => item.blocked = Some(tr!("err.not_dir", path = target.path.display())),
        TargetKind::File if meta.is_dir() => item.blocked = Some(tr!("err.is_dir", path = target.path.display())),
        _ => {}
    }
    if target.kind == TargetKind::Move {
        if let Some(why) = move_problem(&target.path, target.dest.as_deref()) {
            item.blocked = Some(why);
            return item;
        }
    }
    if meta.is_dir() {
        let filter = if target.kind == TargetKind::Dir { target.filter.as_ref() } else { None };
        let (b, f, d) = tree_size_filtered(&target.path, filter);
        (item.bytes, item.files, item.dirs) = (b, f, d);
    } else {
        item.bytes = meta.len();
        item.files = 1;
    }
    item.volume = filesystem::probe(&target.path).ok();
    item.media = Some(media::probe(&target.path));
    item
}

/// Why `src` cannot be moved into `dest` (or `None` if it can).
pub fn move_problem(src: &Path, dest: Option<&Path>) -> Option<String> {
    let Some(dest) = dest else { return Some(tr!("err.move_no_dest")) };
    if !dest.is_dir() {
        return Some(tr!("err.not_found", path = dest.display()));
    }
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (s, d) = (canon(src), canon(dest));
    if d.starts_with(&s) || s.parent() == Some(d.as_path()) {
        return Some(tr!("err.move_same", path = dest.display()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert!(mask_match("*.tmp", "A.TMP"));
        assert!(mask_match("~$*", "~$doc.docx"));
        assert!(mask_match("log?.txt", "log1.txt"));
        assert!(!mask_match("log?.txt", "log10.txt"));
        assert!(mask_match("*", ""));
        assert!(!mask_match("*.tmp", "a.tmpx"));
        let f = DirFilter {
            include: DirFilter::parse_masks("*.tmp; *.log,*.bak"),
            exclude: DirFilter::parse_masks("keep*"),
            contents_only: false,
        };
        assert_eq!(f.include.len(), 3);
        assert!(f.wants_file("x.log"));
        assert!(!f.wants_file("keep.log"));
        assert!(!f.wants_file("x.txt"));
        assert!(!f.wants_dir("keepers"));
        assert!(DirFilter::default().wants_file("anything"));
    }

    #[test]
    fn filtered_size() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.tmp"), [0u8; 10]).unwrap();
        std::fs::write(d.path().join("b.txt"), [0u8; 20]).unwrap();
        std::fs::create_dir(d.path().join("keep")).unwrap();
        std::fs::write(d.path().join("keep").join("c.tmp"), [0u8; 40]).unwrap();
        let f = DirFilter { include: vec!["*.tmp".into()], exclude: vec!["keep".into()], contents_only: true };
        assert_eq!(tree_size_filtered(d.path(), Some(&f)).0, 10);
        assert_eq!(tree_size(d.path()).0, 70);
    }
}
