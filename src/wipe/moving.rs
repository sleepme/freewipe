//! "Move to … and wipe the original".
//!
//! * Same volume: a rename. The data never moves on disk, so there is no old
//!   copy to wipe — freewipe just renames and says so.
//! * Another volume: every file is copied, flushed and read back; its SHA-256
//!   must match the source. Only when the whole copy is verified is the
//!   source wiped with the chosen method (file or folder wipe). On any copy
//!   error the partial copy is removed and the source is left untouched.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::progress::{Ctx, Stage};
use crate::sys;
use crate::tr;

use super::{check_cancel, wipe_dir, wipe_file, IoCtx, WipeError, WipeOptions};

/// Outcome of a move.
#[derive(Clone, Debug, Default)]
pub struct MoveReport {
    /// Where the item ended up.
    pub moved_to: PathBuf,
    /// Renamed on the same volume (nothing to wipe).
    pub renamed: bool,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub bytes_overwritten: u64,
    pub files_wiped: u64,
    /// Files of a folder that could not be wiped afterwards.
    pub wipe_errors: u64,
    pub failures: Vec<String>,
}

/// `dest/name`, or `dest/name (2)`, `(3)`… if taken.
fn free_name(dest: &Path, src: &Path) -> PathBuf {
    let name = src.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "item".into());
    let first = dest.join(&name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let p = Path::new(&name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..10_000)
        .map(|i| dest.join(format!("{stem} ({i}){ext}")))
        .find(|c| fs::symlink_metadata(c).is_err())
        .unwrap_or(first)
}

