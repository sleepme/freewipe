//! Portable (non-Windows) backend built on `std::fs`.
//!
//! Mirrors the API of `windows_io` so the wipe engine is identical on every
//! platform. It is what the test-suite runs against on CI, and it also makes
//! freewipe usable on Linux/macOS volumes (ext4, btrfs, APFS, …). There are no
//! alternate data streams here: every file has exactly one stream.
//!
//! The only `unsafe` is a single `statvfs` call (free space), documented
//! below.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Sector alignment used for direct reads.
pub const DIRECT_IO_ALIGN: usize = 4096;

/// Name used to open a stream.
pub type OpenName = PathBuf;

pub fn open_name(p: &Path) -> OpenName {
    p.to_path_buf()
}

/// Owned file handle.
pub struct WipeHandle(File);

pub fn open_rw(name: &OpenName) -> io::Result<WipeHandle> {
    OpenOptions::new().read(true).write(true).open(name).map(WipeHandle)
}

/// Opens for reading. The page cache is not bypassed here (`O_DIRECT` has
/// too many per-filesystem caveats); `flush` already did `fsync`.
pub fn open_read_direct(name: &OpenName) -> io::Result<(WipeHandle, bool)> {
    File::open(name).map(|f| (WipeHandle(f), false))
}

pub fn open_read(name: &OpenName) -> io::Result<WipeHandle> {
    File::open(name).map(WipeHandle)
}

/// One `read` call (mirrors the Windows backend).
pub fn read_once(h: &WipeHandle, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match (&h.0).read(buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            r => return r,
        }
    }
}

pub fn create_new_rw(p: &Path) -> io::Result<WipeHandle> {
    OpenOptions::new().read(true).write(true).create_new(true).open(p).map(WipeHandle)
}

pub fn file_size(h: &WipeHandle) -> io::Result<u64> {
    Ok(h.0.metadata()?.len())
}

pub fn seek_begin(h: &WipeHandle, pos: u64) -> io::Result<()> {
    (&h.0).seek(SeekFrom::Start(pos)).map(|_| ())
}

pub fn write_all(h: &WipeHandle, buf: &[u8]) -> io::Result<()> {
    (&h.0).write_all(buf)
}

pub fn read_up_to(h: &WipeHandle, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match (&h.0).read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// `fsync`: data and metadata reach the device.
pub fn flush(h: &WipeHandle) -> io::Result<()> {
    h.0.sync_all()
}

pub fn truncate_zero(h: &WipeHandle) -> io::Result<()> {
    h.0.set_len(0)?;
    h.0.sync_all()
}

pub fn set_end(h: &WipeHandle, pos: u64) -> io::Result<()> {
    h.0.set_len(pos)
}

/// Bytes reserved on disk (`st_blocks` × 512).
pub fn alloc_size(h: &WipeHandle) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(h.0.metadata()?.blocks() * 512)
}

/// Timestamps saved so they can be put back.
pub struct SavedTimes(std::fs::FileTimes);

pub fn freeze_times(h: &WipeHandle) -> io::Result<SavedTimes> {
    let m = h.0.metadata()?;
    Ok(SavedTimes(std::fs::FileTimes::new().set_accessed(m.accessed()?).set_modified(m.modified()?)))
}

pub fn restore_times(h: &WipeHandle, t: &SavedTimes) -> io::Result<()> {
    h.0.set_times(t.0)
}

/// No Windows attributes here.
pub fn attributes(path: &Path) -> io::Result<u32> {
    std::fs::symlink_metadata(path).map(|_| 0)
}

pub fn set_attributes(_path: &Path, _attrs: u32) -> io::Result<()> {
    Ok(())
}

pub fn is_locked(_e: &io::Error) -> bool {
    false
}

/// A process that has one of the files open (not tracked here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locker {
    pub pid: u32,
    pub name: String,
    pub closable: bool,
}

pub fn lockers(_paths: &[PathBuf]) -> io::Result<Vec<Locker>> {
    Ok(Vec::new())
}

pub fn close_lockers(_paths: &[PathBuf]) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "not supported on this platform"))
}

/// Make the file writable (the read-only bit is the only attribute here).
pub fn clear_attributes(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let mut perm = meta.permissions();
    if perm.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub fn delete_file(p: &Path) -> io::Result<()> {
    std::fs::remove_file(p)
}

pub fn is_disk_full(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT))
}

pub fn is_file_too_large(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EFBIG))
}

/// One stream of a file (always the single data stream here).
pub struct StreamInfo {
    pub open_name: OpenName,
    pub size: u64,
    pub display: String,
    pub is_default: bool,
}

pub fn list_streams(path: &Path) -> io::Result<Vec<StreamInfo>> {
    let size = std::fs::metadata(path)?.len();
    Ok(vec![StreamInfo { open_name: path.to_path_buf(), size, display: "data".into(), is_default: true }])
}

// --- volumes & media ---------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct VolumeRaw {
    pub root: PathBuf,
    pub fs_name: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub cluster_size: u32,
}

