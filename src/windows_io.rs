//! Windows backend: thin, audited wrappers over the Win32 calls freewipe needs.
//!
//! This is the **only** module in the crate allowed to use `unsafe`
//! (`lib.rs` denies it everywhere else). Every block carries a `// SAFETY:`
//! note. The rest of the crate sees plain safe functions with `io::Result`.
//!
//! Choices that matter for actually reaching the medium:
//! * data handles use `FILE_FLAG_WRITE_THROUGH`, and every pass ends with
//!   `FlushFileBuffers`, so writes are not left in the cache;
//! * verification re-opens the stream with `FILE_FLAG_NO_BUFFERING` (aligned
//!   buffers) so the read-back comes from the device, not the cache;
//! * all streams — the default `::$DATA` *and* NTFS/ReFS alternate data
//!   streams — are enumerated with `FindFirstStreamW` and wiped.
//!
//! Numeric Win32 constants are defined locally as plain integers so the code
//! does not depend on where a given `windows-sys` version puts them.

#![warn(clippy::undocumented_unsafe_blocks)]

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_USER};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DeleteFileW, FileStandardInfo, FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
    FlushFileBuffers, GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW, GetFileAttributesW,
    GetFileInformationByHandleEx, GetFileSizeEx, GetFileTime, GetLogicalDrives, GetVolumeInformationW,
    GetVolumePathNameW, MoveFileExW, ReadFile, SetEndOfFile, SetFileAttributesW, SetFilePointerEx, SetFileTime,
    WriteFile, FILE_STANDARD_INFO, WIN32_FIND_STREAM_DATA,
};
use windows_sys::Win32::System::Power::SetThreadExecutionState;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
};
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmShutdown, RmStartSession, RM_PROCESS_INFO,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, SHChangeNotify, ShellExecuteExW, SHELLEXECUTEINFOW};

// --- Win32 constants (kept local on purpose) --------------------------------
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const CREATE_NEW: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
const FILE_BEGIN: u32 = 0;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_HANDLE_DISK_FULL: i32 = 39;
const ERROR_DISK_FULL: i32 = 112;
const ERROR_FILE_TOO_LARGE: i32 = 223;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_SUCCESS: u32 = 0;
const ERROR_NO_MORE_ITEMS: u32 = 259;

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const STORAGE_DEVICE_PROPERTY: u32 = 0;
const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;
const STORAGE_DEVICE_TRIM_PROPERTY: u32 = 8;
const PROPERTY_STANDARD_QUERY: u32 = 0;

const KEY_READ: u32 = 0x20019;
const KEY_WRITE: u32 = 0x20006;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_OPTION_NON_VOLATILE: u32 = 0;

const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
const SHCNF_IDLIST: u32 = 0;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
const SW_HIDE: i32 = 0;
const SW_SHOWNORMAL: i32 = 1;
const ERROR_ACCESS_DENIED_U32: u32 = 5;

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER_CLASS: i32 = 1; // TokenUser

const ES_CONTINUOUS: u32 = 0x8000_0000;
const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

/// Sector alignment used for unbuffered reads (covers 512e and 4Kn drives).
pub const DIRECT_IO_ALIGN: usize = 4096;

// --- helpers -----------------------------------------------------------------

