//! Compact window opened from the Explorer context menu ("wipe right away"):
//! confirmation and/or password (if enabled), then a small progress window
//! that shows the result and — if configured — closes itself after a clean
//! run.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Ui};

use freewipe::report::Status;
use freewipe::tr;
use freewipe::util::{human_bytes, human_duration};

use super::job::{apply_outcome, confirm_body};
use super::queue::names;
use crate::app::{open_path, App};
use crate::theme::pal;
use crate::widgets;

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let margin = if app.job.is_some() { Margin::symmetric(16, 12) } else { Margin::symmetric(22, 18) };
    egui::CentralPanel::default().frame(Frame::new().fill(p.bg).inner_margin(margin)).show(ui, |ui| {
        if app.confirm.is_some() {
            let outcome = confirm_body(app, ui);
            let cancelled = outcome == super::job::ConfirmOutcome::Cancel;
            apply_outcome(app, outcome);
            if cancelled {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if app.job.is_some() {
            let in_use = app.job.as_ref().map(|j| j.finished && !j.locked_files.is_empty()).unwrap_or(false);
            if in_use {
                let grown = egui::Id::unique("mini-grown");
                if !ui.ctx().data(|d| d.get_temp::<bool>(grown).unwrap_or(false)) {
                    ui.ctx().data_mut(|d| d.insert_temp(grown, true));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(MINI_W + 60.0, MINI_H + 150.0)));
                }
                super::inuse::panel(app, ui, true);
            }
            progress(app, ui);
            return;
        }
        // Waiting for the request (Explorer may still be starting siblings).
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.spinner();
            ui.label(RichText::new(tr!("gui.quick_preparing")).color(p.muted));
        });
    });
}