/// `(mount point, fs type, source device)` of the mount holding `p`.
fn mount_of(p: &Path) -> Option<(PathBuf, String, String)> {
    let abs = std::fs::canonicalize(p).ok()?;
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(PathBuf, String, String)> = None;
    for line in text.lines() {
        // id parent maj:min root mountpoint opts ... - fstype source superopts
        let mut halves = line.splitn(2, " - ");
        let (Some(left), Some(right)) = (halves.next(), halves.next()) else { continue };
        let mp = left.split_whitespace().nth(4).map(unescape_mount);
        let mut r = right.split_whitespace();
        let (Some(fstype), Some(src)) = (r.next(), r.next()) else { continue };
        let Some(mp) = mp else { continue };
        let mpp = PathBuf::from(&mp);
        if abs.starts_with(&mpp)
            && best.as_ref().map(|b| mpp.as_os_str().len() >= b.0.as_os_str().len()).unwrap_or(true)
        {
            best = Some((mpp, fstype.to_string(), src.to_string()));
        }
    }
    best
}

fn unescape_mount(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

pub fn volume_root(p: &Path) -> io::Result<PathBuf> {
    Ok(mount_of(p).map(|m| m.0).unwrap_or_else(|| PathBuf::from("/")))
}

pub fn volume_raw(p: &Path) -> io::Result<VolumeRaw> {
    let (root, fs_name) = match mount_of(p) {
        Some((r, f, _)) => (r, f),
        None => (PathBuf::from("/"), String::new()),
    };
    let (total, free, bsize) = statvfs(p)?;
    Ok(VolumeRaw { root, fs_name, label: String::new(), total, free, cluster_size: bsize })
}

fn statvfs(p: &Path) -> io::Result<(u64, u64, u32)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    // SAFETY: `statvfs` is plain old data, so all-zero is a valid value; `c`
    // is a valid NUL-terminated path and `st` a valid out-pointer.
    #[allow(unsafe_code)]
    let (rc, st) = unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c.as_ptr(), &mut st), st)
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // Field widths differ between libc targets; the casts are no-ops on some.
    #[allow(clippy::unnecessary_cast)]
    let frsize = st.f_frsize as u64;
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_blocks as u64 * frsize, st.f_bavail as u64 * frsize, st.f_bsize as u32))
}

pub fn list_volumes() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/")];
    if let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") {
        for line in text.lines() {
            let mut halves = line.splitn(2, " - ");
            let (Some(left), Some(right)) = (halves.next(), halves.next()) else { continue };
            let src = right.split_whitespace().nth(1).unwrap_or("");
            if !src.starts_with("/dev/") {
                continue;
            }
            if let Some(mp) = left.split_whitespace().nth(4).map(unescape_mount) {
                let p = PathBuf::from(mp);
                if !v.contains(&p) {
                    v.push(p);
                }
            }
        }
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DriveKind {
    #[default]
    Unknown,
    Removable,
    Fixed,
    Remote,
    CdRom,
    RamDisk,
}

#[derive(Clone, Debug, Default)]
pub struct MediaRaw {
    pub drive: DriveKind,
    pub seek_penalty: Option<bool>,
    pub trim: Option<bool>,
    pub removable_media: Option<bool>,
    pub bus: Option<u32>,
    pub model: Option<String>,
}

/// Best-effort via sysfs: `queue/rotational`, `removable`, `device/model`.
pub fn media_raw(p: &Path) -> MediaRaw {
    let mut out = MediaRaw::default();
    let Some((_, fstype, src)) = mount_of(p) else { return out };
    if matches!(fstype.as_str(), "nfs" | "nfs4" | "cifs" | "smb3" | "sshfs" | "fuse.sshfs") {
        out.drive = DriveKind::Remote;
        return out;
    }
    if matches!(fstype.as_str(), "tmpfs" | "ramfs") {
        out.drive = DriveKind::RamDisk;
        return out;
    }
    let Some(dev) = src.strip_prefix("/dev/") else { return out };
    let Ok(mut sys) = std::fs::canonicalize(format!("/sys/class/block/{dev}")) else { return out };
    // A partition's queue lives on its parent disk.
    if !sys.join("queue").exists() {
        if let Some(parent) = sys.parent() {
            sys = parent.to_path_buf();
        }
    }
    let read = |rel: &str| std::fs::read_to_string(sys.join(rel)).ok().map(|s| s.trim().to_string());
    out.drive = DriveKind::Fixed;
    if let Some(r) = read("queue/rotational") {
        out.seek_penalty = Some(r == "1");
    }
    if let Some(r) = read("removable") {
        out.removable_media = Some(r == "1");
        if r == "1" {
            out.drive = DriveKind::Removable;
        }
    }
    if let Some(d) = read("queue/discard_max_bytes") {
        out.trim = Some(d != "0");
    }
    out.model = read("device/model").filter(|m| !m.is_empty());
    if sys.to_string_lossy().contains("/usb") {
        out.bus = Some(7); // same numbering as Windows' BusTypeUsb
    } else if dev.starts_with("nvme") {
        out.bus = Some(17);
    }
    out
}
