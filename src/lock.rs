//! Password protection of the app and its settings.
//!
//! The password is never stored: only a random salt and a
//! PBKDF2-HMAC-SHA256 digest (200 000 iterations) live in `settings.json`.
//!
//! The record is kept in several places at once — `settings.json`, a
//! separate `lock.json`, the registry (`HKCU\Software\freewipe`) and, if the
//! user chose "all users", `HKLM\SOFTWARE\freewipe`. freewipe uses the newest
//! record it finds and writes it back everywhere, so deleting one store
//! (e.g. `settings.json`) does not remove the lock. Removing the password
//! writes a dated "cleared" record that outranks older copies.
//!
//! Honest scope: this is an *access lock* for freewipe on a shared computer;
//! it does not encrypt anything. Whoever controls the same Windows account
//! can delete every per-user copy; only the machine-wide (HKLM) lock is out
//! of reach of standard users, and nothing stops a PC administrator.

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// PBKDF2 iterations for new passwords.
pub const ITERATIONS: u32 = 200_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lock {
    /// Hex salt; empty = no password set.
    pub salt: String,
    /// Hex PBKDF2-HMAC-SHA256 digest.
    pub hash: String,
    pub iterations: u32,
    /// Ask for the password when freewipe starts (GUI and wiping CLI commands).
    pub protect_app: bool,
    /// Ask for the password before opening Settings.
    pub protect_settings: bool,
    /// Ask for the password every time a wipe starts.
    pub protect_wipe: bool,
    /// Ask for the password before opening Schedule (create, change, run
    /// now or delete scheduled wipes). Scheduled runs themselves never ask.
    pub protect_schedule: bool,
    /// Ask for the password before opening Methods (custom methods).
    pub protect_methods: bool,
    /// Ask for the password before opening Reports (view, delete, wipe).
    pub protect_reports: bool,
    /// Unix time of the last change (newest record wins).
    pub updated: u64,
    /// Record lives in HKLM (all users).
    pub machine: bool,
}

impl Lock {
    pub fn has_password(&self) -> bool {
        !self.salt.is_empty() && !self.hash.is_empty()
    }
    pub fn app_locked(&self) -> bool {
        self.has_password() && self.protect_app
    }
    pub fn settings_locked(&self) -> bool {
        self.has_password() && (self.protect_settings || self.protect_app)
    }
    pub fn wipe_locked(&self) -> bool {
        self.has_password() && self.protect_wipe
    }
    pub fn schedule_locked(&self) -> bool {
        self.has_password() && (self.protect_schedule || self.protect_app)
    }
    pub fn methods_locked(&self) -> bool {
        self.has_password() && (self.protect_methods || self.protect_app)
    }
    pub fn reports_locked(&self) -> bool {
        self.has_password() && (self.protect_reports || self.protect_app)
    }
    /// Any protection active at all.
    pub fn active(&self) -> bool {
        self.has_password()
            && (self.protect_app
                || self.protect_settings
                || self.protect_wipe
                || self.protect_schedule
                || self.protect_methods
                || self.protect_reports)
    }
    /// Stamp the record as changed now.
    pub fn touch(&mut self) {
        self.updated = now();
    }

    /// Set (or replace) the password.
    pub fn set_password(&mut self, password: &str) {
        self.set_password_with(password, ITERATIONS);
    }

    pub(crate) fn set_password_with(&mut self, password: &str, iterations: u32) {
        self.touch();
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        self.salt = hex(&salt);
        self.iterations = iterations;
        self.hash = hex(&pbkdf2_sha256(password.as_bytes(), &salt, iterations));
    }

    /// Remove the password and all protections (a dated "cleared" record).
    pub fn clear(&mut self) {
        let machine = self.machine;
        *self = Lock { updated: now(), machine, ..Lock::default() };
    }

