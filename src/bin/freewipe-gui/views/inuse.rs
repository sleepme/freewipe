//! "Files in use": who holds them and what to do about it.

use eframe::egui::{RichText, Ui};

use freewipe::tr;

use crate::app::App;
use crate::theme::pal;
use crate::widgets;

/// Card shown after a job when some files were open in another program.
pub fn panel(app: &mut App, ui: &mut Ui, compact: bool) {
    let p = pal(ui);
    let Some(jv) = app.job.as_ref() else { return };
    if !jv.finished || jv.locked_files.is_empty() {
        return;
    }
    let n = jv.locked_files.len();
    let busy = jv.closing.is_some();
    let names: Vec<String> = jv.lockers.iter().map(|l| format!("{} ({})", l.name, l.pid)).collect();
    let can_close = !jv.lockers.is_empty() && jv.lockers.iter().all(|l| l.closable);
    let msg = jv.inuse_msg.clone();
    let apps = jv.lockers.iter().map(|l| l.name.clone()).collect::<Vec<_>>().join(", ");

    let mut close = false;
    let mut retry = false;
    let mut later = false;
    widgets::card(ui, |ui| {
        ui.label(RichText::new(format!("🔒 {}", tr!("gui.inuse_title", n = n))).strong().color(p.warn_icon));
        if names.is_empty() {
            widgets::hint(ui, &tr!("gui.inuse_unknown"));
        } else {
            let list = if compact && names.len() > 2 {
                format!("{}, … (+{})", names[..2].join(", "), names.len() - 2)
            } else {
                names.join(", ")
            };
            ui.label(RichText::new(tr!("gui.inuse_by", apps = list)).size(12.5));
        }
        if let Some((ok, m)) = &msg {
            ui.label(RichText::new(m).size(12.0).color(if *ok { p.muted } else { p.err }));
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if can_close && widgets::secondary(ui, tr!("gui.inuse_close", apps = apps)).clicked() {
                    close = true;
                }
                if widgets::secondary(ui, tr!("gui.inuse_retry")).clicked() {
                    retry = true;
                }
                if cfg!(windows) && widgets::secondary(ui, tr!("gui.inuse_signin")).clicked() {
                    later = true;
                }
            });
            if busy {
                ui.spinner();
            }
        });
    });
    ui.add_space(8.0);
    if close {
        app.close_lockers_and_retry();
    } else if retry {
        app.retry_locked();
    } else if later {
        app.wipe_at_next_signin();
    }
}
