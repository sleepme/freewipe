//! Verification of the final pass.
//!
//! While the last pass is written its bytes are fed into SHA-256. Afterwards
//! the stream is re-opened — on Windows with `FILE_FLAG_NO_BUFFERING`, so the
//! read comes from the device rather than the cache — and hashed again. The
//! two digests must match. This works the same for fixed bytes, Gutmann
//! patterns and random data.
//!
//! Unbuffered reads have strict rules: every request must start at a
//! sector-aligned offset and ask for a whole number of sectors. The reader
//! therefore asks for full aligned buffers and stops at the first short read
//! (end of file) instead of reading again from the unaligned position — that
//! second read is what Windows rejects with `ERROR_INVALID_PARAMETER` (87).
//! If unbuffered reading fails for any other reason, verification falls back
//! to a normal cached read rather than failing the wipe.

use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::progress::{Ctx, Stage};
use crate::sys::{self, OpenName};
use crate::util::AlignedBuf;

use super::{check_cancel, IoCtx, WipeError};

/// Why hashing stopped early.
enum Stop {
    Io(io::Error),
    Wipe(WipeError),
}

/// Hash the first `size` bytes delivered by `read` (one OS read per call).
/// Returns `None` if the stream turned out shorter than `size`.
fn hash_reader(
    size: u64,
    buf: &mut [u8],
    mut read: impl FnMut(&mut [u8]) -> io::Result<usize>,
    mut on_chunk: impl FnMut(u64) -> Result<(), WipeError>,
) -> Result<Option<[u8; 32]>, Stop> {
    let mut hasher = Sha256::new();
    let mut remaining = size;
    while remaining > 0 {
        let got = read(buf).map_err(Stop::Io)?;
        if got == 0 {
            break;
        }
        let used = (got as u64).min(remaining) as usize;
        hasher.update(&buf[..used]);
        remaining -= used as u64;
        on_chunk(used as u64).map_err(Stop::Wipe)?;
        if got < buf.len() {
            break; // short read = end of file; never read again from here
        }
    }
    Ok((remaining == 0).then(|| hasher.finalize().into()))
}

/// SHA-256 of the first `size` bytes of a stream, read back from the medium.
/// A stream shorter than `size` yields an all-zero digest (= mismatch).
pub fn hash_stream(
    name: &OpenName,
    size: u64,
    buf: &mut AlignedBuf,
    path: &Path,
    ctx: &mut Ctx,
) -> Result<[u8; 32], WipeError> {
    ctx.progress.stage(Stage::Verify);
    let slice = buf.as_mut_slice();

    let (h, direct) = sys::open_read_direct(name).at(path)?;
    let mut done = 0u64;
    let first = hash_reader(
        size,
        slice,
        |b| sys::read_once(&h, b),
        |n| {
            done += n;
            ctx.progress.advance(n);
            check_cancel(ctx)
        },
    );
    drop(h);
    match first {
        Ok(d) => return Ok(d.unwrap_or([0u8; 32])),
        Err(Stop::Wipe(e)) => return Err(e),
        Err(Stop::Io(e)) if !direct => return Err(WipeError::io(path, e)),
        Err(Stop::Io(_)) => {} // retry below with the cache
    }

    // Fallback: cached read (the data was flushed with FlushFileBuffers).
    ctx.progress.note(&crate::tr!("note.verify_cached"));
    let h = sys::open_read(name).at(path)?;
    let mut skip = done;
    let second = hash_reader(
        size,
        slice,
        |b| sys::read_once(&h, b),
        |n| {
            // Don't count bytes twice in the progress bar.
            let counted = n.saturating_sub(skip);
            skip = skip.saturating_sub(n);
            ctx.progress.advance(counted);
            check_cancel(ctx)
        },
    );
    match second {
        Ok(d) => Ok(d.unwrap_or([0u8; 32])),
        Err(Stop::Wipe(e)) => Err(e),
        Err(Stop::Io(e)) => Err(WipeError::io(path, e)),
    }
}

/// Compare an expected digest with a read-back one.
pub fn check(expected: &[u8], actual: &[u8], path: &Path, stream: &str) -> Result<(), WipeError> {
    if expected == actual {
        Ok(())
    } else {
        Err(WipeError::VerifyMismatch { path: path.to_path_buf(), stream: stream.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTOR: usize = 4096;

    /// Behaves like a Windows handle opened with FILE_FLAG_NO_BUFFERING:
    /// misaligned offsets or lengths fail with error 87.
    struct Unbuffered {
        data: Vec<u8>,
        pos: usize,
    }

    impl Unbuffered {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos % SECTOR != 0 || buf.len() % SECTOR != 0 {
                return Err(io::Error::from_raw_os_error(87));
            }
            let n = buf.len().min(self.data.len().saturating_sub(self.pos));
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn sha(data: &[u8]) -> [u8; 32] {
        Sha256::digest(data).into()
    }

    #[test]
    fn unbuffered_rules_are_respected_for_any_size() {
        for size in [1usize, 15, 100, 4095, 4096, 4097, 8192, 70_000, 3 * 65_536 + 5] {
            let data: Vec<u8> = (0..size).map(|i| (i * 7 + 3) as u8).collect();
            let mut f = Unbuffered { data: data.clone(), pos: 0 };
            let mut buf = vec![0u8; 16 * SECTOR];
            let r = hash_reader(size as u64, &mut buf, |b| f.read(b), |_| Ok(()));
            match r {
                Ok(Some(d)) => assert_eq!(d, sha(&data), "size {size}"),
                Ok(None) => panic!("size {size}: reported short"),
                Err(Stop::Io(e)) => panic!("size {size}: {e}"),
                Err(Stop::Wipe(e)) => panic!("size {size}: {e}"),
            }
        }
    }

    #[test]
    fn shorter_stream_is_detected() {
        let mut f = Unbuffered { data: vec![1u8; 5000], pos: 0 };
        let mut buf = vec![0u8; 2 * SECTOR];
        let r = hash_reader(9000, &mut buf, |b| f.read(b), |_| Ok(()));
        assert!(matches!(r, Ok(None)));
    }

    #[test]
    fn old_loop_would_have_failed() {
        // Regression guard documenting the bug: reading again after a short
        // read hits a misaligned offset.
        let mut f = Unbuffered { data: vec![0u8; 100], pos: 0 };
        let mut buf = vec![0u8; SECTOR];
        assert_eq!(f.read(&mut buf).unwrap(), 100);
        assert_eq!(f.read(&mut buf).unwrap_err().raw_os_error(), Some(87));
    }
}
