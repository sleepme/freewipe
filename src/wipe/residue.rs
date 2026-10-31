//! Leftovers that filling the free space does not reach.
//!
//! **Cluster tips.** A file rarely ends exactly on a cluster boundary; the
//! rest of its last cluster ("slack") still holds whatever was there before —
//! often pieces of deleted files. Those clusters belong to the file, so the
//! free-space fill never touches them. For every file on the volume we
//! temporarily extend it to the end of its allocation, overwrite only the
//! slack, flush, and cut it back to the original size. File contents are
//! never touched; timestamps and attributes are restored. A small journal
//! next to the fill folder records the file being processed so an
//! interrupted run (power loss, crash) is repaired on the next start.
//!
//! **NTFS file records.** Small files live entirely inside their MFT record
//! and every record keeps the file's name; deleting a file only marks its
//! record free. With the disk full, we create many tiny files with long
//! random names and random content: NTFS hands out the free records first, so
//! the old names and contents are overwritten. Then the files are removed.
//!
//! Not covered (README): the NTFS log file ($LogFile), the USN journal,
//! Volume Shadow Copies, directory index slack and FAT/exFAT directory
//! entries.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rand::RngCore;

use crate::pattern::Pass;
use crate::progress::{Ctx, Stage};
use crate::sys;
use crate::util::random_hex;

use super::{check_cancel, IoCtx, WipeError};

/// Name of the crash journal (kept in the folder where the fill folder lives).
pub const JOURNAL: &str = "~freewipe-tips.journal";
/// Never treat more than this as slack (sanity limit for odd allocations).
const MAX_SLACK: u64 = 1 << 20;
/// Free space handed back to NTFS before the record fill (index + growth).
pub const RECORD_POOL: u64 = 128 << 20;
/// Upper bound on files created by the record fill.
const MAX_RECORD_FILES: u64 = 4_000_000;
const FILES_PER_DIR: u64 = 4000;
/// Resident content per record file (fits a 1 KiB record with the name).
const RECORD_DATA: usize = 480;

#[derive(Clone, Debug, Default)]
pub struct TipsReport {
    /// Files whose slack was overwritten.
    pub files: u64,
    pub bytes: u64,
    /// Files skipped (in use, read-only, cloud placeholder, …).
    pub skipped: u64,
}

// Windows attribute bits that make us leave a file alone.
const A_READONLY: u32 = 0x1;
const A_SYSTEM: u32 = 0x4;
const A_SPARSE: u32 = 0x200;
const A_REPARSE: u32 = 0x400;
const A_COMPRESSED: u32 = 0x800;
const A_OFFLINE: u32 = 0x1000;
const A_ENCRYPTED: u32 = 0x4000;
const A_RECALL_ON_OPEN: u32 = 0x4_0000;
const A_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const SKIP_ATTRS: u32 = A_READONLY
    | A_SYSTEM
    | A_SPARSE
    | A_REPARSE
    | A_COMPRESSED
    | A_OFFLINE
    | A_ENCRYPTED
    | A_RECALL_ON_OPEN
    | A_RECALL_ON_DATA_ACCESS;

/// Folder names at any level that are never walked.
fn skip_dir_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "system volume information" || n.starts_with("~freewipe-free-") || n.starts_with('$') && n != "$recycle.bin"
}

/// Records the file currently being extended, so a crash can be repaired.
struct Journal {
    path: PathBuf,
    handle: Option<sys::WipeHandle>,
}

fn mtime_nanos(p: &Path) -> u128 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

impl Journal {
    fn open(dir: &Path) -> Self {
        let path = dir.join(JOURNAL);
        let _ = std::fs::remove_file(&path);
        let handle = sys::create_new_rw(&path).ok();
        Journal { path, handle }
    }

    /// `size` and modification time of `file` before we extend it.
    fn record(&self, file: &Path, size: u64) {
        if let Some(h) = &self.handle {
            let line = format!("{size}\n{}\n{}\n", mtime_nanos(file), file.display());
            let _ = sys::seek_begin(h, 0);
            let _ = sys::write_all(h, line.as_bytes());
            let _ = sys::set_end(h, line.len() as u64);
        }
    }

