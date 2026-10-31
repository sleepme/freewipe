//! `freewipe_shell.dll` — Explorer context-menu handler for freewipe.
//!
//! Each freewipe menu item is a registry verb with
//! `ExplorerCommandHandler = {CLSID}`. Explorer loads this DLL, creates the
//! class and calls [`IExplorerCommand::Invoke`] with the selected items.
//! Because the items are *not* opened with `ShellExecute`, Windows performs
//! no zone / SmartScreen check on them: no "Open File – Security Warning",
//! no long busy cursor while a large download is scanned, and the whole
//! selection arrives in one call.
//!
//! The handler does nothing but write the selected paths to a temporary list
//! file and start `freewipe-gui.exe --shell <action> --list <file>` (the path
//! of freewipe-gui comes from the `FreewipeExe` value of the class key). All work
//! happens in freewipe-gui; the code running inside Explorer stays tiny.
//!
//! **Shortcuts.** For a right-click on a shortcut (`.lnk`) Explorer may hand
//! the handler the shortcut's *target* instead of the shortcut. The handler
//! therefore asks the folder window for its real selection (via the site
//! Explorer gives it) and, where a selected `.lnk` points at an item it was
//! given, passes the `.lnk` itself — wiping a shortcut never touches what it
//! points to. Without a site (or when nothing matches) the given items are
//! used unchanged.
//!
//! Every COM entry point catches panics so Explorer can never be taken down.

#![cfg(windows)]
#![allow(non_snake_case)]

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

use std::cell::RefCell;
use windows::core::{implement, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOTIMPL, E_POINTER, S_FALSE, S_OK,
};

use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl, IPersistFile, IServiceProvider,
    CLSCTX_INPROC_SERVER, STGM_READ,
};
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::{
    IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl, IFolderView, IShellBrowser, IShellItemArray,
    IShellLinkW, SID_STopLevelBrowser, ShellLink, ECF_DEFAULT, ECS_ENABLED, SIGDN_FILESYSPATH, SVGIO_SELECTION,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

/// First handler CLSID; action `n` uses `CLSID_BASE + n`.
/// Must match `freewipe::shell::HANDLER_CLSID_BASE`.
pub const CLSID_BASE: u128 = 0x7C4A9E10_52B1_4D6F_9E3A_0A1B2C3D4F00;
/// Number of action slots reserved.
const SLOTS: u128 = 0x40;

/// Live objects + server locks, for `DllCanUnloadNow`.
static REFS: AtomicIsize = AtomicIsize::new(0);

fn action_of(clsid: &GUID) -> Option<u32> {
    let v = clsid.to_u128();
    (CLSID_BASE..CLSID_BASE + SLOTS).contains(&v).then(|| (v - CLSID_BASE) as u32)
}

fn clsid_string(index: u32) -> String {
    let v = CLSID_BASE + u128::from(index);
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (v >> 96) as u32,
        (v >> 80) as u16,
        (v >> 64) as u16,
        (v >> 48) as u16,
        v & 0xFFFF_FFFF_FFFF
    )
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `FreewipeExe` value of our class key (HKCR merges per-user and machine).
fn gui_exe(index: u32) -> Option<PathBuf> {
    let key = wide(&format!(r"CLSID\{}", clsid_string(index)));
    let name = wide("FreewipeExe");
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: key/name are NUL-terminated; `buf` is writable for `bytes`
    // bytes and RegGetValueW writes at most that many (NUL-terminated).
    let rc = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut bytes),
        )
    };
    if rc.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let p = PathBuf::from(OsString::from_wide(&buf[..len]));
    p.is_file().then_some(p)
}

/// File-system paths of the selected items.
fn selection(items: &IShellItemArray) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: plain COM calls on a valid array supplied by Explorer; every
    // string returned by GetDisplayName is freed with CoTaskMemFree.
    unsafe {
        let n = items.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(item) = items.GetItemAt(i) else { continue };
            let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) else { continue };
            if !p.is_null() {
                let s = OsString::from_wide(p.as_wide());
                CoTaskMemFree(Some(p.0 as *const c_void));
                out.push(PathBuf::from(s));
            }
        }
    }
    out
}

/// What the folder window really has selected (shortcuts as `.lnk`).
fn view_selection(site: &IUnknown) -> Option<Vec<PathBuf>> {
    // SAFETY: COM calls on interfaces obtained from Explorer's site; every
    // result is checked.
    unsafe {
        let sp: IServiceProvider = site.cast().ok()?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser).ok()?;
        let view = browser.QueryActiveShellView().ok()?;
        let fv: IFolderView = view.cast().ok()?;
        let items: IShellItemArray = fv.Items(SVGIO_SELECTION).ok()?;
        Some(selection(&items))
    }
}

