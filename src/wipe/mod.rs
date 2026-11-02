//! Wiping orchestration.
//!
//! Sequencing for a file: every stream × every pass (flush after each) →
//! verify the last pass → rename N times → truncate to 0 → delete.
//! Directories apply that to every file, then remove folders bottom-up under
//! random names. Free space is filled with chunk files that get the same
//! passes and are then removed. All of it reports through
//! [`crate::progress::Progress`] and honours the cancel flag between buffers.

pub mod dir;
pub mod file;
pub mod freespace;
pub mod moving;
pub mod recycle;
pub mod residue;
pub mod verify;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use dir::{wipe_dir, wipe_dir_contents, DirReport};
pub use file::{wipe_file, FileReport};
pub use freespace::{wipe_free_space, FreeSpaceReport};
pub use recycle::wipe_recycle_bin;

use crate::methods::Method;
use crate::plan::{Target, TargetKind};
use crate::progress::Ctx;
use crate::report::{ReportItem, Status};
use crate::tr;

/// Default I/O buffer: 4 MiB.
pub const DEFAULT_BUFFER: usize = 4 << 20;

/// How a wipe is performed.
#[derive(Clone, Debug)]
pub struct WipeOptions {
    pub method: Method,
    /// Pass count for `randomN` (ignored by fixed-length standards).
    pub passes_override: Option<usize>,
    /// Read back and verify the final pass.
    pub verify: bool,
    /// How many times a file/folder is renamed before deletion.
    pub rename_passes: u32,
    /// Overwrite only; do not rename, truncate or delete.
    pub keep: bool,
    /// Also wipe NTFS/ReFS alternate data streams.
    pub include_ads: bool,
    /// Write buffer size in bytes.
    pub buffer_size: usize,
    /// Bypass the protected-path rules.
    pub force: bool,
    /// Free space: leave this many bytes unfilled.
    pub reserve_bytes: u64,
    /// On SSD/flash use one pass (NIST SP 800-88 Clear) instead of `method`:
    /// more passes add no security there, only time and wear.
    pub flash_single_pass: bool,
    /// Free space: also overwrite the slack at the end of every file.
    pub cluster_tips: bool,
    /// Free space (NTFS): also overwrite free file records (old names and
    /// small files).
    pub file_records: bool,
    /// Folders: which files are wiped (taken from the target).
    pub filter: Option<crate::plan::DirFilter>,
}

impl Default for WipeOptions {
    fn default() -> Self {
        WipeOptions {
            method: Method::DodE,
            passes_override: None,
            verify: false,
            rename_passes: 3,
            keep: false,
            include_ads: true,
            buffer_size: DEFAULT_BUFFER,
            force: false,
            reserve_bytes: 0,
            flash_single_pass: false,
            cluster_tips: false,
            file_records: false,
            filter: None,
        }
    }
}

impl WipeOptions {
    pub fn pass_count(&self) -> usize {
        self.method.pass_count(self.passes_override)
    }

    /// The options actually used for a target on SSD/flash (`flash = true`)
    /// or other media: with `flash_single_pass`, flash gets one NIST Clear pass.
    pub fn effective_for(&self, flash: bool) -> WipeOptions {
        if self.single_pass_applies(flash) {
            WipeOptions { method: Method::NistClear, passes_override: None, ..self.clone() }
        } else {
            self.clone()
        }
    }

    /// Does the SSD/flash single-pass rule change the method for this medium?
    pub fn single_pass_applies(&self, flash: bool) -> bool {
        self.flash_single_pass && flash && self.method != Method::NistClear
    }

    /// Bytes the engine will write (and read back) for a planned target,
    /// taking the SSD/flash single-pass rule into account.
    pub fn work_bytes(&self, item: &crate::plan::PlanItem) -> u64 {
        let flash = item.media.as_ref().is_some_and(|m| m.is_flash_likely());
        let o = self.effective_for(flash);
        item.work_bytes(o.pass_count(), o.verify)
    }
}

