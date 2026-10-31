//! File-system detection and the limits that shape how we wipe.
//!
//! What matters per file system:
//! * **maximum file size** — free-space filling splits into chunk files that
//!   never exceed it (FAT32 cannot hold a file ≥ 4 GiB);
//! * **alternate data streams** — only NTFS/ReFS have them, and they must be
//!   wiped separately;
//! * **resident data** — NTFS stores very small files (≈ < 700 bytes)
//!   inside the MFT record itself; overwriting works, but older copies of
//!   that record may survive elsewhere;
//! * **journaling** — metadata (names, sizes) may linger in the journal;
//! * **copy-on-write** — ReFS (with integrity streams / block cloning),
//!   btrfs, ZFS and APFS may write new data to *new* blocks, so in-place
//!   overwriting is not guaranteed to touch the original ones.

use std::io;
use std::path::{Path, PathBuf};

use crate::sys;
use crate::tr;

const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsKind {
    Ntfs,
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    ReFs,
    Ext2,
    Ext3,
    Ext4,
    Btrfs,
    Xfs,
    Zfs,
    Apfs,
    HfsPlus,
    Udf,
    Tmpfs,
    Network(String),
    Other(String),
    Unknown,
}

impl FsKind {
    /// Map a name reported by the OS (`"NTFS"`, `"FAT32"`, `"vfat"`, `"ext4"`, …).
    pub fn from_name(name: &str) -> FsKind {
        let n = name.trim().to_ascii_lowercase();
        match n.as_str() {
            "" => FsKind::Unknown,
            "ntfs" | "ntfs3" => FsKind::Ntfs,
            "fat12" => FsKind::Fat12,
            // Windows reports FAT12/FAT16 both as "FAT"; Linux "msdos" is 8.3 FAT.
            "fat" | "fat16" | "msdos" => FsKind::Fat16,
            // Linux "vfat" is FAT12/16/32 with long names; FAT32 is by far the
            // common case and its 4 GiB limit is the safe assumption.
            "fat32" | "vfat" => FsKind::Fat32,
            "exfat" => FsKind::ExFat,
            "refs" => FsKind::ReFs,
            "ext2" => FsKind::Ext2,
            "ext3" => FsKind::Ext3,
            "ext4" => FsKind::Ext4,
            "btrfs" => FsKind::Btrfs,
            "xfs" => FsKind::Xfs,
            "zfs" => FsKind::Zfs,
            "apfs" => FsKind::Apfs,
            "hfs" | "hfs+" | "hfsplus" => FsKind::HfsPlus,
            "udf" => FsKind::Udf,
            "tmpfs" | "ramfs" => FsKind::Tmpfs,
            "nfs" | "nfs4" | "cifs" | "smb" | "smb2" | "smb3" | "smbfs" | "sshfs" | "fuse.sshfs" | "9p" => {
                FsKind::Network(n)
            }
            _ => FsKind::Other(name.trim().to_string()),
        }
    }

    pub fn display(&self) -> String {
        match self {
            FsKind::Ntfs => "NTFS".into(),
            FsKind::Fat12 => "FAT12".into(),
            FsKind::Fat16 => "FAT16".into(),
            FsKind::Fat32 => "FAT32".into(),
            FsKind::ExFat => "exFAT".into(),
            FsKind::ReFs => "ReFS".into(),
            FsKind::Ext2 => "ext2".into(),
            FsKind::Ext3 => "ext3".into(),
            FsKind::Ext4 => "ext4".into(),
            FsKind::Btrfs => "btrfs".into(),
            FsKind::Xfs => "XFS".into(),
            FsKind::Zfs => "ZFS".into(),
            FsKind::Apfs => "APFS".into(),
            FsKind::HfsPlus => "HFS+".into(),
            FsKind::Udf => "UDF".into(),
            FsKind::Tmpfs => "tmpfs".into(),
            FsKind::Network(n) | FsKind::Other(n) => n.clone(),
            FsKind::Unknown => "?".into(),
        }
    }

    /// Largest single file the file system can hold, if limited in practice.
    pub fn max_file_size(&self) -> Option<u64> {
        match self {
            FsKind::Fat12 | FsKind::Fat16 => Some(2 * GIB - 1),
            FsKind::Fat32 => Some(4 * GIB - 1),
            FsKind::Ntfs | FsKind::Ext3 | FsKind::Ext4 => Some(16 * TIB),
            FsKind::Ext2 => Some(2 * TIB),
            FsKind::HfsPlus => Some(8 * TIB),
            _ => None,
        }
    }