/// Small progress window (shown after confirmation, or straight away when
/// no confirmation is needed).
fn progress(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    if !app.mini_applied {
        app.mini_applied = true;
        let ctx = ui.ctx();
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(380.0, 200.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(MINI_W, MINI_H)));
        if app.settings.mini_on_top {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        }
    }
    let close_secs = app.settings.finish_close_secs;
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
    let paused = jv.job.cancel.is_paused();
    let n = jv.targets.len();
    let all_ok = jv.finished
        && jv.locked_files.is_empty()
        && jv.results.iter().all(|r| r.as_ref().map(|r| r.status == Status::Ok).unwrap_or(false));
    let denied = jv.finished && jv.results.iter().any(|r| r.as_ref().map(|r| r.access_denied).unwrap_or(false));

    // Title + percent.
    ui.horizontal(|ui| {
        let title = if !jv.finished {
            if paused {
                tr!("gui.job_paused")
            } else {
                tr!("gui.job_running")
            }
        } else if all_ok {
            tr!("gui.quick_done")
        } else {
            tr!("gui.quick_problems")
        };
        ui.label(RichText::new(title).size(18.0).strong());
        if n > 1 && !jv.finished {
            let i = jv.current.map(|c| c + 1).unwrap_or(n);
            ui.label(RichText::new(tr!("gui.mini_item", i = i, n = n)).color(p.muted).size(12.5));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{:.0}%", frac * 100.0)).size(18.0).monospace());
        });
    });
    let fill = if jv.finished && !all_ok {
        p.warn_icon
    } else if jv.finished {
        p.ok
    } else {
        p.accent
    };
    ui.add(egui::ProgressBar::new(frac as f32).fill(fill).desired_height(8.0).corner_radius(CornerRadius::same(4)));
    ui.add_space(4.0);

    if !jv.finished {
        let name = jv.current.map(|c| names(&jv.targets[c]).0).unwrap_or_default();
        ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
        ui.label(RichText::new(format!("{}   {}", jv.pass, jv.stage)).monospace().size(11.5).color(p.muted));
        let el = jv.active_elapsed();
        let speed = if el.as_secs_f64() > 0.5 { jv.done as f64 / el.as_secs_f64() } else { 0.0 };
        let eta = if speed > 0.0 {
            human_duration(Duration::from_secs_f64(jv.total.saturating_sub(jv.done) as f64 / speed))
        } else {
            "—".into()
        };
        ui.label(
            RichText::new(format!(
                "{} / {} · {}/s · ≈ {eta}",
                human_bytes(jv.done),
                human_bytes(jv.total),
                human_bytes(speed as u64)
            ))
            .monospace()
            .size(11.5)
            .color(p.muted),
        );
    } else {
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 56.0).max(30.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, t) in jv.targets.iter().enumerate() {
                    let (name, _) = names(t);
                    let (icon, color, extra) = match &jv.results[i] {
                        Some(r) if r.status == Status::Ok => ("✔", p.ok, human_bytes(r.bytes_overwritten)),
                        Some(r) if r.status == Status::Cancelled => ("■", p.muted, tr!("err.cancelled")),
                        Some(r) => ("✖", p.err, r.error.clone().unwrap_or_default()),
                        None => ("■", p.muted, tr!("err.cancelled")),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.add(egui::Label::new(name).truncate());
                        ui.label(RichText::new(extra).size(11.5).color(p.muted));
                    });
                }
            });
    }

    // Auto-close countdown after a clean run.
    let mut close = false;
    let countdown_id = egui::Id::unique("mini-close-at");
    let mut keep_open = false;
    if all_ok && !denied && close_secs > 0 && !app.mini_keep_open {
        let at = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(countdown_id, Instant::now));
        let left = Duration::from_secs(u64::from(close_secs)).saturating_sub(at.elapsed());
        if left.is_zero() {
            close = true;
        } else {
            ui.horizontal(|ui| {
                widgets::hint(ui, &tr!("gui.mini_closing", s = left.as_secs() + 1));
                if ui.link(tr!("gui.mini_keep_open")).clicked() {
                    keep_open = true;
                }
            });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    let mut stop = false;
    let mut pause_toggle = false;
    let mut expand = false;
    let mut retry = false;
    let report = jv.report_path.clone();
    let finished = jv.finished;
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if finished {
                    if widgets::primary(ui, tr!("gui.close")).clicked() {
                        close = true;
                    }
                    if denied && widgets::secondary(ui, format!("🛡 {}", tr!("gui.retry_admin"))).clicked() {
                        retry = true;
                    }
                    if let Some(r) = &report {
                        if widgets::secondary(ui, tr!("gui.open_report")).clicked() {
                            open_path(r);
                            keep_open = true;
                        }
                    }
                    if ui.link(tr!("gui.open_freewipe")).clicked() {
                        expand = true;
                    }
                } else {
                    if widgets::danger(ui, tr!("gui.stop")).clicked() {
                        stop = true;
                    }
                    let label = if paused { tr!("gui.resume") } else { tr!("gui.pause") };
                    if widgets::secondary(ui, label).clicked() {
                        pause_toggle = true;
                    }
                    if has_tray && ui.link(tr!("gui.to_tray")).clicked() {
                        to_tray = true;
                    }
                }
            });
        });
    });

    if stop {
        jv.job.cancel.set_paused(false);
        jv.job.cancel.cancel();
    }
    if pause_toggle {
        if paused {
            if let Some(s) = jv.paused_since.take() {
                jv.paused_total += s.elapsed();
            }
        } else {
            jv.paused_since = Some(Instant::now());
        }
        jv.job.cancel.set_paused(!paused);
    }
    if keep_open {
        app.mini_keep_open = true;
    }
    if to_tray {
        app.hide_to_tray();
    }

    #[cfg(windows)]
    if retry && app.retry_as_admin() {
        close = true;
    }
    #[cfg(not(windows))]
    let _ = retry;

    if expand {
        app.close_job();
        app.expand();
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
    } else if close {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if app.is_running() {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
}

/// Size of the small progress window.
pub const MINI_W: f32 = 480.0;
pub const MINI_H: f32 = 250.0;
