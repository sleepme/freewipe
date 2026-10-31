//! Notification-area ("tray") icon shown only while freewipe is wiping.
//!
//! A small thread owns a message-only window that receives the icon's
//! mouse and balloon messages; the app updates the tooltip and shows
//! notifications by calling `Shell_NotifyIconW` with that window, which is
//! allowed from any thread. Clicks are queued for [`Tray::take_events`] and
//! wake the egui event loop (also while the main window is hidden).
//!
//! `unsafe` is limited to the Win32 calls; every block says why it is sound.

#![allow(unsafe_code)]

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use eframe::egui;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, LoadIconW,
    LoadImageW, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, HICON, HWND_MESSAGE, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTCOLOR, MSG, SM_CXSMICON, SM_CYSMICON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER, WNDCLASSW,
};

/// Message the icon sends to our window.
const WM_TRAY: u32 = WM_APP + 1;
/// The user clicked the notification balloon / toast.
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const ICON_ID: u32 = 1;

/// What the user did with the icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Clicked the icon: show the window.
    Show,
    /// Clicked the notification.
    NotificationClicked,
}

/// Kind of notification (picks the system icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

struct Shared {
    events: Vec<TrayEvent>,
    ctx: Option<egui::Context>,
}

fn shared() -> &'static Mutex<Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared { events: Vec::new(), ctx: None }))
}

fn push(ev: TrayEvent) {
    let ctx = {
        let Ok(mut s) = shared().lock() else { return };
        s.events.push(ev);
        s.ctx.clone()
    };
    if let Some(c) = ctx {
        c.request_repaint();
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let ev = lparam.0 as u32 & 0xFFFF;
            match ev {
                x if x == WM_LBUTTONUP || x == WM_LBUTTONDBLCLK || x == WM_RBUTTONUP => push(TrayEvent::Show),
                NIN_BALLOONUSERCLICK => push(TrayEvent::NotificationClicked),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: our own window, destroyed on its own thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling for everything else, same arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Copy `s` into a fixed UTF-16 buffer (truncated, NUL-terminated).
fn put(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for c in s.encode_utf16() {
        if n + 1 >= dst.len() {
            break;
        }
        dst[n] = c;
        n += 1;
    }
    dst[n] = 0;
}

fn load_icon() -> HICON {
    // SAFETY: plain resource lookups in our own module; the result is
    // checked, and the stock icon is the fallback.
    unsafe {
        if let Ok(m) = GetModuleHandleW(PCWSTR::null()) {
            let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            // Resource id 1 = the icon embedded by build.rs.
            if let Ok(h) = LoadImageW(Some(m.into()), PCWSTR(1 as _), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) {
                if !h.is_invalid() {
                    return HICON(h.0);
                }
            }
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

pub struct Tray {
    hwnd: isize,
    icon: isize,
    thread: Option<JoinHandle<()>>,
}

impl Tray {
    /// Create the icon with an initial tooltip. `None` if Windows refused.
    pub fn new(ctx: &egui::Context, tip: &str) -> Option<Tray> {
        if let Ok(mut s) = shared().lock() {
            s.ctx = Some(ctx.clone());
            s.events.clear();
        }
        let (tx, rx) = channel::<isize>();
        let thread = std::thread::spawn(move || {
            // SAFETY: standard window class registration and creation of a
            // message-only window owned by this thread; `wndproc` matches the
            // required signature; the loop runs until WM_DESTROY.
            unsafe {
                let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
                let class = w!("freewipe-tray");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: hinst.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc); // fails harmlessly if already registered
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    w!("freewipe"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(hinst.into()),
                    None,
                );
                let Ok(hwnd) = hwnd else {
                    let _ = tx.send(0);
                    return;
                };
                let _ = tx.send(hwnd.0 as isize);
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let hwnd = rx_wait(rx)?;
        let icon = load_icon();
        let tray = Tray { hwnd, icon: icon.0 as isize, thread: Some(thread) };
        let mut d = tray.data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        put(&mut d.szTip, tip);
        // SAFETY: `d` is a fully initialised NOTIFYICONDATAW with cbSize set.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &d) }.as_bool() {
            return None; // Drop closes the window thread
        }
        Some(tray)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(self.hwnd as *mut _),
            uID: ICON_ID,
            hIcon: HICON(self.icon as *mut _),
            ..Default::default()
        }
    }

    /// Change the tooltip (e.g. "freewipe — 42 %").
    pub fn set_tip(&self, tip: &str) {
        let mut d = self.data();
        d.uFlags = NIF_TIP;
        put(&mut d.szTip, tip);
        // SAFETY: initialised struct; the icon exists (added in `new`).
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Show a notification (a toast on Windows 10/11).
    pub fn notify(&self, title: &str, text: &str, level: Level) {
        let mut d = self.data();
        d.uFlags = NIF_INFO;
        put(&mut d.szInfoTitle, title);
        put(&mut d.szInfo, text);
        d.dwInfoFlags = match level {
            Level::Info => NIIF_INFO,
            Level::Warning => NIIF_WARNING,
            Level::Error => NIIF_ERROR,
        };
        // SAFETY: as in `set_tip`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Clicks since the last call.
    pub fn take_events(&self) -> Vec<TrayEvent> {
        shared().lock().map(|mut s| std::mem::take(&mut s.events)).unwrap_or_default()
    }
}

fn rx_wait(rx: Receiver<isize>) -> Option<isize> {
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(h) if h != 0 => Some(h),
        _ => None,
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let d = self.data();
        // SAFETY: removes our icon (harmless if it was never added) and asks
        // the window thread to finish; the handle stays valid until then.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut s) = shared().lock() {
            s.ctx = None;
        }
    }
}