/// Target path of a `.lnk` file.
fn link_target(lnk: &std::path::Path) -> Option<PathBuf> {
    let w = wide(&lnk.to_string_lossy());
    // SAFETY: standard IShellLink use; the buffer is sized and NUL-terminated
    // by GetPath, and the path string outlives Load.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = link.cast().ok()?;
        pf.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut buf = vec![0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (len > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len])))
    }
}

fn is_lnk(p: &std::path::Path) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false)
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Replace any given item that is really the target of a selected shortcut
/// by that shortcut. Items that match nothing are kept as given.
pub fn prefer_shortcuts(
    given: Vec<PathBuf>,
    selected: &[PathBuf],
    target_of: impl Fn(&std::path::Path) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let links: Vec<(PathBuf, PathBuf)> =
        selected.iter().filter(|p| is_lnk(p)).filter_map(|l| target_of(l).map(|t| (l.clone(), t))).collect();
    given
        .into_iter()
        .map(|g| {
            if selected.iter().any(|s| same_path(s, &g)) {
                return g; // selected as itself
            }
            match links.iter().find(|(_, t)| same_path(t, &g)) {
                Some((l, _)) => l.clone(),
                None => g,
            }
        })
        .collect()
}

/// Debug log: when freewipe's debug log is on (marker file `logs\enabled` in
/// `%LOCALAPPDATA%\freewipe`), append a line to `logs\shell.log`.
fn slog(msg: &str) {
    let dir = match std::env::var_os("FREEWIPE_LOG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => match std::env::var_os("LOCALAPPDATA") {
            Some(d) => PathBuf::from(d).join("freewipe").join("logs"),
            None => return,
        },
    };
    if !dir.join("enabled").is_file() {
        return;
    }
    let path = dir.join("shell.log");
    if std::fs::metadata(&path).map(|m| m.len() > 5 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("shell.1.log"));
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("unix {secs} [{}] shell  | {msg}\r\n", std::process::id());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// Hand the selection to freewipe-gui.
fn launch(index: u32, paths: &[PathBuf]) -> std::io::Result<()> {
    let exe = gui_exe(index).ok_or_else(|| {
        slog(&format!("action {index}: FreewipeExe missing or not a file ({})", clsid_string(index)));
        std::io::Error::other("freewipe-gui.exe not found")
    })?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let list = std::env::temp_dir().join(format!("freewipe-sel-{}-{nanos}.txt", std::process::id()));
    let mut text = String::new();
    for p in paths {
        text.push_str(&p.to_string_lossy());
        text.push('\n');
    }
    std::fs::write(&list, text)?;
    let child = std::process::Command::new(&exe)
        .arg("--shell")
        .arg(index.to_string())
        .arg("--list")
        .arg(&list)
        // Inherit nothing from Explorer.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(c) => {
            // Explorer owns the foreground right now; pass it on so the
            // freewipe window comes to the front.
            // SAFETY: simple call with a process id we just created.
            let _ = unsafe { AllowSetForegroundWindow(c.id()) };
            slog(&format!("launched {} (pid {}) with {}", exe.display(), c.id(), list.display()));
            Ok(())
        }
        Err(e) => {
            slog(&format!("launch {} failed: {e}", exe.display()));
            let _ = std::fs::remove_file(&list);
            Err(e)
        }
    }
}

#[implement(IExplorerCommand, IObjectWithSite)]
struct Command {
    index: u32,
    /// Set by Explorer (the folder window); used to read the real selection.
    site: RefCell<Option<IUnknown>>,
}

impl Command {
    fn new(index: u32) -> Self {
        REFS.fetch_add(1, Ordering::SeqCst);
        Command { index, site: RefCell::new(None) }
    }
}

impl IObjectWithSite_Impl for Command_Impl {
    fn SetSite(&self, site: Ref<'_, IUnknown>) -> Result<()> {
        *self.site.borrow_mut() = site.cloned();
        Ok(())
    }
    fn GetSite(&self, riid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        if out.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked out-pointer from the caller.
        unsafe { *out = std::ptr::null_mut() };
        match self.site.borrow().as_ref() {
            // SAFETY: riid/out checked; `query` AddRefs on success.
            Some(s) => unsafe { s.query(riid, out).ok() },
            None => Err(E_FAIL.into()),
        }
    }
}