/// Last Win32 error as an `io::Error`.
fn last_error() -> io::Error {
    // SAFETY: GetLastError has no preconditions; it reads thread-local state.
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn win_err(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// NUL-terminated UTF-16 for a plain string.
pub fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// NUL-terminated UTF-16 path for the `*W` APIs. Absolute paths longer than
/// `MAX_PATH` get the `\\?\` prefix so deep trees still work.
pub fn to_wide(p: &Path) -> Vec<u16> {
    let s = p.as_os_str().to_string_lossy();
    let long = s.len() >= 248 && !s.starts_with(r"\\?\");
    let prefixed: std::ffi::OsString = if long && s.starts_with(r"\\") {
        format!(r"\\?\UNC\{}", &s[2..]).into()
    } else if long && p.is_absolute() {
        format!(r"\\?\{s}").into()
    } else {
        p.as_os_str().to_os_string()
    };
    prefixed.encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

// --- file handles ------------------------------------------------------------

/// Name used to open a stream: NUL-terminated UTF-16.
pub type OpenName = Vec<u16>;

/// Open name for a path's default data stream.
pub fn open_name(p: &Path) -> OpenName {
    to_wide(p)
}

/// Owned file handle that closes itself on drop.
pub struct WipeHandle(HANDLE);

// SAFETY: a Win32 file handle may be used from any thread; we never share it
// concurrently (no Sync), we only move it.
unsafe impl Send for WipeHandle {}

impl Drop for WipeHandle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a valid handle we own (constructors reject
        // INVALID_HANDLE_VALUE) and it is closed exactly once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn create(name: &[u16], access: u32, share: u32, disposition: u32, flags: u32) -> io::Result<WipeHandle> {
    debug_assert_eq!(name.last(), Some(&0));
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call;
    // null security attributes and template handle are permitted.
    let h = unsafe { CreateFileW(name.as_ptr(), access, share, ptr::null(), disposition, flags, ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    Ok(WipeHandle(h))
}

/// Open an existing stream for read+write with write-through.
pub fn open_rw(name: &OpenName) -> io::Result<WipeHandle> {
    create(name, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, OPEN_EXISTING, FILE_FLAG_WRITE_THROUGH)
}

/// Open a stream for reading straight from the device (no cache). Reads must
/// use buffers aligned to [`DIRECT_IO_ALIGN`]. Falls back to a cached handle
/// if the volume refuses unbuffered I/O; the bool says which one you got.
pub fn open_read_direct(name: &OpenName) -> io::Result<(WipeHandle, bool)> {
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    match create(name, GENERIC_READ, share, OPEN_EXISTING, FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN) {
        Ok(h) => Ok((h, true)),
        Err(_) => create(name, GENERIC_READ, share, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN).map(|h| (h, false)),
    }
}

/// Open a stream for ordinary (cached) reading.
pub fn open_read(name: &OpenName) -> io::Result<WipeHandle> {
    create(name, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN)
}

/// Create a brand-new file (fails if it exists) for write-through writing.
pub fn create_new_rw(p: &Path) -> io::Result<WipeHandle> {
    create(
        &to_wide(p),
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ,
        CREATE_NEW,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH,
    )
}

/// Size of the stream behind an open handle, in bytes.
pub fn file_size(h: &WipeHandle) -> io::Result<u64> {
    let mut size: i64 = 0;
    // SAFETY: h.0 is a valid open handle; `size` is a valid out-pointer.
    let ok = unsafe { GetFileSizeEx(h.0, &mut size) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(size as u64)
}

/// Move the file pointer to an absolute offset from the start.
pub fn seek_begin(h: &WipeHandle, pos: u64) -> io::Result<()> {
    // SAFETY: valid handle; a null new-position out-pointer is allowed.
    let ok = unsafe { SetFilePointerEx(h.0, pos as i64, ptr::null_mut(), FILE_BEGIN) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Write the whole buffer, looping over partial writes.
pub fn write_all(h: &WipeHandle, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let to_write = buf.len().min(1 << 30) as u32;
        let mut written: u32 = 0;
        // SAFETY: `buf` is valid for `to_write` bytes; `written` is a valid
        // out-pointer; the handle is synchronous so no OVERLAPPED is needed.
        let ok = unsafe { WriteFile(h.0, buf.as_ptr(), to_write, &mut written, ptr::null_mut()) };
        if ok == 0 {
            return Err(last_error());
        }
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "WriteFile wrote 0 bytes"));
        }
        buf = &buf[written as usize..];
    }
    Ok(())
}

/// Read up to `buf.len()` bytes; returns how many were read (0 at EOF).
pub fn read_up_to(h: &WipeHandle, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0usize;
    while total < buf.len() {
        let to_read = (buf.len() - total).min(1 << 30) as u32;
        let mut read: u32 = 0;
        // SAFETY: the destination slice is valid and writable for `to_read`
        // bytes; `read` is a valid out-pointer; synchronous handle.
        let ok = unsafe { ReadFile(h.0, buf[total..].as_mut_ptr(), to_read, &mut read, ptr::null_mut()) };
        if ok == 0 {
            return Err(last_error());
        }
        if read == 0 {
            break;
        }
        total += read as usize;
    }
    Ok(total)
}

/// Exactly one `ReadFile` call. With `FILE_FLAG_NO_BUFFERING` every read must
/// start at a sector-aligned offset, so after a short read (end of file) the
/// caller must stop instead of reading again from the unaligned position —
/// doing that is what returns `ERROR_INVALID_PARAMETER` (87).
pub fn read_once(h: &WipeHandle, buf: &mut [u8]) -> io::Result<usize> {
    let to_read = buf.len().min(1 << 30) as u32;
    let mut read: u32 = 0;
    // SAFETY: `buf` is valid and writable for `to_read` bytes; `read` is a
    // valid out-pointer; synchronous handle, so no OVERLAPPED.
    let ok = unsafe { ReadFile(h.0, buf.as_mut_ptr(), to_read, &mut read, ptr::null_mut()) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(read as usize)
}

/// Flush the OS write cache for this handle down to the device.
pub fn flush(h: &WipeHandle) -> io::Result<()> {
    // SAFETY: valid handle opened with write access.
    let ok = unsafe { FlushFileBuffers(h.0) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Truncate the stream to zero bytes.
pub fn truncate_zero(h: &WipeHandle) -> io::Result<()> {
    set_end(h, 0)
}

/// Set the end of the stream to `pos` (shrinks or extends).
pub fn set_end(h: &WipeHandle, pos: u64) -> io::Result<()> {
    seek_begin(h, pos)?;
    // SAFETY: valid handle opened with write access; the pointer is at `pos`.
    let ok = unsafe { SetEndOfFile(h.0) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Bytes the file system has reserved for the stream (whole clusters).
pub fn alloc_size(h: &WipeHandle) -> io::Result<u64> {
    // SAFETY: FILE_STANDARD_INFO is plain old data; all-zero is valid.
    let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: valid handle; `info` is a writable buffer of the size passed.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.0,
            FileStandardInfo,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(info.AllocationSize.max(0) as u64)
}

/// The three timestamps of a file, saved so they can be put back.
pub struct SavedTimes([FILETIME; 3]);

/// Read the timestamps and tell the file system not to update them for
/// writes through this handle (`SetFileTime` with 0xFFFFFFFF).
pub fn freeze_times(h: &WipeHandle) -> io::Result<SavedTimes> {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut t = [z, z, z];
    let [c, a, w] = &mut t;
    // SAFETY: valid handle; three valid FILETIME out-pointers.
    let ok = unsafe { GetFileTime(h.0, c, a, w) };
    if ok == 0 {
        return Err(last_error());
    }
    let keep = FILETIME { dwLowDateTime: u32::MAX, dwHighDateTime: u32::MAX };
    // SAFETY: valid handle; the pointers live for the call. The all-ones
    // value is the documented "do not update" marker; failure is harmless
    // because the times are restored explicitly afterwards anyway.
    unsafe { SetFileTime(h.0, ptr::null(), &keep, &keep) };
    Ok(SavedTimes(t))
}

/// Put saved timestamps back.
pub fn restore_times(h: &WipeHandle, t: &SavedTimes) -> io::Result<()> {
    let [c, a, w] = &t.0;
    // SAFETY: valid handle opened with write access; pointers live for the call.
    let ok = unsafe { SetFileTime(h.0, c, a, w) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Raw attribute bits of a path (`FILE_ATTRIBUTE_*`).
pub fn attributes(path: &Path) -> io::Result<u32> {
    let w = to_wide(path);
    // SAFETY: `w` is a NUL-terminated wide path.
    let a = unsafe { GetFileAttributesW(w.as_ptr()) };
    if a == INVALID_FILE_ATTRIBUTES {
        return Err(last_error());
    }
    Ok(a)
}

/// Set raw attribute bits.
pub fn set_attributes(path: &Path, attrs: u32) -> io::Result<()> {
    let w = to_wide(path);
    // SAFETY: `w` is a NUL-terminated wide path; `attrs` came from `attributes`.
    if unsafe { SetFileAttributesW(w.as_ptr(), attrs) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Clear read-only / hidden / system attributes so the file can be rewritten.
pub fn clear_attributes(path: &Path) -> io::Result<()> {
    let w = to_wide(path);
    // SAFETY: `w` is a NUL-terminated wide path.
    let attrs = unsafe { GetFileAttributesW(w.as_ptr()) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return Err(last_error());
    }
    let cleared = attrs & !(FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
    if cleared != attrs {
        // SAFETY: as above; `cleared` is a valid attribute mask.
        let ok = unsafe { SetFileAttributesW(w.as_ptr(), cleared) };
        if ok == 0 {
            return Err(last_error());
        }
    }
    Ok(())
}

/// Rename via `MoveFileExW` (write-through, replacing any existing target).
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    let (f, t) = (to_wide(from), to_wide(to));
    // SAFETY: both are NUL-terminated wide paths valid for the call.
    let ok = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Delete a file.
pub fn delete_file(p: &Path) -> io::Result<()> {
    let w = to_wide(p);
    // SAFETY: NUL-terminated wide path.
    let ok = unsafe { DeleteFileW(w.as_ptr()) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// `ERROR_DISK_FULL` / `ERROR_HANDLE_DISK_FULL`.
pub fn is_disk_full(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_DISK_FULL) | Some(ERROR_HANDLE_DISK_FULL))
}

/// Another process has the file open (`ERROR_SHARING_VIOLATION` / `ERROR_LOCK_VIOLATION`).
pub fn is_locked(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33))
}

/// The file system refuses to grow this one file any further.
pub fn is_file_too_large(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_FILE_TOO_LARGE))
}

// --- streams -----------------------------------------------------------------

/// One stream of a file.
pub struct StreamInfo {
    pub open_name: OpenName,
    pub size: u64,
    /// `::$DATA` for the default stream, `:name:$DATA` for an ADS.
    pub display: String,
    pub is_default: bool,
}

/// Enumerate the default data stream and all alternate data streams. Falls
/// back to the default stream alone where enumeration is unsupported (FAT,
/// exFAT, network shares).
pub fn list_streams(path: &Path) -> io::Result<Vec<StreamInfo>> {
    let base: Vec<u16> = {
        let mut w = to_wide(path);
        w.pop();
        w
    };
    let w = to_wide(path);
    // SAFETY: WIN32_FIND_STREAM_DATA is plain data; all-zero is a valid value.
    let mut data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };

    // SAFETY: `w` is NUL-terminated; `data` is a valid out-buffer of the
    // type FindStreamInfoStandard expects; flags must be 0.
    let hfind = unsafe { FindFirstStreamW(w.as_ptr(), FindStreamInfoStandard, &mut data as *mut _ as *mut c_void, 0) };
    if hfind == INVALID_HANDLE_VALUE {
        let h = open_rw(&w)?;
        let size = file_size(&h)?;
        return Ok(vec![StreamInfo { open_name: w, size, display: "::$DATA".into(), is_default: true }]);
    }

    let mut out = Vec::new();
    loop {
        let name = from_wide_buf(&data.cStreamName);
        let is_default = name.eq_ignore_ascii_case("::$DATA");
        let open_name = if is_default {
            w.clone()
        } else {
            let mut n = base.clone();
            n.extend(std::ffi::OsStr::new(&name).encode_wide());
            n.push(0);
            n
        };
        out.push(StreamInfo { open_name, size: data.StreamSize as u64, display: name, is_default });
        // SAFETY: hfind is the live find handle; `data` is a valid out-buffer.
        let ok = unsafe { FindNextStreamW(hfind, &mut data as *mut _ as *mut c_void) };
        if ok == 0 {
            break;
        }
    }
    // SAFETY: hfind came from a successful FindFirstStreamW; closed once.
    unsafe {
        FindClose(hfind);
    }
    // Default stream first so progress output reads naturally.
    out.sort_by_key(|s| !s.is_default);
    Ok(out)
}

// --- volumes & media ---------------------------------------------------------

/// Raw facts about the volume holding a path.
#[derive(Clone, Debug, Default)]
pub struct VolumeRaw {
    /// Mount root, e.g. `C:\` or `C:\mnt\data\`.
    pub root: PathBuf,
    /// File-system name as reported by Windows (`NTFS`, `FAT32`, `exFAT`, `ReFS`, …).
    pub fs_name: String,
    pub label: String,
    pub total: u64,
    /// Bytes available to the caller (respects quotas).
    pub free: u64,
    pub cluster_size: u32,
}

/// Volume root (`C:\`) for any path on it.
pub fn volume_root(p: &Path) -> io::Result<PathBuf> {
    let w = to_wide(p);
    let mut buf = [0u16; 1024];
    // SAFETY: `w` is NUL-terminated; `buf` is writable for its stated length.
    let ok = unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(PathBuf::from(crate::util::strip_verbatim(&from_wide_buf(&buf))))
}

pub fn volume_raw(p: &Path) -> io::Result<VolumeRaw> {
    let root = volume_root(p)?;
    let rw = to_wide(&root);

    let mut label = [0u16; 261];
    let mut fsname = [0u16; 261];
    let (mut serial, mut maxcomp, mut flags) = (0u32, 0u32, 0u32);
    // SAFETY: `rw` is NUL-terminated; both buffers are writable for their
    // stated lengths; the u32 out-pointers are valid.
    let ok = unsafe {
        GetVolumeInformationW(
            rw.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &mut serial,
            &mut maxcomp,
            &mut flags,
            fsname.as_mut_ptr(),
            fsname.len() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }

    let (mut avail, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `rw` is NUL-terminated; the three out-pointers are valid.
    let ok = unsafe { GetDiskFreeSpaceExW(rw.as_ptr(), &mut avail, &mut total, &mut total_free) };
    if ok == 0 {
        return Err(last_error());
    }

    let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: as above; failure is tolerated (cluster size stays 0).
    let cluster_size = if unsafe { GetDiskFreeSpaceW(rw.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) } != 0 {
        spc.saturating_mul(bps)
    } else {
        0
    };

    Ok(VolumeRaw {
        root,
        fs_name: from_wide_buf(&fsname),
        label: from_wide_buf(&label),
        total,
        free: avail,
        cluster_size,
    })
}

/// Roots of all lettered drives (`A:\` … `Z:\`).
pub fn list_volumes() -> Vec<PathBuf> {
    // SAFETY: no arguments; returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
}

/// `GetDriveTypeW` mapped to a small enum.
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

/// Raw medium facts behind a volume.
#[derive(Clone, Debug, Default)]
pub struct MediaRaw {
    pub drive: DriveKind,
    pub seek_penalty: Option<bool>,
    pub trim: Option<bool>,
    pub removable_media: Option<bool>,
    /// `STORAGE_BUS_TYPE` value.
    pub bus: Option<u32>,
    pub model: Option<String>,
}

pub fn media_raw(p: &Path) -> MediaRaw {
    let Ok(root) = volume_root(p) else { return MediaRaw::default() };
    let rw = to_wide(&root);
    // SAFETY: NUL-terminated wide root path.
    let drive = match unsafe { GetDriveTypeW(rw.as_ptr()) } {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Remote,
        5 => DriveKind::CdRom,
        6 => DriveKind::RamDisk,
        _ => DriveKind::Unknown,
    };
    let mut out = MediaRaw { drive, ..Default::default() };

    // IOCTLs need a drive letter: \\.\X:
    let s = root.to_string_lossy();
    let b = s.as_bytes();
    if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() || drive == DriveKind::Remote {
        return out;
    }
    let dev = wide(&format!(r"\\.\{}:", b[0] as char));
    let Ok(h) = create(&dev, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING, 0) else {
        return out;
    };

    if let Some(buf) = query_property(&h, STORAGE_DEVICE_SEEK_PENALTY_PROPERTY) {
        out.seek_penalty = buf.get(8).map(|&v| v != 0);
    }
    if let Some(buf) = query_property(&h, STORAGE_DEVICE_TRIM_PROPERTY) {
        out.trim = buf.get(8).map(|&v| v != 0);
    }
    if let Some(buf) = query_property(&h, STORAGE_DEVICE_PROPERTY) {
        // STORAGE_DEVICE_DESCRIPTOR: RemovableMedia @10 (BOOLEAN),
        // ProductIdOffset @16 (u32), BusType @28 (u32).
        let u32_at = |o: usize| buf.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
        out.removable_media = buf.get(10).map(|&v| v != 0);
        out.bus = u32_at(28);
        if let Some(off) = u32_at(16).map(|o| o as usize).filter(|&o| o > 0 && o < buf.len()) {
            let end = buf[off..].iter().position(|&c| c == 0).map(|e| off + e).unwrap_or(buf.len());
            let m = String::from_utf8_lossy(&buf[off..end]).trim().to_string();
            if !m.is_empty() {
                out.model = Some(m);
            }
        }
    }
    out
}

/// `IOCTL_STORAGE_QUERY_PROPERTY` (standard query) into a byte buffer.
fn query_property(h: &WipeHandle, property_id: u32) -> Option<Vec<u8>> {
    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional: [u8; 4],
    }
    let q = StoragePropertyQuery { property_id, query_type: PROPERTY_STANDARD_QUERY, additional: [0; 4] };
    let mut buf = vec![0u8; 1024];
    let mut returned: u32 = 0;
    // SAFETY: valid device handle; input points at a properly laid-out query
    // of the stated size; output buffer is writable for its length.
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &q as *const _ as *const c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 || returned == 0 {
        return None;
    }
    buf.truncate(returned as usize);
    Some(buf)
}

// --- registry ----------------------------------------------------------------

/// Registry hive used for the Explorer integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegRoot {
    /// `HKEY_CURRENT_USER` — no admin rights needed.
    CurrentUser,
    /// `HKEY_LOCAL_MACHINE` — all users, needs elevation.
    LocalMachine,
}

impl RegRoot {
    fn hkey(self) -> HKEY {
        match self {
            RegRoot::CurrentUser => HKEY_CURRENT_USER,
            RegRoot::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// An open registry key, closed on drop.
pub struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: self.0 was returned by RegCreateKeyExW/RegOpenKeyExW and is
        // closed exactly once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// Create (or open) `subkey` for writing.
pub fn reg_create(root: RegRoot, subkey: &str) -> io::Result<RegKey> {
    let w = wide(subkey);
    let mut hk: HKEY = ptr::null_mut();
    // SAFETY: `w` is NUL-terminated; class may be null; `hk` is a valid
    // out-pointer; disposition out-pointer may be null.
    let rc = unsafe {
        RegCreateKeyExW(
            root.hkey(),
            w.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE | KEY_READ,
            ptr::null(),
            &mut hk,
            ptr::null_mut(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    Ok(RegKey(hk))
}

/// Open `subkey` for reading; `Ok(None)` if it does not exist.
pub fn reg_open(root: RegRoot, subkey: &str) -> io::Result<Option<RegKey>> {
    let w = wide(subkey);
    let mut hk: HKEY = ptr::null_mut();
    // SAFETY: `w` is NUL-terminated; `hk` is a valid out-pointer.
    let rc = unsafe { RegOpenKeyExW(root.hkey(), w.as_ptr(), 0, KEY_READ, &mut hk) };
    if rc == ERROR_FILE_NOT_FOUND as u32 {
        return Ok(None);
    }
    if rc != ERROR_SUCCESS {
        return Err(win_err(rc));
    }
    Ok(Some(RegKey(hk)))
}

/// Delete `subkey` and everything below it. Missing keys are not an error.
pub fn reg_delete_tree(root: RegRoot, subkey: &str) -> io::Result<()> {
    let w = wide(subkey);
    // SAFETY: `w` is NUL-terminated.
    let rc = unsafe { RegDeleteTreeW(root.hkey(), w.as_ptr()) };
    if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND as u32 {
        Ok(())
    } else {
        Err(win_err(rc))
    }
}

impl RegKey {
    /// Set a `REG_SZ` value (`None` = the key's default value).
    pub fn set_string(&self, name: Option<&str>, value: &str) -> io::Result<()> {
        let n = name.map(wide);
        let v = wide(value);
        let name_ptr = n.as_ref().map_or(ptr::null(), |n| n.as_ptr());
        // SAFETY: name is NUL-terminated or null (default value); data points
        // at `v` and its byte length includes the terminating NUL.
        let rc = unsafe { RegSetValueExW(self.0, name_ptr, 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) };
        if rc != ERROR_SUCCESS {
            return Err(win_err(rc));
        }
        Ok(())
    }

    /// Read a string value; `None` if absent or not a string.
    pub fn get_string(&self, name: Option<&str>) -> Option<String> {
        let n = name.map(wide);
        let name_ptr = n.as_ref().map_or(ptr::null(), |n| n.as_ptr());
        let mut ty: u32 = 0;
        let mut buf = vec![0u16; 2048];
        let mut cb: u32 = (buf.len() * 2) as u32;
        // SAFETY: name NUL-terminated or null; buffer writable for `cb` bytes;
        // type and size out-pointers valid.
        let rc =
            unsafe { RegQueryValueExW(self.0, name_ptr, ptr::null(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut cb) };
        if rc != ERROR_SUCCESS || (ty != REG_SZ && ty != REG_EXPAND_SZ) {
            return None;
        }
        buf.truncate((cb as usize) / 2);
        Some(from_wide_buf(&buf))
    }

    /// Names of the direct sub-keys.
    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            // SAFETY: name buffer writable for `len` chars; optional
            // out-pointers are null.
            let rc = unsafe {
                RegEnumKeyExW(
                    self.0,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if rc == ERROR_NO_MORE_ITEMS || rc != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        out
    }
}

/// Tell Explorer that verbs/associations changed so menus refresh.
pub fn notify_assoc_changed() {
    // SAFETY: SHCNE_ASSOCCHANGED takes no items; both item pointers null.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, ptr::null(), ptr::null()) };
}

// --- processes, elevation, single instance, power ----------------------------

/// Run `exe params` elevated (UAC prompt) and wait for it. Returns its exit
/// code. A declined prompt yields `ERROR_CANCELLED` (1223).
pub fn run_elevated(exe: &Path, params: &str) -> io::Result<u32> {
    let verb = wide("runas");
    let file = to_wide(exe);
    let par = wide(params);
    // SAFETY: SHELLEXECUTEINFOW is plain data; all-zero is valid before we
    // fill the fields we use.
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = par.as_ptr();
    sei.nShow = SW_HIDE;
    // SAFETY: `sei` is fully initialised; the string buffers outlive the call.
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        return Err(last_error());
    }
    if sei.hProcess.is_null() {
        return Ok(0);
    }
    let mut code: u32 = 0;
    // SAFETY: hProcess is a live process handle returned because of
    // SEE_MASK_NOCLOSEPROCESS; we wait, read the exit code and close it once.
    unsafe {
        WaitForSingleObject(sei.hProcess, INFINITE);
        GetExitCodeProcess(sei.hProcess, &mut code);
        CloseHandle(sei.hProcess);
    }
    Ok(code)
}

/// Start `exe params` elevated (UAC prompt) without waiting for it.
/// A declined prompt yields `ERROR_CANCELLED` (1223).
pub fn launch_elevated(exe: &Path, params: &str) -> io::Result<()> {
    let verb = wide("runas");
    let file = to_wide(exe);
    let par = wide(params);
    // SAFETY: plain data; all-zero is valid before filling the used fields.
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = par.as_ptr();
    sei.nShow = SW_SHOWNORMAL;
    // SAFETY: `sei` is fully initialised; the string buffers outlive the call.
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Is this process running with administrator rights (elevated token)?
pub fn is_elevated() -> bool {
    // SAFETY: no arguments; returns a BOOL.
    unsafe { IsUserAnAdmin() != 0 }
}

/// Holds a named mutex for the lifetime of the process.
pub struct InstanceGuard(HANDLE);

// SAFETY: the mutex handle is only closed on drop; moving it is fine.
unsafe impl Send for InstanceGuard {}

impl InstanceGuard {
    /// A guard that holds nothing (used when the mutex is unavailable).
    pub fn none() -> Self {
        InstanceGuard(ptr::null_mut())
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: valid mutex handle owned by us, closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// `Some(guard)` if this is the first instance, `None` if another one runs.
pub fn acquire_single_instance(name: &str) -> io::Result<Option<InstanceGuard>> {
    let w = wide(name);
    // SAFETY: null security attributes are allowed; name is NUL-terminated.
    let h = unsafe { CreateMutexW(ptr::null(), 0, w.as_ptr()) };
    if h.is_null() {
        // A mutex created by an *elevated* instance cannot be opened from a
        // normal one: that also means "another instance is running".
        // SAFETY: reads thread-local state set by CreateMutexW just above.
        if unsafe { GetLastError() } == ERROR_ACCESS_DENIED_U32 {
            return Ok(None);
        }
        return Err(last_error());
    }
    // SAFETY: reads thread-local state set by CreateMutexW just above.
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let guard = InstanceGuard(h);
    Ok(if already { None } else { Some(guard) })
}

/// Keep the machine awake while the *calling thread* works (`true`), or
/// release that request (`false`).
pub fn keep_awake(on: bool) {
    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    // SAFETY: plain flag call with valid flags.
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// String SID of the current user (`S-1-5-21-…`), used to find the user's
/// own folder inside `X:\$Recycle.Bin`.
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is a valid
    // out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = WipeHandle(token); // closes on drop

    let mut buf = vec![0u8; 256];
    let mut len: u32 = 0;
    // SAFETY: valid token handle; buffer writable for its length; `len` is a
    // valid out-pointer. Retried once with the size Windows asks for.
    let mut ok = unsafe {
        GetTokenInformation(token.0, TOKEN_USER_CLASS, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut len)
    };
    if ok == 0 && len as usize > buf.len() {
        buf.resize(len as usize, 0);
        // SAFETY: same as above with the enlarged buffer.
        ok = unsafe {
            GetTokenInformation(token.0, TOKEN_USER_CLASS, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut len)
        };
    }
    if ok == 0 {
        return Err(last_error());
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER whose Sid points
    // inside the same buffer, which is alive for the rest of this function.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let mut s: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` is valid (see above); `s` receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(sid, &mut s) } == 0 {
        return Err(last_error());
    }
    // SAFETY: `s` is a NUL-terminated wide string allocated by the call; we
    // measure it, copy it, then free it exactly once with LocalFree.
    let out = unsafe {
        let mut n = 0usize;
        while *s.add(n) != 0 {
            n += 1;
        }
        let v = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
        LocalFree(s as *mut c_void);
        v
    };
    Ok(out)
}

// --- who holds a file open (Restart Manager) -----------------------------------

/// A process that has one of the files open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locker {
    pub pid: u32,
    pub name: String,
    /// A normal windowed application that can be asked to close.
    pub closable: bool,
}

/// Restart Manager session, ended on drop.
struct RmSession(u32);

impl Drop for RmSession {
    fn drop(&mut self) {
        // SAFETY: the handle came from RmStartSession and is ended once.
        unsafe { RmEndSession(self.0) };
    }
}

fn rm_session(paths: &[PathBuf]) -> io::Result<RmSession> {
    let mut handle = 0u32;
    let mut key = [0u16; 33]; // CCH_RM_SESSION_KEY + 1
                              // SAFETY: valid out-pointer and a key buffer of the documented size.
    let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let session = RmSession(handle);
    let wides: Vec<Vec<u16>> = paths.iter().map(|p| to_wide(p)).collect();
    let ptrs: Vec<*const u16> = wides.iter().map(|w| w.as_ptr()).collect();
    // SAFETY: `ptrs` holds `ptrs.len()` NUL-terminated strings kept alive by
    // `wides` for the duration of the call; no applications/services passed.
    let rc =
        unsafe { RmRegisterResources(session.0, ptrs.len() as u32, ptrs.as_ptr(), 0, ptr::null(), 0, ptr::null()) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    Ok(session)
}

fn rm_list(session: &RmSession) -> io::Result<Vec<RM_PROCESS_INFO>> {
    const ERROR_MORE_DATA: u32 = 234;
    let mut cap = 8u32;
    loop {
        // SAFETY: RM_PROCESS_INFO is plain old data; all-zero is valid.
        let mut buf: Vec<RM_PROCESS_INFO> = (0..cap).map(|_| unsafe { std::mem::zeroed() }).collect();
        let (mut needed, mut count, mut reasons) = (0u32, cap, 0u32);
        // SAFETY: `buf` has room for `count` entries; out-pointers are valid.
        let rc = unsafe { RmGetList(session.0, &mut needed, &mut count, buf.as_mut_ptr(), &mut reasons) };
        if rc == ERROR_MORE_DATA && needed > cap {
            cap = needed + 4;
            continue;
        }
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        buf.truncate(count as usize);
        return Ok(buf);
    }
}

/// Processes that currently have any of `paths` open.
pub fn lockers(paths: &[PathBuf]) -> io::Result<Vec<Locker>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let session = rm_session(paths)?;
    let mut out: Vec<Locker> = Vec::new();
    for info in rm_list(&session)? {
        let end = info.strAppName.iter().position(|&c| c == 0).unwrap_or(info.strAppName.len());
        let name = String::from_utf16_lossy(&info.strAppName[..end]);
        // RmMainWindow = 1, RmOtherWindow = 2.
        let closable = matches!(info.ApplicationType, 1 | 2);
        let l = Locker { pid: info.Process.dwProcessId, name, closable };
        if !out.contains(&l) {
            out.push(l);
        }
    }
    Ok(out)
}

/// Ask the applications holding `paths` to close (like Windows Update does:
/// they get the chance to save). Fails if any of them cannot be closed.
pub fn close_lockers(paths: &[PathBuf]) -> io::Result<()> {
    let session = rm_session(paths)?;
    let _ = rm_list(&session)?;
    // SAFETY: valid session; no flags (graceful) and no callback.
    let rc = unsafe { RmShutdown(session.0, 0, None) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    Ok(())
}

/// Decode console output of Windows tools (OEM code page, e.g. CP866).
pub fn oem_to_string(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP};
    if bytes.is_empty() {
        return String::new();
    }
    let len = bytes.len().min(i32::MAX as usize) as i32;
    // SAFETY: `bytes` is valid for `len` bytes; the first call only measures.
    let n = unsafe { MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, ptr::null_mut(), 0) };
    if n <= 0 {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let mut w = vec![0u16; n as usize];
    // SAFETY: `w` has room for exactly `n` UTF-16 units as measured above.
    let n = unsafe { MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, w.as_mut_ptr(), n) };
    String::from_utf16_lossy(&w[..n.max(0) as usize])
}
