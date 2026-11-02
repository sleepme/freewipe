//! Confirmation dialog and the progress screen.

use std::time::Duration;

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use freewipe::plan::PlanItem;
use freewipe::report::Status;
use freewipe::tr;
use freewipe::util::{human_bytes, human_duration};

use freewipe::settings::ConfirmMode;

use super::queue::{kind_icon, names};
use crate::app::{open_path, App};
use crate::theme::pal;
use crate::widgets;

/// What the user did in the confirmation content this frame.
#[derive(PartialEq, Eq)]
pub enum ConfirmOutcome {
    Pending,
    Go,
    Cancel,
}

/// The confirmation as a modal over the main window.
pub fn confirm_dialog(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let mut outcome = ConfirmOutcome::Pending;
    let resp = egui::Modal::new(egui::Id::unique("confirm")).show(&ctx, |ui| {
        ui.set_width(560.0);
        outcome = confirm_body(app, ui);
    });
    if resp.should_close() && outcome == ConfirmOutcome::Pending {
        outcome = ConfirmOutcome::Cancel;
    }
    apply_outcome(app, outcome);
}

/// Start the job or drop the confirmation.
pub fn apply_outcome(app: &mut App, outcome: ConfirmOutcome) {
    match outcome {
        ConfirmOutcome::Pending => {}
        ConfirmOutcome::Go => {
            if let Some(c) = app.confirm.take() {
                let targets: Vec<_> = c.plans.iter().filter(|p| p.is_ok()).map(|p| p.target.clone()).collect();
                let plans: Vec<PlanItem> = c.plans.into_iter().filter(|p| p.is_ok()).collect();
                app.start_job(targets, &plans, c.opts);
            }
        }
        ConfirmOutcome::Cancel => app.confirm = None,
    }
}

