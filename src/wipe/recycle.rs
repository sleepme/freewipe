//! Wipe the contents of the current user's Recycle Bin (every drive).
//!
//! On Windows each volume keeps deleted items in `X:\$Recycle.Bin\<SID>`
//! (`$R…` = the data, `$I…` = the original name/size/time). Both are wiped
//! like any other file; the per-user folder and its `desktop.ini` stay so
//! Explorer keeps working. On Linux the XDG trash (`~/.local/share/Trash`)
//! is handled the same way.

use crate::plan::recycle_bin_dirs;
use crate::progress::Ctx;

use super::dir::{wipe_dir_contents, DirReport};
use super::{WipeError, WipeOptions};

pub fn wipe_recycle_bin(opts: &WipeOptions, ctx: &mut Ctx) -> Result<DirReport, WipeError> {
    let mut report = DirReport::default();
    // The bin folders are system-managed; the content is the user's own.
    let opts = WipeOptions { force: true, ..opts.clone() };
    for d in recycle_bin_dirs() {
        wipe_dir_contents(&d, &opts, ctx, &mut report, &["desktop.ini"])?;
        if report.cancelled {
            break;
        }
    }
    #[cfg(windows)]
    crate::sys::notify_assoc_changed();
    Ok(report)
}