impl Drop for Command {
    fn drop(&mut self) {
        REFS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IExplorerCommand_Impl for Command_Impl {
    // Title, icon and tooltip come from the registry verb (MUIVerb, Icon).
    fn GetTitle(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetIcon(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetToolTip(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(GUID::from_u128(CLSID_BASE + u128::from(self.index)))
    }
    fn GetState(&self, _items: Ref<'_, IShellItemArray>, _slow_ok: BOOL) -> Result<u32> {
        Ok(ECS_ENABLED.0 as u32)
    }
    fn Invoke(&self, items: Ref<'_, IShellItemArray>, _ctx: Ref<'_, IBindCtx>) -> Result<()> {
        let index = self.index;
        let site = self.site.borrow().clone();
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut paths = match items.as_ref() {
                Some(a) => selection(a),
                None => Vec::new(),
            };
            slog(&format!("invoke action {index}: {} items {:?}", paths.len(), paths));
            if let Some(selected) = site.as_ref().and_then(view_selection) {
                let before = paths.clone();
                paths = prefer_shortcuts(paths, &selected, link_target);
                if paths != before {
                    slog(&format!("view selection {selected:?}: shortcuts substituted → {paths:?}"));
                }
            }
            launch(index, &paths).is_ok()
        }));
        if r.is_err() {
            slog("invoke panicked");
        }
        match r {
            Ok(true) => Ok(()),
            _ => Err(E_FAIL.into()),
        }
    }
    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }
    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(IClassFactory)]
struct Factory {
    index: u32,
}

impl Factory {
    fn new(index: u32) -> Self {
        REFS.fetch_add(1, Ordering::SeqCst);
        Factory { index }
    }
}

impl Drop for Factory {
    fn drop(&mut self) {
        REFS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<'_, IUnknown>, riid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if object.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: `object` is a valid out-pointer from the caller.
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let index = self.index;
        let cmd = catch_unwind(|| -> IExplorerCommand { Command::new(index).into() }).map_err(|_| E_FAIL)?;
        // SAFETY: riid/object checked above; `query` AddRefs on success.
        unsafe { cmd.query(riid, object).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            REFS.fetch_add(1, Ordering::SeqCst);
        } else {
            REFS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// COM entry point: the class factory for one of our CLSIDs.
///
/// # Safety
/// Called by COM with valid pointers (checked for null anyway).
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || riid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: checked non-null; COM passes valid pointers.
    unsafe { *out = std::ptr::null_mut() };
    // SAFETY: as above.
    let Some(index) = action_of(unsafe { &*clsid }) else { return CLASS_E_CLASSNOTAVAILABLE };
    let r = catch_unwind(|| {
        let f: IClassFactory = Factory::new(index).into();
        // SAFETY: riid/out are valid; `query` AddRefs on success.
        unsafe { f.query(riid, out) }
    });
    r.unwrap_or(E_FAIL)
}

/// COM entry point: may Explorer unload the DLL?
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if REFS.load(Ordering::SeqCst) <= 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_win_over_their_targets() {
        let p = PathBuf::from;
        let target_of =
            |l: &std::path::Path| (l == std::path::Path::new(r"C:\Desk\Report.lnk")).then(|| p(r"D:\Docs\report.docx"));
        // Explorer gave the target; the shortcut was selected.
        let out = prefer_shortcuts(vec![p(r"D:\docs\REPORT.docx")], &[p(r"C:\Desk\Report.lnk")], target_of);
        assert_eq!(out, vec![p(r"C:\Desk\Report.lnk")]);
        // A folder from the tree with an unrelated selection: unchanged.
        let out = prefer_shortcuts(vec![p(r"E:\Folder")], &[p(r"C:\Desk\Report.lnk")], target_of);
        assert_eq!(out, vec![p(r"E:\Folder")]);
        // Selected as itself.
        let out = prefer_shortcuts(vec![p(r"D:\Docs\report.docx")], &[p(r"D:\Docs\report.docx")], target_of);
        assert_eq!(out, vec![p(r"D:\Docs\report.docx")]);
    }

    #[test]
    fn clsid_layout() {
        assert_eq!(clsid_string(0), "{7C4A9E10-52B1-4D6F-9E3A-0A1B2C3D4F00}");
        assert_eq!(action_of(&GUID::from_u128(CLSID_BASE + 5)), Some(5));
        assert_eq!(action_of(&GUID::from_u128(CLSID_BASE - 1)), None);
    }
}
