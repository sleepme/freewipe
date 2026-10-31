//! JSON reports of what was done (`--log <file>` in the CLI, automatic in
//! the GUI). Reports never contain data, only names, sizes and outcomes —
//! and with `redact` not even names.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::util::rfc3339;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    /// Finished with some per-file errors (directories, recycle bin).
    Partial,
    Error,
    Cancelled,
    /// Refused by the safety rules or not found.
    Skipped,
    /// `--dry-run`: nothing was written.
    Planned,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportItem {
    pub target: String,
    /// `file`, `dir`, `freespace`, `recycle_bin`.
    pub kind: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub bytes_overwritten: u64,
    pub passes: usize,
    pub verified: bool,
    pub deleted: bool,
    pub files_ok: u64,
    pub files_err: u64,
    pub streams: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    pub seconds: f64,
    /// At least one failure was "access denied" (may succeed as administrator).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub access_denied: bool,
    /// Per-file failures inside a directory (capped).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failures: Vec<String>,
    /// Files that were in use by another program.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub locked: Vec<String>,
    /// Free space: files whose cluster tip (slack) was overwritten.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub tips_files: u64,
    #[serde(skip_serializing_if = "is_zero", default)]
    pub tips_bytes: u64,
    /// Free space: NTFS file records overwritten.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub records: u64,
    /// The target was a symbolic link / junction: only the link was removed.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub link: bool,
    /// Move: where the item was moved to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub moved_to: Option<String>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl ReportItem {
    pub fn new(target: &Path, kind: &str) -> Self {
        ReportItem {
            target: target.display().to_string(),
            kind: kind.to_string(),
            status: Status::Ok,
            error: None,
            bytes_overwritten: 0,
            passes: 0,
            verified: false,
            deleted: false,
            files_ok: 0,
            files_err: 0,
            streams: 0,
            filesystem: None,
            media: None,
            seconds: 0.0,
            access_denied: false,
            failures: Vec::new(),
            locked: Vec::new(),
            tips_files: 0,
            tips_bytes: 0,
            records: 0,
            moved_to: None,
            link: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub tool: String,
    pub version: String,
    pub started: String,
    pub finished: String,
    pub method: String,
    pub method_name: String,
    pub passes: usize,
    pub verify: bool,
    pub keep: bool,
    pub dry_run: bool,
    pub items: Vec<ReportItem>,
    pub summary: Summary,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub ok: usize,
    pub partial: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub skipped: usize,
    pub bytes_overwritten: u64,
}

impl Report {
    pub fn new(method: crate::Method, passes: usize, verify: bool, keep: bool, dry_run: bool) -> Self {
        let now = rfc3339(SystemTime::now());
        Report {
            tool: crate::app::name().into(),
            version: crate::app::version().into(),
            started: now.clone(),
            finished: now,
            method: method.key().into(),
            method_name: method.name().into(),
            passes,
            verify,
            keep,
            dry_run,
            items: Vec::new(),
            summary: Summary::default(),
        }
    }

    pub fn push(&mut self, item: ReportItem) {
        self.items.push(item);
    }

    /// Stamp the finish time and compute the summary.
    pub fn finish(&mut self) {
        self.finished = rfc3339(SystemTime::now());
        let mut s = Summary::default();
        for i in &self.items {
            match i.status {
                Status::Ok | Status::Planned => s.ok += 1,
                Status::Partial => s.partial += 1,
                Status::Error => s.failed += 1,
                Status::Cancelled => s.cancelled += 1,
                Status::Skipped => s.skipped += 1,
            }
            s.bytes_overwritten += i.bytes_overwritten;
        }
        self.summary = s;
    }

    /// Replace target names with `item-N` (privacy option).
    pub fn redact(&mut self) {
        for (n, i) in self.items.iter_mut().enumerate() {
            i.target = format!("item-{}", n + 1);
            i.failures = i.failures.iter().map(|f| f.split(" — ").nth(1).unwrap_or("").to_string()).collect();
            i.locked = (1..=i.locked.len()).map(|k| format!("file-{k}")).collect();
            if i.moved_to.is_some() {
                i.moved_to = Some("folder".into());
            }
            // Error texts quote paths too.
            if let Some(e) = &i.error {
                if e.contains(':') || e.contains('/') || e.contains('\\') {
                    i.error = Some(e.rsplit(": ").next().unwrap_or("").to_string());
                }
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Write to `path`, creating parent folders.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let r = std::fs::write(path, self.to_json());
        crate::dlog!("report saved: {} ({:?})", path.display(), r.as_ref().err());
        r
    }

    /// `<dir>/freewipe-YYYYMMDD-HHMMSS.json`.
    pub fn default_path(dir: &Path) -> PathBuf {
        dir.join(format!("{}-{}.json", crate::app::name(), crate::util::file_stamp(SystemTime::now())))
    }
}
