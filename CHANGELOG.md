# Changelog

All notable changes to freewipe are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- Text in the desktop app is no longer selectable, like in other Windows programs. The log, error messages, report paths and the "files in use" details can still be selected and copied.
- Buttons react to the mouse: they change colour on hover and when pressed, and the pointer turns into a hand.
- *About → Support the project* links to the support section of the project page.

### Fixed
- With *Use a single pass on SSD / flash*, the progress bar stopped at about 50% and the confirmation showed the full method's total: totals, the pass list and the confirmation now reflect the single pass used on SSD/flash.

## [1.0.0] - 2026-10-31

First public release.

### Wiping engine
- Wipe files, folders (recursively), the free space of a volume and the Recycle Bin.
- 19 wiping methods, from a single NIST SP 800-88 Clear pass to Gutmann's 35 passes,
  plus custom methods of up to 99 passes (zeros, ones, a byte, a hex pattern or random data).
- Verification reads the last pass back from the disk (unbuffered I/O, SHA-256).
- Every NTFS alternate data stream is overwritten; read-only, hidden and system files are handled;
  files are renamed several times and truncated before deletion.
- Symbolic links and junctions are removed as links — their targets are never overwritten.
  Shortcuts (`.lnk`) are wiped as files, not their targets.
- Free space: file-system-aware chunk files (FAT32 4 GiB limit), progressive fill,
  guaranteed cleanup on errors and cancellation.
- Free-space extras: **cluster tips** (the unused end of each file's last cluster, with a repair
  journal for interrupted runs) and **NTFS file records** (free MFT records).
- Optional single pass on SSD and flash media, where more passes only add time and wear.
- File-system detection (NTFS, FAT, exFAT, ReFS, ext, btrfs and others) and SSD / NVMe / USB
  detection, with honest warnings about what overwriting cannot guarantee.
- Folder filters: include and exclude masks, "keep the folder itself".
- **Move & wipe original**: copies to another drive, verifies every file by SHA-256,
  and only then wipes the originals.
- Files in use: shows which program holds the file and offers *Close it and retry*,
  *Try again* or *Wipe at next sign-in*.

### Safety
- Protected locations (Windows, Program Files, the user profile, drive roots, freewipe itself),
  editable in the settings; `--force` / *Allow protected targets* is required to bypass them.
- Confirmation styles: buttons, a typed word, or none.
- Optional password lock (PBKDF2-SHA256) when freewipe starts, before Settings and/or before every
  wipe, with an optional machine-wide lock for all users. Wiping CLI commands honour it.
- *Retry as administrator* for items that failed with "access denied".

### Desktop app
- Queue with drag and drop, confirmation, progress with pause and stop.
- Pages for free space, methods, reports, schedules and settings; layouts adapt to narrow windows.
- Notification-area icon while a wipe runs, with a notification when it ends.
- **Scheduled wipes** through the Windows Task Scheduler: free space, files in a folder (with masks)
  or the Recycle Bin — daily, weekly, monthly or at sign-in.
- Reports in JSON: open, delete, or wipe them; optional redaction of paths.
- About page with links to the project and to donations.

### Explorer integration
- Context-menu items for files, folders, drives and the Recycle Bin, served by a shell extension
  (`freewipe_shell.dll`, `IExplorerCommand`): the whole selection arrives at once, with no
  "Open File – Security Warning" for downloaded files.
- Submenu or flat items, optional Shift-only mode, current user or all users, `.reg` export.
- "Wipe permanently" from Explorer opens a small progress window or adds the items to the queue.
- Menu entries from another version or another copy of freewipe are detected and can be updated.

### Command line
- Commands `file`, `dir`, `freespace`, `recycle-bin`, `move`, `methods`, `languages`, `shell`.
- `--dry-run`, JSON reports (`--log`), `--include` / `--exclude` / `--contents-only`,
  `--cluster-tips`, `--file-records`, `--ssd-single-pass`; Ctrl+C stops safely.

### Other
- English built in; German, Spanish, French, Polish, Russian and Ukrainian language packs.
  New languages can be added as JSON files without rebuilding.
- Optional debug log for troubleshooting (passwords are never written).
- No network access, no telemetry.
- The engine is usable as a Rust library (`app::set_identity`, `i18n::register_english`,
  `wipe::wipe_dir_contents`).

[Unreleased]: https://github.com/sleepme/freewipe/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/sleepme/freewipe/releases/tag/v1.0.0
