//! Storage-medium classification.
//!
//! Used to warn loudly (never to block) when a target lives on flash media,
//! where overwriting logical blocks does not guarantee the old data is
//! physically gone: SSD controllers remap writes (wear-levelling) and keep
//! spare blocks the OS cannot address.
//!
//! The raw facts come from the backend: drive type, the "seek penalty"
//! property (no penalty ⇒ solid state), TRIM support, bus type and model.

use std::path::Path;

use crate::sys::{self, DriveKind, MediaRaw};
use crate::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Hdd,
    Ssd,
    /// USB sticks, SD/MMC cards and other removable flash.
    Flash,
    Network,
    Optical,
    Ram,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub trim: Option<bool>,
    pub bus: Option<String>,
    pub model: Option<String>,
}

/// `STORAGE_BUS_TYPE` → short name.
fn bus_name(b: u32) -> Option<&'static str> {
    Some(match b {
        1 => "SCSI",
        2 => "ATAPI",
        3 => "ATA",
        4 => "IEEE1394",
        5 => "SSA",
        6 => "Fibre",
        7 => "USB",
        8 => "RAID",
        9 => "iSCSI",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        14 | 15 => "Virtual",
        16 => "Spaces",
        17 => "NVMe",
        18 => "SCM",
        19 => "UFS",
        _ => return None,
    })
}

/// Pure classification of raw facts (unit-tested).
pub fn classify(raw: &MediaRaw) -> MediaInfo {
    let bus = raw.bus.and_then(bus_name).map(str::to_string);
    let flash_bus = matches!(raw.bus, Some(7) | Some(12) | Some(13));
    let ssd_bus = matches!(raw.bus, Some(17) | Some(18) | Some(19));
    let kind = match raw.drive {
        DriveKind::Remote => MediaKind::Network,
        DriveKind::CdRom => MediaKind::Optical,
        DriveKind::RamDisk => MediaKind::Ram,
        _ if raw.seek_penalty == Some(true) && raw.drive != DriveKind::Removable => MediaKind::Hdd,
        _ if flash_bus || raw.drive == DriveKind::Removable => MediaKind::Flash,
        _ if raw.seek_penalty == Some(false) || ssd_bus => MediaKind::Ssd,
        _ if raw.seek_penalty == Some(true) => MediaKind::Hdd,
        _ => MediaKind::Unknown,
    };
    MediaInfo { kind, trim: raw.trim, bus, model: raw.model.clone() }
}

/// Probe the medium behind `path`. Best-effort: unknown facts stay unknown.
pub fn probe(path: &Path) -> MediaInfo {
    classify(&sys::media_raw(path))
}

impl MediaInfo {
    /// Whether the target is (likely) on flash memory.
    pub fn is_flash_likely(&self) -> bool {
        matches!(self.kind, MediaKind::Ssd | MediaKind::Flash)
    }

    /// Short label: `"SSD (NVMe)"`, `"HDD"`, `"USB flash"`, …
    pub fn label(&self) -> String {
        let base = match self.kind {
            MediaKind::Hdd => "HDD".to_string(),
            MediaKind::Ssd => "SSD".to_string(),
            MediaKind::Flash => tr!("media.flash"),
            MediaKind::Network => tr!("media.network"),
            MediaKind::Optical => tr!("media.optical"),
            MediaKind::Ram => "RAM".to_string(),
            MediaKind::Unknown => tr!("media.unknown"),
        };
        match (&self.bus, self.kind) {
            (Some(b), MediaKind::Ssd | MediaKind::Hdd | MediaKind::Flash) => format!("{base} ({b})"),
            _ => base,
        }
    }

    /// A localised warning when overwriting cannot be trusted on this medium.
    pub fn warning(&self) -> Option<String> {
        match self.kind {
            MediaKind::Ssd | MediaKind::Flash => {
                let mut w = tr!("media.warn_flash");
                if self.trim == Some(true) {
                    w.push(' ');
                    w.push_str(&tr!("media.warn_trim"));
                }
                Some(w)
            }
            MediaKind::Network => Some(tr!("media.warn_network")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(drive: DriveKind, seek: Option<bool>, bus: Option<u32>) -> MediaRaw {
        MediaRaw { drive, seek_penalty: seek, bus, ..Default::default() }
    }

    #[test]
    fn classification() {
        assert_eq!(classify(&raw(DriveKind::Fixed, Some(true), Some(11))).kind, MediaKind::Hdd);
        assert_eq!(classify(&raw(DriveKind::Fixed, Some(false), Some(11))).kind, MediaKind::Ssd);
        assert_eq!(classify(&raw(DriveKind::Fixed, None, Some(17))).kind, MediaKind::Ssd);
        assert_eq!(classify(&raw(DriveKind::Removable, None, Some(7))).kind, MediaKind::Flash);
        // External USB hard disk still has a seek penalty.
        assert_eq!(classify(&raw(DriveKind::Fixed, Some(true), Some(7))).kind, MediaKind::Hdd);
        assert_eq!(classify(&raw(DriveKind::Remote, None, None)).kind, MediaKind::Network);
        assert_eq!(classify(&raw(DriveKind::Unknown, None, None)).kind, MediaKind::Unknown);
    }

    #[test]
    fn flash_warns() {
        assert!(classify(&raw(DriveKind::Fixed, Some(false), None)).warning().is_some());
        assert!(classify(&raw(DriveKind::Fixed, Some(true), None)).warning().is_none());
    }
}