fn same_volume(a: &Path, b: &Path) -> bool {
    match (sys::volume_root(a), sys::volume_root(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Move `src` into the folder `dest`, then wipe `src`.
pub fn move_and_wipe(src: &Path, dest: &Path, opts: &WipeOptions, ctx: &mut Ctx) -> Result<MoveReport, WipeError> {
    if let Some(why) = crate::plan::move_problem(src, Some(dest)) {
        return Err(WipeError::Refused { path: src.to_path_buf(), reason: why });
    }
    let meta = fs::symlink_metadata(src).at(src)?;
    let target = free_name(dest, src);
    let mut report = MoveReport { moved_to: target.clone(), ..Default::default() };

    if same_volume(src, dest) {
        fs::rename(src, &target).at(src)?;
        report.renamed = true;
        ctx.progress.note(&tr!("note.move_renamed", path = target.display()));
        return Ok(report);
    }

    // 1. Copy and verify everything.
    ctx.progress.stage(Stage::Copy);
    let copied = if meta.is_dir() {
        copy_tree(src, &target, ctx, &mut report)
    } else {
        copy_verified(src, &target, ctx).map(|n| {
            report.bytes_copied += n;
            report.files_copied += 1;
        })
    };
    if let Err(e) = copied {
        // Leave the source untouched and remove the partial copy.
        if meta.is_dir() {
            let _ = fs::remove_dir_all(&target);
        } else {
            let _ = fs::remove_file(&target);
        }
        return Err(e);
    }
    ctx.progress.note(&tr!(
        "note.move_copied",
        files = report.files_copied,
        bytes = crate::util::human_bytes(report.bytes_copied),
        path = target.display()
    ));

    // 2. Wipe the original.
    let opts = WipeOptions { keep: false, filter: None, ..opts.clone() };
    if meta.is_dir() {
        let r = wipe_dir(src, &opts, ctx)?;
        report.bytes_overwritten = r.bytes_overwritten;
        report.files_wiped = r.files_ok;
        report.wipe_errors = r.files_err;
        report.failures = r.failures;
        if r.cancelled {
            return Err(WipeError::Cancelled);
        }
    } else {
        let r = wipe_file(src, &opts, ctx)?;
        report.bytes_overwritten = r.bytes_overwritten;
        report.files_wiped = 1;
    }
    Ok(report)
}

fn copy_tree(src: &Path, dst: &Path, ctx: &mut Ctx, report: &mut MoveReport) -> Result<(), WipeError> {
    fs::create_dir(dst).at(dst)?;
    let mut entries: Vec<_> = fs::read_dir(src).at(src)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        check_cancel(ctx)?;
        let from = e.path();
        let to = dst.join(e.file_name());
        let ft = e.file_type().at(&from)?;
        if ft.is_symlink() {
            continue; // links are not followed (and are removed with the source)
        } else if ft.is_dir() {
            copy_tree(&from, &to, ctx, report)?;
        } else {
            report.bytes_copied += copy_verified(&from, &to, ctx)?;
            report.files_copied += 1;
        }
    }
    if let Ok(m) = fs::metadata(src).and_then(|m| m.modified()) {
        let _ = fs::File::open(dst).and_then(|f| f.set_modified(m));
    }
    Ok(())
}

/// Copy one file, flush it, read the copy back and compare SHA-256.
/// Keeps the modification time. Returns the size.
fn copy_verified(src: &Path, dst: &Path, ctx: &mut Ctx) -> Result<u64, WipeError> {
    ctx.progress.item(&src.display().to_string());
    let mut input = fs::File::open(src).at(src)?;
    let mut out = fs::OpenOptions::new().write(true).create_new(true).open(dst).at(dst)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut hash = Sha256::new();
    let mut total = 0u64;
    loop {
        check_cancel(ctx)?;
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WipeError::io(src, e)),
        };
        out.write_all(&buf[..n]).at(dst)?;
        hash.update(&buf[..n]);
        total += n as u64;
        ctx.progress.advance(n as u64);
    }
    out.sync_all().at(dst)?;
    if let Ok(m) = input.metadata().and_then(|m| m.modified()) {
        let _ = out.set_modified(m);
    }
    drop(out);
    let expected: [u8; 32] = hash.finalize().into();

    // Read back.
    let mut back = fs::File::open(dst).at(dst)?;
    let mut h2 = Sha256::new();
    loop {
        check_cancel(ctx)?;
        let n = match back.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WipeError::io(dst, e)),
        };
        h2.update(&buf[..n]);
        ctx.progress.advance(n as u64);
    }
    let actual: [u8; 32] = h2.finalize().into();
    if actual != expected {
        return Err(WipeError::VerifyMismatch { path: dst.to_path_buf(), stream: "copy".into() });
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{CancelFlag, NoProgress};

    #[test]
    fn free_names() {
        let d = tempfile::tempdir().unwrap();
        let src = Path::new("/x/report.pdf");
        assert_eq!(free_name(d.path(), src), d.path().join("report.pdf"));
        std::fs::write(d.path().join("report.pdf"), b"1").unwrap();
        assert_eq!(free_name(d.path(), src), d.path().join("report (2).pdf"));
    }

    #[test]
    fn copy_then_wipe_tree() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let src = a.path().join("data");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("one.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub").join("two.bin"), vec![7u8; 100_000]).unwrap();
        let (mut p, c) = (NoProgress, CancelFlag::new());
        let mut ctx = Ctx::new(&c, &mut p);
        let opts = WipeOptions { method: crate::Method::Zero, rename_passes: 1, ..Default::default() };
        // Exercise the copy path directly (temp dirs share a volume here).
        let dst = b.path().join("data");
        let mut r = MoveReport::default();
        copy_tree(&src, &dst, &mut ctx, &mut r).unwrap();
        assert_eq!(r.files_copied, 2);
        assert_eq!(std::fs::read(dst.join("sub").join("two.bin")).unwrap(), vec![7u8; 100_000]);
        // And the full operation (same volume → rename).
        let r = move_and_wipe(&src, b.path(), &opts, &mut ctx).unwrap();
        assert!(r.renamed);
        assert!(!src.exists());
        assert_eq!(r.moved_to, b.path().join("data (2)"));
        assert!(move_and_wipe(&b.path().join("data"), b.path(), &opts, &mut ctx).is_err(), "same folder");
    }
}
