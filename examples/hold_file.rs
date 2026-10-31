//! Test helper: keep a file open without sharing (like an editor would),
//! so freewipe sees it as "in use". Usage: hold_file <path> <seconds>
fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("path");
    let secs: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    let _f = o.open(&path).expect("open");
    println!("holding {path} for {secs} s");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
