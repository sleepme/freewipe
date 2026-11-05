//! Password prompts: the app lock screen and the page gates (Settings,
//! Schedule, Methods, Reports). One correct password unlocks every gated
//! page for the rest of the session.

use std::time::Instant;

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use freewipe::tr;

use crate::app::App;
use crate::theme::pal;
use crate::widgets;

/// Password card. Returns `true` once the right password was entered.
fn prompt(app: &mut App, ui: &mut Ui, title: &str, sub: &str) -> bool {
    let p = pal(ui);
    let mut unlocked = false;
    let waiting = app
        .lock
        .wait_until
        .map(|t| t.saturating_duration_since(Instant::now()).as_secs() + 1)
        .filter(|_| app.lock.wait_until.map(|t| Instant::now() < t).unwrap_or(false));
    ui.vertical_centered(|ui| {
        ui.set_max_width(380.0);
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(24, 22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("🔒").size(34.0).color(p.accent));
                    ui.add_space(4.0);
                    ui.label(RichText::new(title).size(19.0).strong());
                    widgets::hint(ui, sub);
                });
                ui.add_space(12.0);
                let r = ui.add_enabled(
                    waiting.is_none(),
                    egui::TextEdit::singleline(&mut app.lock.text)
                        .password(true)
                        .hint_text(tr!("lock.placeholder"))
                        .desired_width(f32::INFINITY),
                );
                // Focus the field once (asking every frame would keep
                // repainting while the window itself is not focused).
                if waiting.is_none() && !app.lock.focus_requested {
                    app.lock.focus_requested = true;
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                if let Some(secs) = waiting {
                    ui.label(RichText::new(tr!("lock.wait", s = secs)).color(p.warn_icon));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                } else if let Some(e) = &app.lock.error {
                    ui.label(RichText::new(e).color(p.err));
                }
                let w = ui.available_width();
                ui.allocate_ui_with_layout(egui::vec2(w, 44.0), Layout::right_to_left(Align::Center), |ui| {
                    let clicked = ui
                        .add_enabled_ui(waiting.is_none(), |ui| widgets::primary(ui, tr!("lock.unlock")))
                        .inner
                        .clicked();
                    if (clicked || enter) && waiting.is_none() {
                        let lock = app.settings.lock.clone();
                        unlocked = app.lock.try_unlock(&lock);
                    }
                });
            });
    });
    unlocked
}

/// Full-window lock shown on start when the app is password-protected.
pub fn app_lock_screen(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    egui::CentralPanel::default().frame(Frame::new().fill(p.bg).inner_margin(Margin::same(24))).show(ui, |ui| {
        ui.add_space((ui.available_height() / 2.0 - 150.0).max(10.0));
        if prompt(app, ui, &tr!("lock.title"), &tr!("lock.sub")) {
            app.locked = false;
            app.session_unlocked = true;
        }
    });
}

/// A page that can be put behind the password.
#[derive(Clone, Copy)]
pub enum Gated {
    Settings,
    Schedule,
    Methods,
    Reports,
}

/// Gate in front of a page. Returns `true` when the page may be shown.
pub fn gate(app: &mut App, ui: &mut Ui, page: Gated) -> bool {
    let lock = &app.settings.lock;
    let locked = match page {
        Gated::Settings => lock.settings_locked(),
        Gated::Schedule => lock.schedule_locked(),
        Gated::Methods => lock.methods_locked(),
        Gated::Reports => lock.reports_locked(),
    };
    if app.session_unlocked || !locked {
        return true;
    }
    let (title, sub) = match page {
        Gated::Settings => (tr!("lock.settings_title"), tr!("lock.settings_sub")),
        Gated::Schedule => (tr!("lock.schedule_title"), tr!("lock.schedule_sub")),
        Gated::Methods => (tr!("lock.methods_title"), tr!("lock.methods_sub")),
        Gated::Reports => (tr!("lock.reports_title"), tr!("lock.reports_sub")),
    };
    ui.add_space(60.0);
    if prompt(app, ui, &title, &sub) {
        app.session_unlocked = true;
    }
    false
}
