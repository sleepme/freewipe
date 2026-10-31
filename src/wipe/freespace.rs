//! Wipe the free space of a volume.
//!
//! 1. A temporary folder `~freewipe-free-<random>` is created in the given
//!    directory (normally the volume root).
//! 2. **Pass 1** fills the volume with chunk files until the OS reports
//!    "disk full". Chunks never exceed the file system's maximum file size
//!    (FAT32: < 4 GiB). When a big write no longer fits, write sizes shrink
//!    step by step down to 512 bytes so the last slack is filled too.
//! 3. **Passes 2..N** rewrite every chunk file in place.
//! 4. The final pass is optionally verified, then every chunk is truncated
//!    and deleted and the folder removed.
//!
//! Step 4 also runs on any error and on cancellation (Ctrl+C / Stop), via a
//! drop guard, so a full disk is never left behind.
//!
//! Optional extras (see [`super::residue`]): before filling, the slack at
//! the end of every file ("cluster tips"); with the disk full after the
//! passes, the free NTFS file records (old names, small files).
//!
//! Limits (documented in the README): the NTFS log, the USN journal and
//! Volume Shadow Copies are not touched.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::filesystem::{self, FsKind};
use crate::progress::{Ctx, Stage};
use crate::sys::{self, DIRECT_IO_ALIGN};
use crate::tr;
use crate::util::{random_hex, AlignedBuf};

use super::file::{new_rng, overwrite};
use super::{check_cancel, residue, verify, IoCtx, WipeError, WipeOptions};

#[derive(Clone, Debug, Default)]
pub struct FreeSpaceReport {
    pub volume: PathBuf,
    pub filesystem: String,
    /// Bytes of free space that were filled on pass 1.
    pub bytes_filled: u64,
    pub bytes_overwritten: u64,
    pub files_created: u64,
    pub passes: usize,
    pub verified: bool,
    pub tips: Option<super::residue::TipsReport>,
    /// NTFS file records overwritten.
    pub records: u64,
}

/// Removes the fill files and folder however we leave the function.
struct FillGuard {
    dir: PathBuf,
    files: Vec<PathBuf>,
    /// Record-fill folder (many tiny files), removed first.
    records: Option<PathBuf>,
}