/// Confirmation content (used by the modal and by the compact window).
pub fn confirm_body(app: &mut App, ui: &mut Ui) -> ConfirmOutcome {
    let p = pal(ui);
    let mode = app.settings.confirm_mode;
    let lock = app.settings.lock.clone();
    let need_pw = lock.wipe_locked() && !app.wipe_preauthorized;
    let ssd_warning = app.settings.ssd_warning;
    let compact = app.quick;
    let Some(c) = app.confirm.as_mut() else { return ConfirmOutcome::Cancel };

    if let Some(rx) = &c.rx {
        while let Ok(pl) = rx.try_recv() {
            c.plans.push(pl);
        }
        if c.plans.len() >= c.targets.len() {
            c.rx = None;
        }
    }
    let ready = c.rx.is_none();
    let runnable: Vec<&PlanItem> = c.plans.iter().filter(|p| p.is_ok()).collect();
    let bytes: u64 = runnable.iter().map(|p| p.bytes).sum();
    let passes = c.opts.pass_count();
    let work: u64 = runnable.iter().map(|p| c.opts.work_bytes(p)).sum();
    let flash = runnable.iter().filter(|p| p.media.as_ref().map(|m| m.is_flash_likely()).unwrap_or(false)).count();
    let blocked: Vec<String> = c.plans.iter().filter_map(|p| p.blocked.clone()).collect();
    let word = tr!("cli.confirm_word");
    let mut outcome = ConfirmOutcome::Pending;

    ui.horizontal(|ui| {
        Frame::new().fill(p.accent_soft).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(10)).show(
            ui,
            |ui| {
                ui.label(RichText::new("🗑").size(20.0).color(p.accent));
            },
        );
        ui.vertical(|ui| {
            ui.label(RichText::new(tr!("gui.confirm_title", n = runnable.len())).size(19.0).strong());
            ui.label(RichText::new(tr!("gui.confirm_sub")).color(p.muted));
        });
    });
    ui.add_space(10.0);

    Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("confirm-grid").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new(tr!("gui.c_data")).color(p.muted));
                ui.label(RichText::new(if ready { human_bytes(bytes) } else { "…".into() }).monospace());
                ui.end_row();
                ui.label(RichText::new(tr!("gui.c_method")).color(p.muted));
                let mut method = format!("{} · {}", c.opts.method.name(), tr!("cli.passes_n", n = passes));
                if flash > 0 && c.opts.single_pass_applies(true) {
                    method = format!("{method} · {}", tr!("gui.c_ssd_one_pass"));
                }
                ui.label(method);
                ui.end_row();
                if !compact {
                    ui.label(RichText::new(tr!("gui.c_verify")).color(p.muted));
                    ui.label(if c.opts.verify { tr!("gui.yes") } else { tr!("gui.no") });
                    ui.end_row();
                    ui.label(RichText::new(tr!("gui.c_after")).color(p.muted));
                    ui.label(if c.opts.keep { tr!("gui.after_keep") } else { tr!("gui.after_delete") });
                    ui.end_row();
                    ui.label(RichText::new(tr!("gui.c_write")).color(p.muted));
                    ui.label(RichText::new(if ready { human_bytes(work) } else { "…".into() }).monospace());
                    ui.end_row();
                }
                let free = c.targets.iter().any(|t| t.kind == freewipe::plan::TargetKind::FreeSpace);
                if free && (c.opts.cluster_tips || c.opts.file_records) {
                    let mut extra = Vec::new();
                    if c.opts.cluster_tips {
                        extra.push(tr!("gui.c_tips"));
                    }
                    if c.opts.file_records {
                        extra.push(tr!("gui.c_records"));
                    }
                    ui.label(RichText::new(tr!("gui.c_also")).color(p.muted));
                    ui.label(extra.join(", "));
                    ui.end_row();
                }
            });
        });

    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("confirm-list").max_height(if compact { 70.0 } else { 110.0 }).show(
        ui,
        |ui| {
            for t in &c.targets {
                let (n, sub) = names(t);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(kind_icon(t.kind)).color(p.muted));
                    ui.add(egui::Label::new(RichText::new(format!("{n}   {sub}")).size(12.5)).truncate());
                });
            }
        },
    );

    if flash > 0 && ssd_warning {
        ui.add_space(4.0);
        if compact {
            widgets::hint(ui, &format!("⚠ {}", tr!("gui.flash_short", n = flash)));
        } else {
            widgets::warning(ui, &tr!("gui.flash_banner", n = flash));
        }
    }
    for b in blocked.iter().take(3) {
        ui.label(RichText::new(format!("✖ {b}")).color(p.err).size(12.5));
    }

    let mut typed_ok = true;
    if need_pw {
        ui.add_space(8.0);
        ui.label(format!("🔒 {}", tr!("gui.pw_for_wipe")));
        let r = ui.add(egui::TextEdit::singleline(&mut c.password).password(true).desired_width(f32::INFINITY));
        if !c.focused {
            c.focused = true;
            r.request_focus();
        }
        if let Some(e) = &c.pw_error {
            ui.label(RichText::new(e).color(p.err));
        }
        typed_ok = !c.password.is_empty();
    }
    if mode == ConfirmMode::Typed && !need_pw {
        ui.add_space(8.0);
        ui.label(tr!("gui.type_to_confirm", word = word));
        let r = ui.add(
            egui::TextEdit::singleline(&mut c.typed).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace),
        );
        // Focus once; asking every frame would repaint endlessly while the
        // window itself is not focused.
        if !c.focused {
            c.focused = true;
            r.request_focus();
        }
        typed_ok = c.typed.trim().eq_ignore_ascii_case(&word) || c.typed.trim().eq_ignore_ascii_case("yes");
    }
    let ok = ready && typed_ok && !runnable.is_empty();

    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(ok, |ui| widgets::primary(ui, tr!("gui.wipe_now"))).inner.clicked() {
            outcome = ConfirmOutcome::Go;
        }
        if widgets::secondary(ui, tr!("gui.cancel")).clicked() {
            outcome = ConfirmOutcome::Cancel;
        }
    });
    // Keyboard: Enter confirms (when allowed), Escape cancels.
    let (enter, esc) = ui.ctx().input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if enter && ok {
        outcome = ConfirmOutcome::Go;
    }
    if esc {
        outcome = ConfirmOutcome::Cancel;
    }
    // Check the password only when the user actually confirms.
    if outcome == ConfirmOutcome::Go && need_pw {
        let pw = std::mem::take(&mut c.password);
        if !lock.verify(&pw) {
            c.pw_error = Some(tr!("lock.wrong"));
            c.focused = false;
            return ConfirmOutcome::Pending;
        }
    }
    outcome
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    super::inuse::panel(app, ui, false);
    let has_tray = app.tray.is_some();
    let mut to_tray = false;
    let Some(jv) = app.job.as_mut() else { return };

    let frac = if jv.total > 0 {
        (jv.done as f64 / jv.total as f64).min(1.0)
    } else if jv.finished {
        1.0
    } else {
        0.0
    };
    let n = jv.targets.len();
    let cur = jv.current.map(|c| c + 1).unwrap_or(n);
    let paused = jv.job.cancel.is_paused();

    ui.horizontal(|ui| {
        let title = if jv.finished {
            tr!("gui.job_done")
        } else if paused {
            tr!("gui.job_paused")
        } else {
            tr!("gui.job_running")
        };
        ui.heading(title);
        ui.label(RichText::new(tr!("gui.job_sub", i = cur, n = n, method = jv.opts.method.name())).color(p.muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{:.0}%", frac * 100.0)).size(30.0).monospace());
        });
    });
    ui.add_space(4.0);
    ui.add(
        egui::ProgressBar::new(frac as f32).fill(p.accent).desired_height(10.0).corner_radius(CornerRadius::same(5)),
    );
    let el = jv.active_elapsed();
    let speed = if el.as_secs_f64() > 0.5 { jv.done as f64 / el.as_secs_f64() } else { 0.0 };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} / {}", human_bytes(jv.done), human_bytes(jv.total)))
                .monospace()
                .size(12.0)
                .color(p.muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let eta = if speed > 0.0 && !jv.finished {
                human_duration(Duration::from_secs_f64((jv.total.saturating_sub(jv.done)) as f64 / speed))
            } else {
                "—".into()
            };
            ui.label(
                RichText::new(tr!(
                    "gui.speed_eta",
                    speed = format!("{}/s", human_bytes(speed as u64)),
                    eta = eta,
                    elapsed = human_duration(el)
                ))
                .monospace()
                .size(12.0)
                .color(p.muted),
            );
        });
    });
    ui.add_space(12.0);

    // Two cards: current item, queue status (side by side at any width so
    // the log and the buttons below always stay on screen).
    let card_h = if ui.available_height() < 420.0 { 110.0 } else { 160.0 };
    super::columns2_fixed(ui, |ui, col| {
        if col == 0 {
            widgets::card(ui, |ui| {
                ui.set_min_height(card_h);
                widgets::section(ui, &tr!("gui.current"));
                ui.add(
                    egui::Label::new(
                        RichText::new(if jv.item.is_empty() { "—".into() } else { jv.item.clone() }).strong(),
                    )
                    .truncate(),
                );
                ui.label(RichText::new(format!("{}   {}", jv.pass, jv.stage)).monospace().size(12.0).color(p.muted));
                ui.add_space(6.0);
                let flash = jv.current.and_then(|i| jv.flash.get(i)).copied().unwrap_or(false);
                let eff = jv.opts.effective_for(flash);
                let passes = eff.method.passes(eff.passes_override);
                let cur_pass = jv.pass_i;
                ui.horizontal_wrapped(|ui| {
                    for (i, ps) in passes.iter().enumerate().take(35) {
                        let (fg, bg) = if jv.finished || i < cur_pass {
                            (p.ok, p.ok_bg)
                        } else if i == cur_pass {
                            (p.accent, p.accent_soft)
                        } else {
                            (p.muted, p.card_hi)
                        };
                        let text = if passes.len() <= 7 {
                            format!("{} · {}", i + 1, ps.label())
                        } else {
                            format!("{}", i + 1)
                        };
                        widgets::badge(ui, text, fg, bg);
                    }
                });
            });
        } else {
            widgets::card(ui, |ui| {
                ui.set_min_height(card_h);
                widgets::section(ui, &tr!("gui.nav_queue"));
                egui::ScrollArea::vertical().id_salt("jobq").max_height(140.0).show(ui, |ui| {
                    for (i, t) in jv.targets.iter().enumerate() {
                        let (name, _) = names(t);
                        let (icon, color, extra) = match &jv.results[i] {
                            Some(r) => match r.status {
                                Status::Ok => ("✔", p.ok, human_bytes(r.bytes_overwritten)),
                                Status::Partial => {
                                    ("⚠", p.warn_icon, tr!("cli.partial_detail", ok = r.files_ok, err = r.files_err))
                                }
                                Status::Cancelled => ("■", p.muted, tr!("err.cancelled")),
                                _ => ("✖", p.err, r.error.clone().unwrap_or_default()),
                            },
                            None if jv.current == Some(i) => ("▶", p.accent, String::new()),
                            None => ("○", p.muted, String::new()),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).color(color));
                            ui.add(egui::Label::new(name).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(extra).size(12.0).color(p.muted))
                                        .truncate()
                                        .selectable(true),
                                );
                            });
                        });
                    }
                });
            });
        }
    });

    ui.add_space(12.0);
    widgets::section(ui, &tr!("gui.log"));
    let log_h = (ui.available_height() - 84.0).max(40.0);
    Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("log")
                .max_height(log_h)
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for l in &jv.log {
                        ui.add(
                            egui::Label::new(RichText::new(l).monospace().size(12.0).color(p.muted)).selectable(true),
                        );
                    }
                });
        });

    ui.add_space(10.0);
    let mut close = false;
    let mut retry = false;
    let denied = jv.finished && jv.results.iter().any(|r| r.as_ref().map(|r| r.access_denied).unwrap_or(false));
    let hint = if denied {
        tr!("gui.denied_hint")
    } else if jv.finished {
        tr!("gui.job_finished_hint")
    } else {
        tr!("gui.stop_safe")
    };
    ui.horizontal(|ui| {
        // Buttons first (right to left); the hint gets what is left and is
        // shortened rather than drawn under the buttons.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if jv.finished {
                if widgets::primary(ui, tr!("gui.back_to_queue")).clicked() {
                    close = true;
                }
                if denied && widgets::secondary(ui, format!("🛡 {}", tr!("gui.retry_admin"))).clicked() {
                    retry = true;
                }
                if let Some(path) = jv.report_path.clone() {
                    if widgets::secondary(ui, tr!("gui.open_report")).clicked() {
                        open_path(&path);
                    }
                }
            } else {
                if widgets::danger(ui, tr!("gui.stop")).clicked() {
                    jv.job.cancel.set_paused(false);
                    jv.job.cancel.cancel();
                    jv.log(tr!("gui.log_stopping"));
                }
                let pause_label = if paused { tr!("gui.resume") } else { tr!("gui.pause") };
                if has_tray && widgets::secondary(ui, tr!("gui.to_tray")).clicked() {
                    to_tray = true;
                }
                if widgets::secondary(ui, pause_label).clicked() {
                    if paused {
                        if let Some(s) = jv.paused_since.take() {
                            jv.paused_total += s.elapsed();
                        }
                    } else {
                        jv.paused_since = Some(std::time::Instant::now());
                    }
                    jv.job.cancel.set_paused(!paused);
                }
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(p.muted)).truncate())
                    .on_hover_text(String::new());
            });
        });
    });
    #[cfg(windows)]
    if retry && app.retry_as_admin() {
        app.toast(tr!("gui.retry_started"));
    }
    #[cfg(not(windows))]
    let _ = retry;
    if close {
        app.close_job();
    }
    if to_tray {
        app.hide_to_tray();
    }
    if app.is_running() {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
}
