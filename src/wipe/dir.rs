//! Recursively wipe a directory tree: every file with [`wipe_file`], then
//! (unless `keep`) every folder is renamed to random names and removed,
//! deepest first.
//!
//! Symbolic links, junctions and other reparse points are **not**
//! followed — the link itself is removed, never its target — so a wipe can
//! never escape the tree it was given.

use std::fs;
use std::path::{Path, PathBuf};

use crate::progress::Ctx;
use crate::sys::DIRECT_IO_ALIGN;
use crate::util::AlignedBuf;

use super::file::{new_rng, obfuscate_name, wipe_file_with};
use super::{check_cancel, IoCtx, WipeError, WipeOptions};

/// Per-file failures kept in the report.
const MAX_FAILURES: usize = 200;

#[derive(Clone, Debug, Default)]
pub struct DirReport {
    pub files_ok: u64,
    pub files_err: u64,
    pub dirs_removed: u64,
    pub dirs_err: u64,
    pub links_removed: u64,
    pub bytes_overwritten: u64,
    pub streams: u64,
    pub cancelled: bool,
    /// Some failure was "access denied".
    pub access_denied: bool,
    /// `"<path> — <error>"`, capped at 200 entries.
    pub failures: Vec<String>,
    /// Files another program had open (capped like `failures`).
    pub locked: Vec<PathBuf>,
}

impl DirReport {
    fn fail(&mut self, p: &Path, e: &WipeError) {
        self.access_denied |= e.is_access_denied();
        if e.is_locked() && self.locked.len() < MAX_FAILURES {
            self.locked.push(p.to_path_buf());
        }
        if self.failures.len() < MAX_FAILURES {
            self.failures.push(format!("{} — {e}", p.display()));
        }
    }
}

/// Wipe `path` and everything below it (the folder itself is removed too).
pub fn wipe_dir(path: &Path, opts: &WipeOptions, ctx: &mut Ctx) -> Result<DirReport, WipeError> {
    let meta = fs::symlink_metadata(path).at(path)?;
    if !meta.is_dir() {
        return Err(WipeError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, crate::tr!("err.not_dir", path = path.display())),
        ));
    }
    let mut report = DirReport::default();
    walk(path, opts, ctx, &mut report);
    Ok(report)
}

/// What the folder filter in `opts` says about an entry.
fn filter_allows(opts: &WipeOptions, name: &str, is_dir: bool) -> bool {
    match &opts.filter {
        Some(f) if is_dir => f.wants_dir(name),
        Some(f) => f.wants_file(name),
        None => true,
    }
}

/// Folders are removed only when nothing filtered them (or they are empty).
fn may_remove_dir(opts: &WipeOptions, p: &Path, depth: usize) -> bool {
    if opts.keep {
        return false;
    }
    match &opts.filter {
        Some(f) if depth == 0 && f.contents_only => false,
        Some(f) if f.has_masks() => fs::read_dir(p).map(|mut r| r.next().is_none()).unwrap_or(false),
        _ => true,
    }
}

/// Wipe the *contents* of `path` but keep the folder (Recycle Bin).
pub fn wipe_dir_contents(
    path: &Path,
    opts: &WipeOptions,
    ctx: &mut Ctx,
    report: &mut DirReport,
    skip: &[&str],
) -> Result<(), WipeError> {
    let mut rng = new_rng();
    let mut buf = AlignedBuf::new(opts.buffer_size.max(DIRECT_IO_ALIGN), DIRECT_IO_ALIGN);
    for entry in sorted_entries(path)? {
        let name = entry.file_name().to_string_lossy().to_string();
        if skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if visit(&entry.path(), opts, ctx, report, &mut rng, &mut buf, 1).is_err() {
            break; // cancelled
        }
    }
    Ok(())
}

/// Visit the whole tree; cancellation is recorded in `report.cancelled`.
fn walk(path: &Path, opts: &WipeOptions, ctx: &mut Ctx, report: &mut DirReport) {
    let mut rng = new_rng();
    let mut buf = AlignedBuf::new(opts.buffer_size.max(DIRECT_IO_ALIGN), DIRECT_IO_ALIGN);
    let _ = visit(path, opts, ctx, report, &mut rng, &mut buf, 0);
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, WipeError> {
    let mut v: Vec<_> = fs::read_dir(path).at(path)?.flatten().collect();
    v.sort_by_key(|e| e.file_name());
    Ok(v)
}

/// Returns `Err(Cancelled)` to unwind the recursion on cancellation.
fn visit(
    p: &Path,
    opts: &WipeOptions,
    ctx: &mut Ctx,
    report: &mut DirReport,
    rng: &mut rand_chacha::ChaCha20Rng,
    buf: &mut AlignedBuf,
    depth: usize,
) -> Result<(), WipeError> {
    if check_cancel(ctx).is_err() {
        report.cancelled = true;
        return Err(WipeError::Cancelled);
    }
    let Ok(meta) = fs::symlink_metadata(p) else { return Ok(()) };
    let ft = meta.file_type();
    if depth > 0 {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !filter_allows(opts, &name, ft.is_dir()) {
            return Ok(());
        }
    }

    if ft.is_symlink() {
        // Remove the link itself (file or directory link / junction).
        if !opts.keep && (fs::remove_file(p).is_ok() || fs::remove_dir(p).is_ok()) {
            report.links_removed += 1;
        }
        return Ok(());
    }

    if ft.is_dir() {
        let entries = match sorted_entries(p) {
            Ok(e) => e,
            Err(e) => {
                report.dirs_err += 1;
                report.fail(p, &e);
                return Ok(());
            }
        };
        for e in entries {
            visit(&e.path(), opts, ctx, report, rng, buf, depth + 1)?;
        }
        if may_remove_dir(opts, p, depth) {
            let renamed = obfuscate_name(p, opts.rename_passes, rng, ctx);
            match fs::remove_dir(&renamed) {
                Ok(()) => report.dirs_removed += 1,
                Err(e) => {
                    report.dirs_err += 1;
                    report.fail(p, &WipeError::io(p, e));
                }
            }
        }
        return Ok(());
    }

    match wipe_file_with(p, opts, ctx, rng, buf) {
        Ok(r) => {
            report.files_ok += 1;
            report.bytes_overwritten += r.bytes_overwritten;
            report.streams += r.streams as u64;
        }
        Err(WipeError::Cancelled) => {
            report.cancelled = true;
            return Err(WipeError::Cancelled);
        }
        Err(e) => {
            report.files_err += 1;
            ctx.progress.note(&e.to_string());
            report.fail(p, &e);
        }
    }
    Ok(())
}
