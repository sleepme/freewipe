//! `freewipe-gui` — desktop front end (eframe/egui).
//!
//! Launch modes (also used by the Explorer context menu):
//!
//! ```text
//! freewipe-gui                                 open the window
//! freewipe-gui --enqueue <paths…>              add to the queue
//! freewipe-gui --wipe [--method KEY] <paths…>  wipe (confirmation per settings)
//! freewipe-gui --freespace <volume>            wipe free space
//! freewipe-gui --recycle-bin                   wipe the Recycle Bin
//! freewipe-gui --pending                      wipe what was scheduled for this sign-in
//! freewipe-gui --lang uk                       override the UI language once
//! ```
//!
//! Explorer starts one process per selected item. Only the first one opens
//! a window (named mutex); the others drop their request into the inbox
//! folder and exit, and the window merges them into one batch.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![deny(unsafe_code)]

mod app;
#[cfg(windows)]
mod com_server;
mod inbox;
mod shellops;
mod theme;
#[cfg(windows)]
mod tray;
#[cfg(not(windows))]
#[path = "tray_stub.rs"]
mod tray;
mod views;
mod widgets;
mod worker;

use freewipe::settings::Settings;
use freewipe::{i18n, tr};

fn main() -> eframe::Result {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();

    // Started by COM for legacy menu entries of pre-release builds (DropTarget): take
    // the selection and start a normal freewipe-gui with it. (A process started
    // by COM may be told to keep its windows hidden, so it must not open the
    // window itself.) "Update menu" switches to the shell extension.
    #[cfg(windows)]
    if com_server::is_embedding(&args) {
        com_server::hand_over(com_server::serve());
        return Ok(());
    }
    let request = inbox::Request::from_args(args.into_iter());

    // Started at sign-in for files that were in use earlier.
    let request = if request.pending {
        match inbox::take_pending() {
            Some(r) => r,
            None => return Ok(()),
        }
    } else {
        request
    };

    freewipe::debuglog::set_component("gui");
    let mut settings = Settings::load();
    freewipe::dlog!("gui start: request={:?} elevated_now={}", request, {
        #[cfg(windows)]
        {
            freewipe::sys::is_elevated()
        }
        #[cfg(not(windows))]
        {
            false
        }
    });

    // Started by the Task Scheduler: the schedule must exist.
    let schedule = match &request.schedule {
        Some(id) => match settings.schedules.iter().find(|s| &s.id == id) {
            Some(s) => Some(s.clone()),
            None => return Ok(()),
        },
        None => None,
    };
    // Password lock: use the newest stored copy and restore any copy that
    // was deleted (settings.json, lock.json, registry).
    let lock = freewipe::lock::effective(&settings.lock);
    if lock != settings.lock {
        settings.lock = lock.clone();
        let _ = settings.save();
    }
    if lock.updated > 0 {
        let _ = freewipe::lock::save_user_stores(&lock);
    }
    let lang = request.lang.clone().unwrap_or_else(|| settings.language.clone());
    if !i18n::set_language(&lang) {
        i18n::set_language(i18n::DEFAULT_LANG);
    }

    // A relaunch for already-confirmed items is a one-off job window: it
    // neither takes the single-instance lock nor merges other requests.
    #[cfg(windows)]
    let _guard = if (request.confirmed && request.elevated) || schedule.is_some() {
        None
    } else {
        match acquire_instance(request.elevated) {
            Some(g) => Some(g),
            None => {
                // Second instance: hand the request to the running window.
                let r = if request.is_empty() { inbox::Request::focus() } else { request.clone() };
                let _ = inbox::post(&r);
                return Ok(());
            }
        }
    };

    // "Always run as administrator": restart elevated, keeping the request.
    #[cfg(windows)]
    if settings.always_admin && !request.elevated && !freewipe::sys::is_elevated() {
        if let Ok(exe) = std::env::current_exe() {
            let mut args = request.to_args();
            args.push("--elevated".into());
            let params: Vec<String> = args.iter().map(|a| freewipe::util::quote_arg(a)).collect();
            if freewipe::sys::launch_elevated(&exe, &params.join(" ")).is_ok() {
                return Ok(());
            }
            // UAC declined: carry on without administrator rights.
        }
    }

    // From Explorer with "wipe right away": a compact window.
    let quick = schedule.is_some()
        || request.confirmed
        || (request.is_wipe() && settings.shell.menu_action == freewipe::settings::MenuAction::WipeNow);
    // A scheduled run was authorised when the schedule was created.
    let unlocked =
        schedule.is_some() || request.unlock_token.as_deref().map(inbox::consume_unlock_token).unwrap_or(false);
    let hidden = schedule.as_ref().map(|s| !s.show_window).unwrap_or(false);

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/freewipe.png")).ok();
    // Small progress window straight away when nothing has to be confirmed.
    let needs_dialog = schedule.is_none()
        && (settings.lock.app_locked()
            || settings.lock.wipe_locked()
            || (!request.confirmed
                && settings.shell.always_confirm
                && settings.confirm_mode != freewipe::settings::ConfirmMode::None));
    let mini = quick && !needs_dialog;
    let (size, min) = if mini {
        ([views::quick::MINI_W, views::quick::MINI_H], [380.0, 200.0])
    } else if quick {
        ([620.0, 430.0], [520.0, 360.0])
    } else {
        ([1120.0, 720.0], [860.0, 560.0])
    };
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(format!("freewipe {}", freewipe::VERSION))
        .with_inner_size(size)
        .with_min_inner_size(min)
        .with_drag_and_drop(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    if mini && settings.mini_on_top {
        viewport = viewport.with_always_on_top();
    }
    if hidden {
        viewport = viewport.with_visible(false);
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };

    eframe::run_native(
        "freewipe",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx, settings.theme);
            let mut app = app::App::new(settings, request, cc.egui_ctx.clone(), quick, unlocked);
            if let Some(s) = schedule {
                app.run_schedule(&s, hidden);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| {
        eprintln!("{}: {e}", tr!("gui.start_failed"));
        e
    })
}

/// Take the single-instance lock. An elevated relaunch waits (up to 5 s) for
/// the normal instance that started it to exit and release the lock.
#[cfg(windows)]
fn acquire_instance(elevated: bool) -> Option<freewipe::sys::InstanceGuard> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match freewipe::sys::acquire_single_instance(inbox::MUTEX_NAME) {
            Ok(Some(g)) => return Some(g),
            Ok(None) if elevated && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(None) => return None,
            // Could not create the mutex at all: run without the lock.
            Err(_) => return Some(freewipe::sys::InstanceGuard::none()),
        }
    }
}