/// Errors from the engine. Messages are localised.
#[derive(Debug)]
pub enum WipeError {
    Io { path: PathBuf, source: io::Error },
    VerifyMismatch { path: PathBuf, stream: String },
    Refused { path: PathBuf, reason: String },
    NotFound { path: PathBuf },
    Cancelled,
}

impl fmt::Display for WipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            WipeError::Io { path, source } => tr!("err.io", path = path.display(), err = source),
            WipeError::VerifyMismatch { path, stream } => {
                tr!("err.verify", path = path.display(), stream = stream)
            }
            WipeError::Refused { path, reason } => tr!("err.refused", path = path.display(), reason = reason),
            WipeError::NotFound { path } => tr!("err.not_found", path = path.display()),
            WipeError::Cancelled => tr!("err.cancelled"),
        };
        f.write_str(&s)
    }
}

impl std::error::Error for WipeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WipeError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl WipeError {
    /// "Access denied" — retrying as administrator may help.
    pub fn is_access_denied(&self) -> bool {
        match self {
            WipeError::Io { source, .. } => {
                source.kind() == io::ErrorKind::PermissionDenied || source.raw_os_error() == Some(5)
            }
            _ => false,
        }
    }

    /// The file is open in another program (sharing / lock violation).
    pub fn is_locked(&self) -> bool {
        matches!(self, WipeError::Io { source, .. } if crate::sys::is_locked(source))
    }

    pub fn io(path: &Path, source: io::Error) -> Self {
        if source.kind() == io::ErrorKind::NotFound {
            return WipeError::NotFound { path: path.to_path_buf() };
        }
        WipeError::Io { path: path.to_path_buf(), source }
    }
}

/// Extension to attach a path to an `io::Result`.
pub(crate) trait IoCtx<T> {
    fn at(self, path: &Path) -> Result<T, WipeError>;
}
impl<T> IoCtx<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T, WipeError> {
        self.map_err(|e| WipeError::io(path, e))
    }
}

/// Return `Err(Cancelled)` if cancellation was requested.
pub(crate) fn check_cancel(ctx: &Ctx) -> Result<(), WipeError> {
    if ctx.cancel.checkpoint() {
        Err(WipeError::Cancelled)
    } else {
        Ok(())
    }
}

/// Run one target end-to-end (safety check included) and describe the
/// outcome as a report item. Used by both the CLI and the GUI.
pub fn execute(target: &Target, opts: &WipeOptions, ctx: &mut Ctx) -> ReportItem {
    if !crate::debuglog::enabled() {
        return execute_inner(target, opts, ctx);
    }
    crate::dlog!(
        "target start: {} [{}] method={} passes={} verify={} keep={} ads={} force={} rename={} flash1={} tips={} records={} reserve={} filter={:?} dest={:?}",
        target.display(),
        target.kind.key(),
        opts.method.key(),
        opts.pass_count(),
        opts.verify,
        opts.keep,
        opts.include_ads,
        opts.force,
        opts.rename_passes,
        opts.flash_single_pass,
        opts.cluster_tips,
        opts.file_records,
        opts.reserve_bytes,
        target.filter,
        target.dest
    );
    struct Logged<'p>(&'p mut dyn crate::progress::Progress);
    impl crate::progress::Progress for Logged<'_> {
        fn item(&mut self, name: &str) {
            self.0.item(name)
        }
        fn pass(&mut self, index: usize, count: usize, label: &str) {
            self.0.pass(index, count, label)
        }
        fn stage(&mut self, stage: crate::progress::Stage) {
            self.0.stage(stage)
        }
        fn advance(&mut self, bytes: u64) {
            self.0.advance(bytes)
        }
        fn note(&mut self, msg: &str) {
            crate::dlog!("  note: {msg}");
            self.0.note(msg)
        }
    }
    let mut logged = Logged(&mut *ctx.progress);
    let mut inner = Ctx::new(ctx.cancel, &mut logged);
    let item = execute_inner(target, opts, &mut inner);
    crate::dlog!(
        "target done: {} → {:?} {:.2}s bytes={} files_ok={} files_err={} verified={} deleted={} link={} fs={:?} media={:?} error={:?}",
        target.display(),
        item.status,
        item.seconds,
        item.bytes_overwritten,
        item.files_ok,
        item.files_err,
        item.verified,
        item.deleted,
        item.link,
        item.filesystem,
        item.media,
        item.error
    );
    for f in item.failures.iter().take(50) {
        crate::dlog!("  failure: {f}");
    }
    if !item.locked.is_empty() {
        crate::dlog!("  in use: {:?}", item.locked);
    }
    item
}

