//! Out-of-process COM server for the Explorer context menu (DropTarget mode).
//!
//! Explorer starts `freewipe-gui.exe -Embedding`, creates the class registered
//! for the chosen verb, and calls `IDropTarget::DragEnter` + `Drop` with an
//! `IDataObject` holding the whole selection (`CF_HDROP`). Because the items
//! are handed over as data rather than "opened", Explorer performs no
//! zone/SmartScreen check on them: no "Open File – Security Warning" and no
//! long wait while a multi-gigabyte download is scanned.
//!
//! Legacy: only used by menu entries written by pre-release builds. The menu
//! now uses the shell extension (`freewipe_shell.dll`) instead, because Windows
//! still runs its download check before activating a drop-target verb. The server only
//! collects the paths and hands them to a normally started freewipe-gui.
//!
//! This is the only place in the GUI that needs `unsafe` (COM / Win32);
//! every block says why it is sound.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::core::{implement, IUnknown, Interface, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER, POINTL};
use windows::Win32::System::Com::{
    CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, IClassFactory, IClassFactory_Impl,
    IDataObject, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, DVASPECT_CONTENT, FORMATETC, REGCLS_MULTIPLEUSE,
    TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, ReleaseStgMedium, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, MSG, WM_TIMER,
};

use freewipe::shell::DropAction;

/// How long to wait for Explorer to call `Drop` after starting us.
const WAIT_FOR_DROP: Duration = Duration::from_secs(30);
/// After a drop, keep serving briefly so Explorer can release its objects.
const LINGER: Duration = Duration::from_millis(250);

thread_local! {
    /// Drops received on this (STA) thread, in order.
    static DROPS: RefCell<Vec<(DropAction, Vec<PathBuf>)>> = const { RefCell::new(Vec::new()) };
}

/// Was this process started by COM?
pub fn is_embedding(args: &[OsString]) -> bool {
    args.iter().any(|a| {
        let s = a.to_string_lossy().to_ascii_lowercase();
        s == "-embedding" || s == "/embedding"
    })
}

#[implement(IClassFactory)]
struct Factory {
    action: DropAction,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        riid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if object.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: `object` is a valid out-pointer supplied by COM.
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let target: IDropTarget = DropHandler { action: self.action }.into();
        // SAFETY: `riid` and `object` were checked for null above and come
        // from COM; `query` writes an AddRef'ed pointer or null.
        unsafe { target.query(riid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[implement(IDropTarget)]
struct DropHandler {
    action: DropAction,
}

fn set_effect(effect: *mut DROPEFFECT) {
    if !effect.is_null() {
        // SAFETY: non-null out-pointer provided by the caller (Explorer).
        unsafe { *effect = DROPEFFECT_COPY };
    }
}

impl IDropTarget_Impl for DropHandler_Impl {
    fn DragEnter(
        &self,
        _data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        set_effect(effect);
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        set_effect(effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let data = data.ok()?;
        let paths = read_hdrop(data)?;
        set_effect(effect);
        let action = self.action;
        DROPS.with(|d| d.borrow_mut().push((action, paths)));
        Ok(())
    }
}

/// Paths from the `CF_HDROP` format of a data object.
fn read_hdrop(data: &IDataObject) -> Result<Vec<PathBuf>> {
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: `fmt` is a valid FORMATETC for the duration of the call.
    let mut medium = unsafe { data.GetData(&fmt)? };
    let mut out = Vec::new();
    // SAFETY: for TYMED_HGLOBAL + CF_HDROP the union holds an HGLOBAL that
    // is a DROPFILES block, i.e. an HDROP. DragQueryFileW only reads it; the
    // buffer passed in is sized from the length it reports (+1 for NUL).
    // The medium is released exactly once afterwards.
    unsafe {
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            if got > 0 {
                out.push(PathBuf::from(OsString::from_wide(&buf[..got])));
            }
        }
        ReleaseStgMedium(&mut medium);
    }
    Ok(out)
}

/// Register the class objects, pump messages until Explorer has dropped
/// (or the wait times out), then shut COM down again. Returns the drops.
pub fn serve() -> Vec<(DropAction, Vec<PathBuf>)> {
    // SAFETY: first COM call on this thread; balanced by CoUninitialize below.
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return Vec::new();
    }
    let mut cookies = Vec::new();
    for action in DropAction::all() {
        let factory: IClassFactory = Factory { action }.into();
        let clsid = GUID::from_u128(action.legacy_clsid_u128());
        // SAFETY: `clsid` lives for the call; COM AddRefs the factory.
        if let Ok(cookie) = unsafe { CoRegisterClassObject(&clsid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE) }
        {
            cookies.push(cookie);
        }
    }

    let started = Instant::now();
    let mut first_drop: Option<Instant> = None;
    // SAFETY: a thread timer (no window, no callback): WM_TIMER is posted to
    // this thread's queue so the loop below wakes up regularly.
    let timer = unsafe { SetTimer(None, 0, 100, None) };
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns the STA; `msg`
    // is a valid out-buffer.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if msg.message == WM_TIMER {
                let have = DROPS.with(|d| !d.borrow().is_empty());
                if have && first_drop.is_none() {
                    first_drop = Some(Instant::now());
                }
                let done = first_drop.map(|t| t.elapsed() >= LINGER).unwrap_or(false);
                if done || (!have && started.elapsed() >= WAIT_FOR_DROP) {
                    break;
                }
            }
        }
        let _ = KillTimer(None, timer);
        for c in cookies {
            let _ = CoRevokeClassObject(c);
        }
        CoUninitialize();
    }
    DROPS.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

/// Start a normal `freewipe-gui` for each drop (`--shell <n> --list <file>`)
/// and give it the right to come to the foreground.
pub fn hand_over(drops: Vec<(DropAction, Vec<PathBuf>)>) {
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    let Ok(exe) = std::env::current_exe() else { return };
    for (n, (action, paths)) in drops.into_iter().enumerate() {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let list = std::env::temp_dir().join(format!("freewipe-sel-{}-{nanos}-{n}.txt", std::process::id()));
        let text: String = paths.iter().map(|p| format!("{}\n", p.display())).collect();
        if std::fs::write(&list, text).is_err() {
            continue;
        }
        if let Ok(child) = std::process::Command::new(&exe)
            .arg("--shell")
            .arg(action.index().to_string())
            .arg("--list")
            .arg(&list)
            .spawn()
        {
            // SAFETY: plain call with the id of the process just started.
            let _ = unsafe { AllowSetForegroundWindow(child.id()) };
        }
    }
}
