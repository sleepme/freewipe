//! Securely wipe one file: every data stream (including NTFS/ReFS alternate
//! data streams) is overwritten pass by pass with a flush after each pass,
//! the final pass is optionally verified by reading it back from the medium,
//! then — unless `keep` — the file is renamed several times to random names
//! (so the original name does not survive in the directory entry), truncated
//! to zero and deleted.

use std::path::{Path, PathBuf};

use rand::rngs::OsRng;
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};

use crate::pattern::{fill, Pass};
use crate::progress::{Ctx, Stage};
use crate::sys::{self, WipeHandle, DIRECT_IO_ALIGN};
use crate::util::{random_hex, AlignedBuf};

use super::{check_cancel, verify, IoCtx, WipeError, WipeOptions};

/// Outcome of wiping one file.
#[derive(Clone, Debug, Default)]
pub struct FileReport {
    pub streams: usize,
    pub passes: usize,
    pub bytes_overwritten: u64,
    pub verified: bool,
    pub deleted: bool,
}

/// Seeded CSPRNG (ChaCha20, key from the OS).
pub(crate) fn new_rng() -> ChaCha20Rng {
    ChaCha20Rng::from_rng(OsRng).unwrap_or_else(|_| ChaCha20Rng::from_entropy())
}

/// Wipe `path` according to `opts`.
pub fn wipe_file(path: &Path, opts: &WipeOptions, ctx: &mut Ctx) -> Result<FileReport, WipeError> {
    let mut rng = new_rng();
    let mut buf = AlignedBuf::new(opts.buffer_size.max(DIRECT_IO_ALIGN), DIRECT_IO_ALIGN);
    wipe_file_with(path, opts, ctx, &mut rng, &mut buf)
}

/// Same as [`wipe_file`] but reusing an RNG and buffer (directories).
pub(crate) fn wipe_file_with(
    path: &Path,
    opts: &WipeOptions,
    ctx: &mut Ctx,
    rng: &mut ChaCha20Rng,
    buf: &mut AlignedBuf,
) -> Result<FileReport, WipeError> {
    check_cancel(ctx)?;
    ctx.progress.item(&path.display().to_string());

    sys::clear_attributes(path).at(path)?;
    let mut streams = sys::list_streams(path).at(path)?;
    if !opts.include_ads {
        streams.retain(|s| s.is_default);
    }
    let passes = opts.method.passes(opts.passes_override);
    let mut report = FileReport { streams: streams.len(), passes: passes.len(), ..Default::default() };

    for st in &streams {
        if st.size == 0 {
            continue;
        }
        if !st.is_default {
            ctx.progress.note(&crate::tr!("note.ads", name = st.display));
        }
        let h = sys::open_rw(&st.open_name).at(path)?;
        for (i, pass) in passes.iter().enumerate() {
            let last = i + 1 == passes.len();
            ctx.progress.pass(i, passes.len(), &pass.label());
            ctx.progress.stage(Stage::Overwrite);
            let mut hasher = (opts.verify && last).then(Sha256::new);
            overwrite(&h, st.size, pass, buf, rng, hasher.as_mut(), path, ctx)?;
            report.bytes_overwritten += st.size;
            if let Some(hw) = hasher {
                let expected = hw.finalize();
                let actual = verify::hash_stream(&st.open_name, st.size, buf, path, ctx)?;
                verify::check(&expected, &actual, path, &st.display)?;
            }
        }
        // `h` closes here, before rename/delete.
    }
    report.verified = opts.verify && report.bytes_overwritten > 0;

    if !opts.keep {
        let final_path = obfuscate_name(path, opts.rename_passes, rng, ctx);
        ctx.progress.stage(Stage::Truncate);
        if let Ok(h) = sys::open_rw(&sys::open_name(&final_path)) {
            let _ = sys::truncate_zero(&h);
        }
        ctx.progress.stage(Stage::Delete);
        sys::delete_file(&final_path).at(&final_path)?;
        report.deleted = true;
    }
    Ok(report)
}

/// Write one pass over `size` bytes from offset 0, then flush to the device.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overwrite(
    h: &WipeHandle,
    size: u64,
    pass: &Pass,
    buf: &mut AlignedBuf,
    rng: &mut dyn RngCore,
    mut hasher: Option<&mut Sha256>,
    path: &Path,
    ctx: &mut Ctx,
) -> Result<(), WipeError> {
    sys::seek_begin(h, 0).at(path)?;
    let slice = buf.as_mut_slice();
    let mut written = 0u64;
    while written < size {
        check_cancel(ctx)?;
        let chunk = (size - written).min(slice.len() as u64) as usize;
        fill(&mut slice[..chunk], pass, written, rng);
        sys::write_all(h, &slice[..chunk]).at(path)?;
        if let Some(hw) = hasher.as_deref_mut() {
            hw.update(&slice[..chunk]);
        }
        written += chunk as u64;
        ctx.progress.advance(chunk as u64);
    }
    sys::flush(h).at(path)?;
    Ok(())
}

/// Rename `path` to random names of the same length as the original (so the
/// directory entry is overwritten in place where possible). Best-effort:
/// returns the last name that actually exists.
pub(crate) fn obfuscate_name(path: &Path, times: u32, rng: &mut dyn RngCore, ctx: &mut Ctx) -> PathBuf {
    if times == 0 {
        return path.to_path_buf();
    }
    ctx.progress.stage(Stage::Rename);
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let len = path.file_name().map(|n| n.to_string_lossy().chars().count()).unwrap_or(12).clamp(8, 64);
    let mut current = path.to_path_buf();
    for _ in 0..times {
        let target = parent.join(random_hex(rng, len));
        if target.exists() {
            continue;
        }
        if sys::move_file(&current, &target).is_ok() {
            current = target;
        }
    }
    current
}