    /// Constant-time check of `password`.
    pub fn verify(&self, password: &str) -> bool {
        let Some(salt) = unhex(&self.salt) else { return false };
        let Some(expected) = unhex(&self.hash) else { return false };
        let got = pbkdf2_sha256(password.as_bytes(), &salt, self.iterations.max(1));
        let ok =
            expected.len() == got.len() && expected.iter().zip(got.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        // Never the password itself.
        crate::dlog!("password check: {}", if ok { "accepted" } else { "rejected" });
        ok
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// --- storage -------------------------------------------------------------------

fn file_path() -> std::path::PathBuf {
    crate::settings::data_dir().join("lock.json")
}

#[cfg(windows)]
const REG_KEY: &str = r"Software\freewipe";
#[cfg(windows)]
const REG_VALUE: &str = "Lock";

fn parse(s: &str) -> Option<Lock> {
    serde_json::from_str(s).ok()
}

/// Every stored copy except the one inside settings.json.
fn read_stores() -> Vec<Lock> {
    let mut v = Vec::new();
    if let Some(l) = std::fs::read_to_string(file_path()).ok().and_then(|s| parse(&s)) {
        v.push(l);
    }
    #[cfg(windows)]
    {
        use crate::sys::{reg_open, RegRoot};
        for (root, machine) in [(RegRoot::CurrentUser, false), (RegRoot::LocalMachine, true)] {
            if let Some(mut l) = reg_open(root, REG_KEY)
                .ok()
                .flatten()
                .and_then(|k| k.get_string(Some(REG_VALUE)))
                .and_then(|s| parse(&s))
            {
                l.machine = machine;
                v.push(l);
            }
        }
    }
    v
}

/// The lock in force: a machine-wide record if present, otherwise the
/// newest of all per-user copies (including the one from settings.json).
pub fn effective(from_settings: &Lock) -> Lock {
    let stores = read_stores();
    if let Some(m) = stores.iter().find(|l| l.machine && l.has_password()) {
        return m.clone();
    }
    let mut best = from_settings.clone();
    for l in stores.into_iter().filter(|l| !l.machine) {
        if l.updated > best.updated {
            best = l;
        }
    }
    best.machine = false;
    best
}

/// Write the per-user copies (file + HKCU). settings.json is saved by the caller.
pub fn save_user_stores(lock: &Lock) -> std::io::Result<()> {
    let mut l = lock.clone();
    l.machine = false;
    let json = serde_json::to_string(&l).map_err(std::io::Error::other)?;
    if let Some(d) = file_path().parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(file_path(), &json)?;
    #[cfg(windows)]
    crate::sys::reg_create(crate::sys::RegRoot::CurrentUser, REG_KEY)?.set_string(Some(REG_VALUE), &json)?;
    Ok(())
}

/// Write (or with `None`, delete) the machine-wide copy. Needs administrator rights.
#[cfg(windows)]
pub fn save_machine_store(lock: Option<&Lock>) -> std::io::Result<()> {
    use crate::sys::{reg_create, reg_delete_tree, RegRoot};
    match lock {
        Some(l) => {
            let mut l = l.clone();
            l.machine = true;
            let json = serde_json::to_string(&l).map_err(std::io::Error::other)?;
            reg_create(RegRoot::LocalMachine, r"SOFTWARE\freewipe")?.set_string(Some(REG_VALUE), &json)
        }
        None => reg_delete_tree(RegRoot::LocalMachine, r"SOFTWARE\freewipe"),
    }
}

/// Is a machine-wide lock installed?
pub fn machine_lock_present() -> bool {
    read_stores().iter().any(|l| l.machine && l.has_password())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn hmac_sha256(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for d in data {
        inner.update(d);
    }
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(ih);
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA256 with a 32-byte output (one block).
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut u = hmac_sha256(password, &[salt, &1u32.to_be_bytes()]);
    let mut t = u;
    for _ in 1..iterations {
        u = hmac_sha256(password, &[&u]);
        for (a, b) in t.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_known_vectors() {
        // RFC 7914 §11 / widely published PBKDF2-HMAC-SHA256 vectors.
        assert_eq!(
            hex(&pbkdf2_sha256(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            hex(&pbkdf2_sha256(b"password", b"salt", 2)),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        assert_eq!(
            hex(&pbkdf2_sha256(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn page_locks() {
        let mut l = Lock { protect_schedule: true, ..Lock::default() };
        // No password: nothing is locked, whatever the switches say.
        assert!(!l.schedule_locked() && !l.active());
        l.set_password_with("x", 1);
        assert!(l.schedule_locked() && !l.methods_locked() && !l.reports_locked() && l.active());
        l.protect_methods = true;
        l.protect_reports = true;
        assert!(l.methods_locked() && l.reports_locked());
        // "When freewipe starts" puts every page behind the password too.
        let app_only =
            Lock { protect_app: true, ..Lock { salt: l.salt.clone(), hash: l.hash.clone(), ..Lock::default() } };
        assert!(app_only.schedule_locked() && app_only.methods_locked() && app_only.reports_locked());
        // Records saved before these switches existed still load.
        let old = r#"{"salt":"00","hash":"00","iterations":1,"protect_settings":true,"updated":1}"#;
        let o: Lock = serde_json::from_str(old).unwrap();
        assert!(o.protect_settings && !o.protect_schedule && !o.protect_methods && !o.protect_reports);
    }

    #[test]
    fn newest_record_wins_and_clear_is_dated() {
        let mut a = Lock::default();
        a.set_password_with("x", 1);
        a.updated = 100;
        let mut b = a.clone();
        b.clear();
        b.updated = 200;
        assert!(!b.has_password());
        // Merge logic: pick the newer one.
        let pick = |s: &Lock, o: &Lock| if o.updated > s.updated { o.clone() } else { s.clone() };
        assert!(!pick(&a, &b).has_password());
        assert!(pick(&b, &Lock { updated: 50, ..a.clone() }).updated == 200);
        assert!(serde_json::from_str::<Lock>(&serde_json::to_string(&a).unwrap()).unwrap().verify("x"));
    }

    #[test]
    fn set_verify_clear() {
        let mut l = Lock::default();
        assert!(!l.has_password());
        l.set_password_with("pässwört-1", 10);
        l.protect_app = true;
        assert!(l.app_locked());
        assert!(l.verify("pässwört-1"));
        assert!(!l.verify("pässwört-2"));
        assert!(!l.verify(""));
        l.clear();
        assert!(!l.app_locked());
    }
}