fn execute_inner(target: &Target, opts: &WipeOptions, ctx: &mut Ctx) -> ReportItem {
    let started = Instant::now();
    let mut item = ReportItem::new(&target.path, target.kind.key());
    if target.kind == TargetKind::RecycleBin {
        item.target = target.display();
    }
    item.passes = opts.pass_count();

    if matches!(target.kind, TargetKind::File | TargetKind::Dir | TargetKind::Move) {
        if let Err(r) = crate::safety::check(&target.path, opts.force) {
            item.status = Status::Skipped;
            item.error = Some(r.message());
            return item;
        }
    }

    // A symbolic link or junction given directly: remove the link itself,
    // never what it points to.
    if matches!(target.kind, TargetKind::File | TargetKind::Dir | TargetKind::Move) {
        if let Ok(m) = std::fs::symlink_metadata(&target.path) {
            if m.file_type().is_symlink() {
                item.link = true;
                item.passes = 0;
                if target.kind == TargetKind::Move || opts.keep {
                    item.status = Status::Skipped;
                    item.error = Some(tr!("err.is_link", path = target.path.display()));
                } else if std::fs::remove_file(&target.path).or_else(|_| std::fs::remove_dir(&target.path)).is_ok() {
                    ctx.progress.note(&tr!("note.link_removed", path = target.path.display()));
                    item.deleted = true;
                    item.status = Status::Ok;
                } else {
                    item.status = Status::Error;
                    item.error = Some(tr!("err.is_link", path = target.path.display()));
                }
                item.seconds = started.elapsed().as_secs_f64();
                return item;
            }
        }
    }

    let probe_path = if target.kind == TargetKind::RecycleBin { None } else { Some(&target.path) };
    let mut flash = false;
    if let Some(p) = probe_path {
        item.filesystem = crate::filesystem::probe(p).ok().map(|v| v.fs.display());
        let m = crate::media::probe(p);
        flash = m.is_flash_likely();
        item.media = Some(m.label());
    }
    let filtered;
    let opts = if target.kind == TargetKind::Dir && target.filter.is_some() {
        filtered = WipeOptions { filter: target.filter.clone(), ..opts.clone() };
        &filtered
    } else {
        opts
    };
    let single;
    let opts = if opts.single_pass_applies(flash) {
        ctx.progress.note(&tr!("note.ssd_single", method = opts.method.name()));
        single = opts.effective_for(true);
        item.passes = 1;
        &single
    } else {
        opts
    };

    let result: Result<(), WipeError> = match target.kind {
        TargetKind::File => wipe_file(&target.path, opts, ctx).map(|r| {
            item.bytes_overwritten = r.bytes_overwritten;
            item.streams = r.streams as u64;
            item.verified = r.verified;
            item.deleted = r.deleted;
            item.files_ok = 1;
        }),
        TargetKind::Dir => match wipe_dir(&target.path, opts, ctx) {
            Ok(r) => {
                fill_from_dir(&mut item, &r, opts);
                if r.cancelled {
                    Err(WipeError::Cancelled)
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        },
        TargetKind::Move => {
            let dest = target.dest.clone().unwrap_or_default();
            match moving::move_and_wipe(&target.path, &dest, opts, ctx) {
                Ok(r) => {
                    item.moved_to = Some(r.moved_to.display().to_string());
                    item.bytes_overwritten = r.bytes_overwritten;
                    item.files_ok = r.files_wiped.max(u64::from(r.renamed));
                    item.files_err = r.wipe_errors;
                    item.failures = r.failures;
                    item.deleted = r.wipe_errors == 0;
                    if r.renamed {
                        item.passes = 0;
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        TargetKind::FreeSpace => wipe_free_space(&target.path, opts, ctx).map(|r| {
            item.bytes_overwritten = r.bytes_overwritten;
            item.files_ok = r.files_created;
            item.verified = r.verified;
            item.tips_files = r.tips.as_ref().map(|t| t.files).unwrap_or(0);
            item.tips_bytes = r.tips.as_ref().map(|t| t.bytes).unwrap_or(0);
            item.records = r.records;
        }),
        TargetKind::RecycleBin => match wipe_recycle_bin(opts, ctx) {
            Ok(r) => {
                fill_from_dir(&mut item, &r, opts);
                if r.cancelled {
                    Err(WipeError::Cancelled)
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        },
    };

    match result {
        Ok(()) => {
            item.status = if item.files_err > 0 { Status::Partial } else { Status::Ok };
        }
        Err(WipeError::Cancelled) => {
            item.status = Status::Cancelled;
            item.error = Some(WipeError::Cancelled.to_string());
        }
        Err(e @ WipeError::NotFound { .. }) => {
            item.status = Status::Skipped;
            item.error = Some(e.to_string());
        }
        Err(e) => {
            item.status = Status::Error;
            item.access_denied = e.is_access_denied();
            if e.is_locked() && target.kind == TargetKind::File {
                item.locked.push(target.path.display().to_string());
            }
            item.error = Some(e.to_string());
        }
    }
    item.seconds = started.elapsed().as_secs_f64();
    item
}

fn fill_from_dir(item: &mut ReportItem, r: &DirReport, opts: &WipeOptions) {
    item.bytes_overwritten = r.bytes_overwritten;
    item.files_ok = r.files_ok;
    item.files_err = r.files_err;
    item.streams = r.streams;
    item.verified = opts.verify && r.files_err == 0 && r.files_ok > 0;
    item.deleted =
        !opts.keep && r.dirs_err == 0 && r.files_err == 0 && opts.filter.as_ref().map(|f| f.is_empty()).unwrap_or(true);
    item.access_denied = r.access_denied;
    item.failures = r.failures.clone();
    item.locked = r.locked.iter().map(|p| p.display().to_string()).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{MediaInfo, MediaKind};
    use crate::plan::PlanItem;

    fn item(kind: MediaKind) -> PlanItem {
        PlanItem {
            target: Target::file("x.bin"),
            bytes: 100,
            files: 1,
            dirs: 0,
            volume: None,
            media: Some(MediaInfo { kind, trim: None, bus: None, model: None }),
            blocked: None,
            link: false,
            shortcut: false,
        }
    }

    #[test]
    fn work_bytes_follow_ssd_single_pass() {
        let mut o = WipeOptions { method: Method::DodE, verify: true, ..WipeOptions::default() };
        // 3 passes + verification everywhere while the rule is off.
        assert_eq!(o.work_bytes(&item(MediaKind::Ssd)), 400);
        o.flash_single_pass = true;
        // One pass + verification on SSD/flash, unchanged on HDD.
        assert_eq!(o.work_bytes(&item(MediaKind::Ssd)), 200);
        assert_eq!(o.work_bytes(&item(MediaKind::Flash)), 200);
        assert_eq!(o.work_bytes(&item(MediaKind::Hdd)), 400);
        assert_eq!(o.effective_for(true).method, Method::NistClear);
        assert_eq!(o.effective_for(false).method, Method::DodE);
    }
}
