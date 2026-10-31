//! Banner offering to replace context-menu entries left by another freewipe
//! version (or another copy of freewipe in a different folder).

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use freewipe::tr;

use crate::app::App;
use crate::theme::pal;
use crate::widgets;

pub fn banner(app: &mut App, ui: &mut Ui) {
    if app.foreign.is_empty() || app.foreign_dismissed {
        return;
    }
    let p = pal(ui);
    let mut action: Option<(bool, bool)> = None; // (all_users, update?)
    let mut dismiss = false;
    Frame::new()
        .fill(p.warn_bg)
        .stroke(Stroke::new(1.0, p.warn_border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (all_users, st) in &app.foreign {
                let scope = if *all_users { tr!("shell.scope_all") } else { tr!("shell.scope_user") };
                let exe = st.exe.clone().unwrap_or_else(|| "?".into());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("⚠").color(p.warn_icon));
                    ui.label(
                        RichText::new(tr!("gui.legacy_found", version = st.version_label(), scope = scope, exe = exe))
                            .color(p.warn_text)
                            .size(13.0),
                    );
                });
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 36.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        if widgets::secondary(ui, tr!("gui.legacy_later")).clicked() {
                            dismiss = true;
                        }
                        if widgets::secondary(ui, tr!("gui.legacy_remove")).clicked() {
                            action = Some((*all_users, false));
                        }
                        if widgets::primary(ui, tr!("gui.legacy_update")).clicked() {
                            action = Some((*all_users, true));
                        }
                    },
                );
            }
        });
    ui.add_space(10.0);

    if dismiss {
        app.foreign_dismissed = true;
    }
    if let Some((all_users, update)) = action {
        let res = if update {
            crate::shellops::update_scope(&app.settings.shell, all_users)
        } else {
            crate::shellops::remove_scope(all_users)
        };
        match res {
            Ok(msg) => {
                if update {
                    app.settings.shell.enabled = true;
                    app.settings.shell.all_users = all_users;
                    app.shell_applied = Some(app.settings.shell.clone());
                } else if app.settings.shell.all_users == all_users {
                    app.settings.shell.enabled = false;
                    app.shell_applied = None;
                }
                app.toast(msg);
            }
            Err(e) => app.toast(e),
        }
        app.foreign = crate::shellops::foreign_entries();
        app.shell_status = None;
    }
}