    fn clear(&self) {
        if let Some(h) = &self.handle {
            let _ = sys::set_end(h, 0);
        }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.handle = None;
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Repair a file left extended by an interrupted cluster-tip run in `dir`.
/// Only cuts the file back if it was not changed since (same mtime, and at
/// most one slack longer than recorded). Returns the repaired path.
pub fn recover_journal(dir: &Path) -> Option<PathBuf> {
    let jp = dir.join(JOURNAL);
    let text = std::fs::read_to_string(&jp).ok()?;
    let _ = std::fs::remove_file(&jp);
    let mut lines = text.lines();
    let size: u64 = lines.next()?.parse().ok()?;
    let mtime: u128 = lines.next()?.parse().ok()?;
    let file = PathBuf::from(lines.next()?);
    let h = sys::open_rw(&sys::open_name(&file)).ok()?;
    let now = sys::file_size(&h).ok()?;
    if now > size && now - size <= MAX_SLACK && mtime_nanos(&file) == mtime {
        let times = sys::freeze_times(&h).ok();
        sys::set_end(&h, size).ok()?;
        let _ = sys::flush(&h);
        if let Some(t) = times {
            let _ = sys::restore_times(&h, &t);
        }
        return Some(file);
    }
    None
}

#[cfg(windows)]
fn skip_meta(meta: &std::fs::Metadata, _root_dev: u64) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & SKIP_ATTRS != 0
}

#[cfg(unix)]
fn skip_meta(meta: &std::fs::Metadata, root_dev: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    let _ = SKIP_ATTRS;
    meta.dev() != root_dev || meta.permissions().readonly()
}

#[cfg(windows)]
fn dev_of(_p: &Path) -> u64 {
    0
}

#[cfg(unix)]
fn dev_of(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).map(|m| m.dev()).unwrap_or(0)
}