    /// NTFS and ReFS can carry alternate data streams.
    pub fn has_ads(&self) -> bool {
        matches!(self, FsKind::Ntfs | FsKind::ReFs)
    }

    /// Tiny files may live inside metadata records (NTFS MFT, ext4 inline data).
    pub fn small_files_resident(&self) -> bool {
        matches!(self, FsKind::Ntfs | FsKind::Ext4 | FsKind::Btrfs)
    }

    pub fn is_journaled(&self) -> bool {
        matches!(self, FsKind::Ntfs | FsKind::ReFs | FsKind::Ext3 | FsKind::Ext4 | FsKind::Xfs | FsKind::HfsPlus)
    }

    /// New writes may go to new blocks instead of overwriting in place.
    pub fn is_copy_on_write(&self) -> bool {
        matches!(self, FsKind::ReFs | FsKind::Btrfs | FsKind::Zfs | FsKind::Apfs)
    }

    /// Directory entry limit worth respecting when creating many files.
    pub fn max_entries_per_dir(&self) -> Option<u64> {
        match self {
            FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => Some(65_534),
            FsKind::ExFat => Some(2_796_202),
            _ => None,
        }
    }

    /// Size of each fill file when wiping free space: 1 GiB, or less where the
    /// file system caps file size. Always a multiple of 1 MiB.
    pub fn freespace_chunk(&self) -> u64 {
        let cap = self.max_file_size().unwrap_or(u64::MAX);
        let chunk = GIB.min(cap);
        (chunk / (1 << 20)).max(1) * (1 << 20)
    }

    /// Honest, localised notes about what overwriting cannot guarantee here.
    pub fn warnings(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.is_copy_on_write() {
            v.push(tr!("fs.warn_cow", fs = self.display()));
        }
        if self.small_files_resident() {
            v.push(tr!("fs.warn_resident", fs = self.display()));
        }
        if self.is_journaled() {
            v.push(tr!("fs.warn_journal", fs = self.display()));
        }
        if matches!(self, FsKind::Ntfs | FsKind::ReFs) {
            v.push(tr!("fs.warn_vss"));
        }
        if matches!(self, FsKind::Network(_)) {
            v.push(tr!("fs.warn_network"));
        }
        v
    }
}

/// Facts about the volume holding a path.
#[derive(Clone, Debug)]
pub struct VolumeInfo {
    pub root: PathBuf,
    pub fs: FsKind,
    pub fs_name: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub cluster_size: u32,
}

/// Probe the volume holding `path` (which must exist).
pub fn probe(path: &Path) -> io::Result<VolumeInfo> {
    let raw = sys::volume_raw(path)?;
    Ok(VolumeInfo {
        fs: FsKind::from_name(&raw.fs_name),
        root: raw.root,
        fs_name: raw.fs_name,
        label: raw.label,
        total: raw.total,
        free: raw.free,
        cluster_size: raw.cluster_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map() {
        assert_eq!(FsKind::from_name("NTFS"), FsKind::Ntfs);
        assert_eq!(FsKind::from_name("FAT32"), FsKind::Fat32);
        assert_eq!(FsKind::from_name("FAT"), FsKind::Fat16);
        assert_eq!(FsKind::from_name("vfat"), FsKind::Fat32);
        assert_eq!(FsKind::from_name("exFAT"), FsKind::ExFat);
        assert_eq!(FsKind::from_name("ReFS"), FsKind::ReFs);
        assert_eq!(FsKind::from_name("ext4"), FsKind::Ext4);
        assert!(matches!(FsKind::from_name("cifs"), FsKind::Network(_)));
        assert!(matches!(FsKind::from_name("weirdfs"), FsKind::Other(_)));
    }

    #[test]
    fn fat32_chunks_stay_under_4gib() {
        let c = FsKind::Fat32.freespace_chunk();
        assert!(c < 4 * GIB);
        assert_eq!(c % (1 << 20), 0);
        assert_eq!(FsKind::Ntfs.freespace_chunk(), GIB);
        assert!(FsKind::Fat16.freespace_chunk() < 2 * GIB);
    }

    #[test]
    fn properties() {
        assert!(FsKind::Ntfs.has_ads());
        assert!(!FsKind::ExFat.has_ads());
        assert!(FsKind::ReFs.is_copy_on_write());
        assert!(!FsKind::Ntfs.is_copy_on_write());
        assert_eq!(FsKind::Fat32.max_entries_per_dir(), Some(65_534));
    }
}