impl FillGuard {
    fn cleanup(&mut self) {
        if let Some(r) = self.records.take() {
            let _ = std::fs::remove_dir_all(&r);
        }
        for f in self.files.drain(..) {
            if let Ok(h) = sys::open_rw(&sys::open_name(&f)) {
                let _ = sys::truncate_zero(&h);
            }
            let _ = sys::delete_file(&f);
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

impl Drop for FillGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Wipe free space on the volume holding `path`.
pub fn wipe_free_space(path: &Path, opts: &WipeOptions, ctx: &mut Ctx) -> Result<FreeSpaceReport, WipeError> {
    let base =
        if path.is_dir() { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    if !base.exists() {
        return Err(WipeError::NotFound { path: path.to_path_buf() });
    }
    let vol = filesystem::probe(&base).at(&base)?;
    let chunk = vol.fs.freespace_chunk();
    let max_files = vol.fs.max_entries_per_dir().unwrap_or(u64::MAX).saturating_sub(16);
    let passes = opts.method.passes(opts.passes_override);
    let mut rng = new_rng();
    let mut buf = AlignedBuf::new(opts.buffer_size.max(DIRECT_IO_ALIGN), DIRECT_IO_ALIGN);

    if let Some(f) = residue::recover_journal(&base) {
        ctx.progress.note(&tr!("note.tips_repaired", path = f.display()));
    }
    let dir = base.join(format!("~freewipe-free-{}", random_hex(&mut rng, 8)));
    std::fs::create_dir(&dir).at(&dir)?;
    let mut guard = FillGuard { dir: dir.clone(), files: Vec::new(), records: None };
    let mut report = FreeSpaceReport {
        volume: vol.root.clone(),
        filesystem: vol.fs.display(),
        passes: passes.len(),
        ..Default::default()
    };
    ctx.progress.note(&tr!(
        "note.freespace_start",
        volume = vol.root.display(),
        fs = vol.fs.display(),
        free = crate::util::human_bytes(vol.free),
        chunk = crate::util::human_bytes(chunk)
    ));

    // --- cluster tips (before filling: needs no free space) -------------------
    if opts.cluster_tips {
        if vol.fs == FsKind::ReFs {
            ctx.progress.note(&tr!("note.tips_refs"));
        } else {
            let t = residue::wipe_cluster_tips(&vol.root, &base, &dir, &passes, &mut rng, ctx)?;
            ctx.progress.note(&tr!(
                "note.tips_done",
                files = t.files,
                bytes = crate::util::human_bytes(t.bytes),
                skipped = t.skipped
            ));
            report.tips = Some(t);
        }
    }

    // --- pass 1: fill -------------------------------------------------------
    let first = &passes[0];
    ctx.progress.pass(0, passes.len(), &first.label());
    ctx.progress.stage(Stage::Fill);
    let mut sizes: Vec<u64> = Vec::new();
    let mut hashes: Vec<Option<[u8; 32]>> = Vec::new();
    let verify_first = opts.verify && passes.len() == 1;

    'fill: loop {
        check_cancel(ctx)?;
        if guard.files.len() as u64 >= max_files {
            break;
        }
        let f = dir.join(format!("f{:06}.tmp", guard.files.len()));
        let h = match sys::create_new_rw(&f) {
            Ok(h) => h,
            Err(e) if sys::is_disk_full(&e) => break,
            Err(e) => return Err(WipeError::io(&f, e)),
        };
        guard.files.push(f.clone());
        ctx.progress.item(&f.display().to_string());

        let mut written = 0u64;
        let mut hasher = verify_first.then(Sha256::new);
        let mut step = buf.len();
        let slice = buf.as_mut_slice();
        loop {
            check_cancel(ctx)?;
            if opts.reserve_bytes > 0 {
                let free = sys::volume_raw(&dir).map(|v| v.free).unwrap_or(u64::MAX);
                if free <= opts.reserve_bytes {
                    finish_file(&h, &f, written, &mut sizes, &mut hashes, hasher)?;
                    break 'fill;
                }
            }
            let n = ((chunk - written) as usize).min(step);
            if n == 0 {
                break; // chunk complete → next file
            }
            fill_slice(&mut slice[..n], first, written, &mut rng);
            match sys::write_all(&h, &slice[..n]) {
                Ok(()) => {
                    if let Some(hw) = hasher.as_mut() {
                        hw.update(&slice[..n]);
                    }
                    written += n as u64;
                    report.bytes_filled += n as u64;
                    ctx.progress.advance(n as u64);
                }
                Err(e) if sys::is_disk_full(&e) => {
                    // Partial writes may have extended the file; rewind to
                    // the last fully written position before shrinking.
                    let _ = sys::seek_begin(&h, written);
                    if step > 512 {
                        step /= 8;
                        step = step.max(512);
                        continue;
                    }
                    finish_file(&h, &f, written, &mut sizes, &mut hashes, hasher)?;
                    break 'fill;
                }
                Err(e) if sys::is_file_too_large(&e) => break,
                Err(e) => return Err(WipeError::io(&f, e)),
            }
        }
        finish_file(&h, &f, written, &mut sizes, &mut hashes, hasher)?;
    }
    report.files_created = guard.files.len() as u64;
    report.bytes_overwritten = report.bytes_filled;

    // --- passes 2..N: rewrite in place ----------------------------------------
    for (i, pass) in passes.iter().enumerate().skip(1) {
        let last = i + 1 == passes.len();
        ctx.progress.pass(i, passes.len(), &pass.label());
        ctx.progress.stage(Stage::Overwrite);
        for (k, f) in guard.files.clone().iter().enumerate() {
            let size = sizes.get(k).copied().unwrap_or(0);
            if size == 0 {
                continue;
            }
            ctx.progress.item(&f.display().to_string());
            let h = sys::open_rw(&sys::open_name(f)).at(f)?;
            let mut hasher = (opts.verify && last).then(Sha256::new);
            overwrite(&h, size, pass, &mut buf, &mut rng, hasher.as_mut(), f, ctx)?;
            if let Some(hw) = hasher {
                hashes[k] = Some(hw.finalize().into());
            }
            report.bytes_overwritten += size;
        }
    }

    // --- verify ---------------------------------------------------------------
    if opts.verify {
        for (k, f) in guard.files.clone().iter().enumerate() {
            let (Some(size), Some(Some(expected))) = (sizes.get(k).copied(), hashes.get(k)) else { continue };
            if size == 0 {
                continue;
            }
            let actual = verify::hash_stream(&sys::open_name(f), size, &mut buf, f, ctx)?;
            verify::check(expected, &actual, f, "fill")?;
        }
        report.verified = report.bytes_filled > 0;
    }

    // --- free NTFS file records (disk still full) -------------------------------
    if opts.file_records {
        if vol.fs != FsKind::Ntfs {
            ctx.progress.note(&tr!("note.records_ntfs_only"));
        } else if opts.reserve_bytes > 0 {
            ctx.progress.note(&tr!("note.records_reserve"));
        } else {
            // Give NTFS a little room for the folders' indexes (and for
            // growing the MFT once the free records are used up).
            if let Some((k, _)) = sizes.iter().enumerate().max_by_key(|(_, s)| **s) {
                let f = &guard.files[k];
                if let Ok(h) = sys::open_rw(&sys::open_name(f)) {
                    let _ = sys::set_end(&h, sizes[k].saturating_sub(residue::RECORD_POOL));
                    let _ = sys::flush(&h);
                }
            }
            let rdir = dir.join("records");
            guard.records = Some(rdir.clone());
            let n = residue::fill_file_records(&rdir, &mut rng, ctx)?;
            ctx.progress.note(&tr!("note.records_done", n = n));
            report.records = n;
        }
    }

    // --- cleanup ----------------------------------------------------------------
    ctx.progress.stage(Stage::Cleanup);
    guard.cleanup();
    Ok(report)
}

fn fill_slice(buf: &mut [u8], pass: &crate::pattern::Pass, offset: u64, rng: &mut dyn rand::RngCore) {
    crate::pattern::fill(buf, pass, offset, rng);
}

/// Flush a finished fill file and record its size and (optional) digest.
fn finish_file(
    h: &sys::WipeHandle,
    f: &Path,
    written: u64,
    sizes: &mut Vec<u64>,
    hashes: &mut Vec<Option<[u8; 32]>>,
    hasher: Option<Sha256>,
) -> Result<(), WipeError> {
    // A flush can itself fail with "disk full" when metadata must grow;
    // that is expected at the very end of filling and not an error.
    if let Err(e) = sys::flush(h) {
        if !sys::is_disk_full(&e) {
            return Err(WipeError::io(f, e));
        }
    }
    sizes.push(written);
    hashes.push(hasher.map(|h| h.finalize().into()));
    Ok(())
}