/// Overwrite the slack of every file under `root` (not following links or
/// junctions, not leaving the volume, skipping `exclude`).
pub fn wipe_cluster_tips(
    root: &Path,
    journal_dir: &Path,
    exclude: &Path,
    passes: &[Pass],
    rng: &mut dyn RngCore,
    ctx: &mut Ctx,
) -> Result<TipsReport, WipeError> {
    recover_journal(journal_dir);
    ctx.progress.stage(Stage::Tips);
    let journal = Journal::open(journal_dir);
    let root_dev = dev_of(root);
    let mut report = TipsReport::default();
    let mut buf = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut last_item = Instant::now() - Duration::from_secs(1);
    while let Some(dir) = stack.pop() {
        check_cancel(ctx)?;
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            check_cancel(ctx)?;
            let path = entry.path();
            if path == exclude || path == journal.path {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() || skip_meta(&meta, root_dev) {
                if meta.is_file() {
                    report.skipped += 1;
                }
                continue;
            }
            if meta.is_dir() {
                if !entry.file_name().to_str().map(skip_dir_name).unwrap_or(false) {
                    stack.push(path);
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if last_item.elapsed() >= Duration::from_millis(150) {
                last_item = Instant::now();
                ctx.progress.item(&path.display().to_string());
            }
            match tip_file(&path, passes, &mut buf, rng, &journal) {
                Ok(0) => {}
                Ok(n) => {
                    report.files += 1;
                    report.bytes += n;
                }
                Err(_) => report.skipped += 1,
            }
        }
    }
    journal.clear();
    Ok(report)
}

/// Overwrite one file's slack. Returns the slack size (0 = none).
fn tip_file(
    path: &Path,
    passes: &[Pass],
    buf: &mut Vec<u8>,
    rng: &mut dyn RngCore,
    journal: &Journal,
) -> std::io::Result<u64> {
    let attrs = sys::attributes(path)?;
    let h = sys::open_rw(&sys::open_name(path))?;
    let size = sys::file_size(&h)?;
    let alloc = sys::alloc_size(&h)?;
    if alloc <= size {
        return Ok(0);
    }
    let slack = (alloc - size).min(MAX_SLACK);
    let times = sys::freeze_times(&h)?;
    journal.record(path, size);
    buf.resize(slack as usize, 0);
    let mut result = Ok(());
    for pass in passes {
        crate::pattern::fill(buf, pass, size, rng);
        result = sys::seek_begin(&h, size).and_then(|_| sys::write_all(&h, buf)).and_then(|_| sys::flush(&h));
        if result.is_err() {
            break;
        }
    }
    // Always cut back to the original size, whatever happened above.
    let cut = sys::set_end(&h, size).and_then(|_| sys::flush(&h));
    let _ = sys::restore_times(&h, &times);
    drop(h);
    if sys::attributes(path).map(|a| a != attrs).unwrap_or(false) {
        let _ = sys::set_attributes(path, attrs);
    }
    cut?;
    journal.clear();
    result.map(|_| slack)
}

/// With the volume full, create tiny files with long random names and random
/// resident content inside `dir` until NTFS has no record left to give.
/// Returns how many were created. The caller removes `dir`.
pub fn fill_file_records(dir: &Path, rng: &mut dyn RngCore, ctx: &mut Ctx) -> Result<u64, WipeError> {
    ctx.progress.stage(Stage::Residue);
    std::fs::create_dir(dir).at(dir)?;
    let mut data = vec![0u8; RECORD_DATA];
    let mut data_len = RECORD_DATA;
    let mut count = 0u64;
    let mut last_item = Instant::now() - Duration::from_secs(1);
    'outer: for k in 0.. {
        let sub = dir.join(format!("m{k}"));
        match std::fs::create_dir(&sub) {
            Ok(()) => count += 1,
            Err(e) if sys::is_disk_full(&e) => break,
            Err(e) => return Err(WipeError::io(&sub, e)),
        }
        for _ in 0..FILES_PER_DIR {
            check_cancel(ctx)?;
            if count >= MAX_RECORD_FILES {
                break 'outer;
            }
            let f = sub.join(random_hex(rng, 48));
            let h = match sys::create_new_rw(&f) {
                Ok(h) => h,
                Err(e) if sys::is_disk_full(&e) => break 'outer,
                Err(e) => return Err(WipeError::io(&f, e)),
            };
            count += 1;
            if data_len > 0 {
                rng.fill_bytes(&mut data[..data_len]);
                if let Err(e) = sys::write_all(&h, &data[..data_len]) {
                    if !sys::is_disk_full(&e) {
                        return Err(WipeError::io(&f, e));
                    }
                    data_len = 0; // no room for content any more: names only
                    let _ = sys::truncate_zero(&h);
                }
            }
            let _ = sys::flush(&h);
            if last_item.elapsed() >= Duration::from_millis(150) {
                last_item = Instant::now();
                ctx.progress.item(&crate::tr!("note.records_n", n = count));
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::Method;
    use crate::progress::{CancelFlag, NoProgress};

    fn ctx() -> (NoProgress, CancelFlag) {
        (NoProgress, CancelFlag::new())
    }

    #[test]
    fn tips_keep_content_size_and_times() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        let content: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&f, &content).unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(old).unwrap();
        let excl = dir.path().join("~freewipe-free-x");
        let (mut p, c) = ctx();
        let mut cx = Ctx { progress: &mut p, cancel: &c };
        let passes = Method::DodE.passes(None);
        let mut rng = super::super::file::new_rng();
        let r = wipe_cluster_tips(dir.path(), dir.path(), &excl, &passes, &mut rng, &mut cx).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), content);
        assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), old);
        assert!(r.files + r.skipped >= 1);
        assert!(!dir.path().join(JOURNAL).exists());
    }

    #[test]
    fn journal_repairs_extended_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("b.bin");
        std::fs::write(&f, vec![7u8; 100]).unwrap();
        let mt = mtime_nanos(&f);
        std::fs::write(dir.path().join(JOURNAL), format!("100\n{mt}\n{}\n", f.display())).unwrap();
        // simulate a crash after extending (keep mtime)
        let fh = std::fs::File::options().write(true).open(&f).unwrap();
        let t = fh.metadata().unwrap().modified().unwrap();
        fh.set_len(612).unwrap();
        fh.set_modified(t).unwrap();
        drop(fh);
        assert_eq!(recover_journal(dir.path()), Some(f.clone()));
        assert_eq!(std::fs::read(&f).unwrap(), vec![7u8; 100]);
    }

    #[test]
    fn journal_leaves_changed_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.bin");
        std::fs::write(&f, vec![1u8; 300]).unwrap();
        std::fs::write(dir.path().join(JOURNAL), format!("100\n1\n{}\n", f.display())).unwrap();
        assert_eq!(recover_journal(dir.path()), None);
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 300);
    }

    #[test]
    fn record_fill_creates_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, c) = ctx();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let mut cx = Ctx { progress: &mut p, cancel: &c };
        let mut rng = super::super::file::new_rng();
        let r = fill_file_records(&dir.path().join("m"), &mut rng, &mut cx);
        assert!(matches!(r, Err(WipeError::Cancelled)));
        assert!(std::fs::read_dir(dir.path().join("m").join("m0")).unwrap().count() > 0);
    }
}
