//! Saved JSON reports.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use freewipe::report::Report;
use freewipe::tr;
use freewipe::util::human_bytes;

use super::title;
use crate::app::{open_path, App, ReportRow};
use crate::theme::pal;
use crate::widgets;

/// freewipe's own report files in `dir` (`freewipe-*.json`).
fn report_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("json")
                && p.file_name().map(|n| n.to_string_lossy().starts_with("freewipe-")).unwrap_or(false)
        })
        .collect()
}

fn load(dir: &std::path::Path) -> Vec<ReportRow> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<_> =
        rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).collect();
    files.sort();
    files.reverse();
    files.truncate(200);
    files
        .into_iter()
        .map(|path| {
            let report = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Report>(&s).ok());
            ReportRow { path, report }
        })
        .collect()
}

pub fn show(app: &mut App, ui: &mut Ui) {
    if !super::lock::gate(app, ui, super::lock::Gated::Reports) {
        return;
    }
    let p = pal(ui);
    let dir = app.settings.report_dir();
    let mut refresh = false;
    let mut ask: Option<bool> = None;
    let any = app.reports.as_ref().map(|r| !r.is_empty()).unwrap_or(false);
    title(ui, &tr!("gui.reports_title"), |ui| {
        if any && widgets::danger(ui, tr!("gui.reports_wipe_all")).clicked() {
            ask = Some(true);
        }
        if any && widgets::secondary(ui, tr!("gui.reports_delete_all")).clicked() {
            ask = Some(false);
        }
        if widgets::secondary(ui, tr!("gui.open_folder")).clicked() {
            let _ = std::fs::create_dir_all(&dir);
            open_path(&dir);
        }
        if widgets::secondary(ui, tr!("gui.refresh")).clicked() {
            refresh = true;
        }
    });
    if ask.is_some() {
        app.reports_confirm = ask;
    }
    confirm_clear(app, ui, &dir);
    if refresh || app.reports.is_none() {
        app.reports = Some(load(&dir));
    }
    ui.add(
        egui::Label::new(RichText::new(dir.display().to_string()).monospace().size(12.0).color(p.muted))
            .selectable(true),
    );
    if !app.settings.save_reports {
        widgets::hint(ui, &tr!("gui.reports_off"));
    }
    ui.add_space(10.0);

    let rows = app.reports.as_deref().unwrap_or_default();
    if rows.is_empty() {
        ui.label(RichText::new(tr!("gui.reports_empty")).color(p.muted));
        return;
    }
    let mut delete: Option<std::path::PathBuf> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for r in rows {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| match &r.report {
                    Some(rep) => {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(rep.started.replace('T', " ").replace('Z', " UTC")).strong());
                            ui.label(
                                RichText::new(format!("{} · {}", rep.method_name, tr!("cli.passes_n", n = rep.passes)))
                                    .color(p.muted)
                                    .size(12.5),
                            );
                        });
                        ui.add_space(20.0);
                        let s = &rep.summary;
                        if s.ok > 0 {
                            widgets::badge(ui, format!("✔ {}", s.ok), p.ok, p.ok_bg);
                        }
                        if s.failed + s.partial > 0 {
                            widgets::badge(ui, format!("✖ {}", s.failed + s.partial), p.err, p.accent_soft);
                        }
                        if s.cancelled + s.skipped > 0 {
                            widgets::badge(ui, format!("■ {}", s.cancelled + s.skipped), p.muted, p.card_hi);
                        }
                        ui.label(RichText::new(human_bytes(s.bytes_overwritten)).monospace().color(p.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✖").on_hover_text(tr!("gui.reports_delete_one")).clicked() {
                                delete = Some(r.path.clone());
                            }
                            if widgets::secondary(ui, tr!("gui.open")).clicked() {
                                open_path(&r.path);
                            }
                        });
                    }
                    None => {
                        ui.add(egui::Label::new(r.path.display().to_string()).selectable(true));
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
    if let Some(f) = delete {
        let _ = std::fs::remove_file(&f);
        app.reports = None;
    }
}

/// "Delete all reports?" / "Wipe all reports?"
fn confirm_clear(app: &mut App, ui: &mut Ui, dir: &std::path::Path) {
    let Some(wipe) = app.reports_confirm else { return };
    let files = report_files(dir);
    let mut go = false;
    let mut cancel = false;
    let resp = egui::Modal::new(egui::Id::unique("reports-clear")).show(ui.ctx(), |ui| {
        ui.set_width(440.0);
        ui.heading(if wipe { tr!("gui.reports_wipe_title") } else { tr!("gui.reports_delete_title") });
        ui.add_space(4.0);
        widgets::hint(
            ui,
            &if wipe {
                tr!("gui.reports_wipe_body", n = files.len())
            } else {
                tr!("gui.reports_delete_body", n = files.len())
            },
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = if wipe { tr!("gui.reports_wipe_all") } else { tr!("gui.reports_delete_all") };
            if widgets::danger(ui, label).clicked() {
                go = true;
            }
            if widgets::secondary(ui, tr!("gui.cancel")).clicked() {
                cancel = true;
            }
        });
    });
    if go {
        app.reports_confirm = None;
        if wipe {
            let targets = files.into_iter().map(freewipe::plan::Target::file).collect();
            let opts = app.current_options();
            // This job leaves no new report behind.
            app.skip_report_once = true;
            app.request_wipe(targets, opts, true);
        } else {
            for f in files {
                let _ = std::fs::remove_file(f);
            }
            app.toast(tr!("gui.reports_deleted"));
        }
        app.reports = None;
    } else if cancel || resp.should_close() {
        app.reports_confirm = None;
    }
}
