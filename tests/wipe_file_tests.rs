//! End-to-end wiping inside temporary folders. Nothing outside the temp
//! directories is touched.

use std::fs;
use std::path::Path;

use freewipe::plan::{self, Target, TargetKind};
use freewipe::progress::{CancelFlag, Ctx, NoProgress, Progress};
use freewipe::report::Status;
use freewipe::wipe::{self, wipe_dir, wipe_file, WipeOptions};
use freewipe::Method;

fn opts(method: Method) -> WipeOptions {
    WipeOptions { method, buffer_size: 64 * 1024, ..Default::default() }
}

fn write(p: &Path, len: usize, byte: u8) {
    fs::write(p, vec![byte; len]).unwrap();
}

/// Counts bytes and passes reported by the engine.
#[derive(Default)]
struct Counter {
    bytes: u64,
    passes: usize,
    items: usize,
}
impl Progress for Counter {
    fn item(&mut self, _: &str) {
        self.items += 1;
    }
    fn pass(&mut self, _: usize, _: usize, _: &str) {
        self.passes += 1;
    }
    fn advance(&mut self, b: u64) {
        self.bytes += b;
    }
}

#[test]
fn file_is_overwritten_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("secret.txt");
    write(&f, 200_000, b'A');

    let cancel = CancelFlag::new();
    let mut c = Counter::default();
    let r = wipe_file(&f, &opts(Method::DodE), &mut Ctx::new(&cancel, &mut c)).unwrap();

    assert!(!f.exists(), "file must be gone");
    assert!(r.deleted);
    assert_eq!(r.passes, 3);
    assert_eq!(r.bytes_overwritten, 3 * 200_000);
    assert_eq!(c.passes, 3);
    assert_eq!(c.bytes, 3 * 200_000);
    // Renamed and removed: the folder is empty, no stray names left.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn keep_overwrites_in_place_with_last_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("keep.bin");
    write(&f, 70_000, b'Z');
    let o = WipeOptions { keep: true, ..opts(Method::One) };
    let r = wipe_file(&f, &o, &mut Ctx::new(&CancelFlag::new(), &mut NoProgress)).unwrap();
    assert!(!r.deleted);
    let data = fs::read(&f).unwrap();
    assert_eq!(data.len(), 70_000, "size must be preserved with --keep");
    assert!(data.iter().all(|&b| b == 0xFF));
}

#[test]
fn verification_passes_for_random_and_patterns() {
    let dir = tempfile::tempdir().unwrap();
    for m in [Method::Random, Method::Vsitr, Method::Gutmann, Method::NistClear] {
        let f = dir.path().join(format!("v-{}.bin", m.key()));
        write(&f, 33_333, 7);
        let o = WipeOptions { verify: true, ..opts(m) };
        let r = wipe_file(&f, &o, &mut Ctx::new(&CancelFlag::new(), &mut NoProgress)).unwrap();
        assert!(r.verified, "{} should verify", m.key());
        assert!(!f.exists());
    }
}

#[test]
fn empty_and_readonly_files() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    fs::write(&empty, b"").unwrap();
    let ro = dir.path().join("ro.txt");
    write(&ro, 1000, 1);
    let mut perm = fs::metadata(&ro).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&ro, perm).unwrap();

    for f in [&empty, &ro] {
        wipe_file(f, &opts(Method::Zero), &mut Ctx::new(&CancelFlag::new(), &mut NoProgress)).unwrap();
        assert!(!f.exists());
    }
}

#[test]
fn directory_tree_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("a/b/c")).unwrap();
    write(&root.join("1.txt"), 5000, 1);
    write(&root.join("a/2.txt"), 6000, 2);
    write(&root.join("a/b/3.txt"), 7000, 3);
    write(&root.join("a/b/c/4.txt"), 0, 0);

    let mut c = Counter::default();
    let r = wipe_dir(&root, &opts(Method::Random), &mut Ctx::new(&CancelFlag::new(), &mut c)).unwrap();
    assert_eq!(r.files_ok, 4);
    assert_eq!(r.files_err, 0);
    assert_eq!(r.dirs_removed, 4);
    assert_eq!(r.bytes_overwritten, 18_000);
    assert!(!root.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.txt");
    write(&outside, 1234, b'K');
    let root = dir.path().join("tree");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    std::os::unix::fs::symlink(dir.path(), root.join("dirlink")).unwrap();

    wipe_dir(&root, &opts(Method::Zero), &mut Ctx::new(&CancelFlag::new(), &mut NoProgress)).unwrap();
    assert!(!root.exists());
    let data = fs::read(&outside).unwrap();
    assert!(data.iter().all(|&b| b == b'K'), "link target must be untouched");
}

#[test]
fn cancellation_stops_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("big.bin");
    write(&f, 1_000_000, 9);
    let cancel = CancelFlag::new();
    cancel.cancel();
    let err = wipe_file(&f, &opts(Method::Gutmann), &mut Ctx::new(&cancel, &mut NoProgress)).unwrap_err();
    assert!(matches!(err, wipe::WipeError::Cancelled));
    assert!(f.exists(), "a cancelled file is not deleted");
}

#[test]
fn free_space_files_are_cleaned_up() {
    // Fills a small directory on the temp volume? That could fill the whole
    // disk, so instead reserve almost everything: only ~8 MiB get written.
    let dir = tempfile::tempdir().unwrap();
    let vol = freewipe::filesystem::probe(dir.path()).unwrap();
    if vol.free < (64 << 20) {
        return; // not enough room to test safely
    }
    let o = WipeOptions { reserve_bytes: vol.free - (8 << 20), verify: true, ..opts(Method::Gost) };
    let r = wipe::wipe_free_space(dir.path(), &o, &mut Ctx::new(&CancelFlag::new(), &mut NoProgress)).unwrap();
    assert!(r.files_created >= 1);
    assert!(r.bytes_filled > 0);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "temporary fill files must be removed");
}

#[test]
fn execute_skips_protected_and_missing() {
    let cancel = CancelFlag::new();
    let missing = Target::file("/definitely/not/here.txt");
    let it = wipe::execute(&missing, &opts(Method::Zero), &mut Ctx::new(&cancel, &mut NoProgress));
    assert_eq!(it.status, Status::Skipped);

    let root = if cfg!(windows) { Target::dir("C:\\") } else { Target::dir("/") };
    let it = wipe::execute(&root, &opts(Method::Zero), &mut Ctx::new(&cancel, &mut NoProgress));
    assert_eq!(it.status, Status::Skipped);
    assert!(it.error.is_some());
}

#[test]
fn plan_counts_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(root.join("x")).unwrap();
    write(&root.join("a"), 100, 0);
    write(&root.join("x/b"), 250, 0);
    let p = plan::scan(&Target::auto(&root), false);
    assert_eq!(p.target.kind, TargetKind::Dir);
    assert!(p.is_ok(), "{:?}", p.blocked);
    assert_eq!(p.bytes, 350);
    assert_eq!(p.files, 2);
    assert_eq!(p.work_bytes(3, true), 350 * 4);
}
