//! Main window: the wipe queue and the action bar.

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Shape, Stroke, Ui, Vec2};

use freewipe::media::MediaKind;
use freewipe::plan::TargetKind;
use freewipe::util::human_bytes;
use freewipe::{tr, Method};

use freewipe::plan::DirFilter;

use super::{cell, title};
use crate::app::{App, View};
use crate::theme::pal;
use crate::widgets;

pub fn kind_icon(k: TargetKind) -> &'static str {
    match k {
        TargetKind::File => "🗋",
        TargetKind::Dir => "🗀",
        TargetKind::FreeSpace => "🖴",
        TargetKind::RecycleBin => "🗑",
        TargetKind::Move => "➜",
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    super::legacy::banner(app, ui);
    title(ui, &tr!("gui.queue_title"), |ui| {
        if !app.queue.is_empty() && widgets::secondary(ui, tr!("gui.clear")).clicked() {
            app.queue.clear();
        }
        if widgets::secondary(ui, tr!("gui.add_freespace")).clicked() {
            app.view = View::FreeSpace;
        }
        if widgets::secondary(ui, tr!("gui.add_folder")).clicked() {
            if let Some(d) = rfd::FileDialog::new().set_title(tr!("gui.add_folder")).pick_folders() {
                app.add_paths(d);
            }
        }
        if widgets::secondary(ui, tr!("gui.add_files")).clicked() {
            if let Some(f) = rfd::FileDialog::new().set_title(tr!("gui.add_files")).pick_files() {
                app.add_paths(f);
            }
        }
    });

    drop_zone(ui);
    ui.add_space(10.0);

    if app.queue.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(tr!("gui.queue_empty")).color(p.muted).size(15.0));
            ui.add_space(4.0);
            widgets::hint(ui, &tr!("gui.queue_empty_hint"));
        });
        return;
    }

    // Flash / file-system warnings for what is queued.
    let flash = app
        .queue
        .iter()
        .filter(|q| q.plan.as_ref().and_then(|p| p.media.as_ref()).map(|m| m.is_flash_likely()).unwrap_or(false))
        .count();

    let bottom_space = if flash > 0 && app.settings.ssd_warning { 70.0 } else { 0.0 };
    let table_h = (ui.available_height() - bottom_space).max(120.0);

    Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let w = ui.available_width() - 16.0;
        // Narrow windows drop the less important columns first.
        let (show_type, show_fs, show_media) = (w > 860.0, w > 700.0, w > 600.0);
        let size_w = 110.0;
        let mut fixed = 30.0 + size_w + 36.0 + 36.0 + 5.0 * 8.0;
        if show_type {
            fixed += 100.0 + 8.0;
        }
        if show_fs {
            fixed += 72.0 + 8.0;
        }
        if show_media {
            fixed += 120.0 + 8.0;
        }
        let name_w = (w - fixed).max(120.0);
        // Header
        Frame::new()
            .fill(p.panel)
            .inner_margin(Margin::symmetric(8, 0))
            .corner_radius(CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let h = |ui: &mut Ui, w: f32, t: String| {
                        cell(ui, w, 34.0, |ui| {
                            ui.label(RichText::new(t.to_uppercase()).size(11.5).color(p.muted).strong())
                        });
                    };
                    h(ui, 30.0, String::new());
                    h(ui, name_w, tr!("gui.col_object"));
                    if show_type {
                        h(ui, 100.0, tr!("gui.col_type"));
                    }
                    h(ui, size_w, tr!("gui.col_size"));
                    if show_fs {
                        h(ui, 72.0, tr!("gui.col_fs"));
                    }
                    if show_media {
                        h(ui, 120.0, tr!("gui.col_media"));
                    }
                });
            });
        let mut remove: Option<usize> = None;
        let mut edit: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(table_h - 40.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, q) in app.queue.iter().enumerate() {
                ui.painter().hline(
                    ui.available_rect_before_wrap().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, p.border),
                );
                Frame::new().inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        cell(ui, 30.0, 48.0, |ui| {
                            ui.label(RichText::new(kind_icon(q.target.kind)).size(18.0).color(p.muted))
                        });
                        cell(ui, name_w, 48.0, |ui| {
                            ui.vertical(|ui| {
                                ui.add_space(4.0);
                                let (name, mut sub) = names(&q.target);
                                if let Some(pl) = q
                                    .plan
                                    .as_ref()
                                    .filter(|_| matches!(q.target.kind, TargetKind::Dir | TargetKind::RecycleBin))
                                {
                                    sub = format!("{sub}  ·  {}", tr!("cli.files_n", n = pl.files));
                                }
                                if let Some(n) = q.plan.as_ref().and_then(|p| p.note()) {
                                    sub = format!("{sub}  ·  {n}");
                                }
                                if let Some(f) = &q.target.filter {
                                    sub = format!("{sub}  ·  {}", tr!("gui.filter_short", f = f.summary()));
                                }
                                ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                                let sub = match (&q.last_error, q.plan.as_ref().and_then(|p| p.blocked.clone())) {
                                    (_, Some(b)) => RichText::new(b).color(p.err).size(12.0),
                                    (Some(e), _) => RichText::new(e.clone()).color(p.err).size(12.0),
                                    _ => RichText::new(sub).color(p.muted).size(12.0).monospace(),
                                };
                                ui.add(egui::Label::new(sub).truncate());
                            });
                        });
                        if show_type {
                            cell(ui, 100.0, 48.0, |ui| ui.label(RichText::new(q.target.kind.label()).color(p.muted)));
                        }
                        let (size, fs, media) = match &q.plan {
                            None => ("…".to_string(), "…".to_string(), None),
                            Some(pl) => (
                                match q.target.kind {
                                    TargetKind::File => human_bytes(pl.bytes),
                                    TargetKind::FreeSpace => tr!("cli.free_n", size = human_bytes(pl.bytes)),
                                    _ => human_bytes(pl.bytes),
                                },
                                pl.volume.as_ref().map(|v| v.fs.display()).unwrap_or_else(|| "—".into()),
                                pl.media.clone(),
                            ),
                        };
                        cell(ui, size_w, 48.0, |ui| {
                            ui.add(egui::Label::new(RichText::new(size).monospace()).truncate())
                        });
                        if show_fs {
                            cell(ui, 72.0, 48.0, |ui| ui.label(RichText::new(fs).monospace().color(p.muted)));
                        }
                        if show_media {
                            cell(ui, 120.0, 48.0, |ui| {
                                if let Some(m) = media {
                                    let (fg, bg) = media_colors(ui, m.kind);
                                    widgets::badge(ui, m.label(), fg, bg);
                                }
                            });
                        }
                        cell(ui, 36.0, 48.0, |ui| {
                            if q.target.kind == TargetKind::Dir {
                                let on = q.target.filter.is_some();
                                let icon = RichText::new("⛃").color(if on { p.accent } else { p.muted });
                                if ui
                                    .add(egui::Button::new(icon).frame(false))
                                    .on_hover_text(tr!("gui.filter_edit"))
                                    .clicked()
                                {
                                    edit = Some(i);
                                }
                            }
                        });
                        cell(ui, 36.0, 48.0, |ui| {
                            if ui
                                .add(egui::Button::new(RichText::new("✖").color(p.muted)).frame(false))
                                .on_hover_text(tr!("gui.remove"))
                                .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    });
                });
            }
        });
        if let Some(i) = remove {
            app.queue.remove(i);
        }
        if let Some(i) = edit {
            let f = app.queue[i].target.filter.clone().unwrap_or_default();
            app.filter_edit = Some(FilterEdit {
                index: i,
                include: f.include.join("; "),
                exclude: f.exclude.join("; "),
                contents_only: f.contents_only,
            });
        }
    });
    filter_dialog(app, ui);

    if flash > 0 && app.settings.ssd_warning {
        ui.add_space(12.0);
        widgets::warning(ui, &tr!("gui.flash_banner", n = flash));
    }
}

/// Editing the filter of a queued folder.
pub struct FilterEdit {
    pub index: usize,
    pub include: String,
    pub exclude: String,
    pub contents_only: bool,
}

fn filter_dialog(app: &mut App, ui: &mut Ui) {
    let Some(fe) = app.filter_edit.as_mut() else { return };
    let mut close = false;
    let mut save = false;
    let folder = app.queue.get(fe.index).map(|q| q.target.path.display().to_string()).unwrap_or_default();
    let resp = egui::Modal::new(egui::Id::unique("filter-dialog")).show(ui.ctx(), |ui| {
        ui.set_width(460.0);
        ui.heading(tr!("gui.filter_title"));
        widgets::hint(ui, &folder);
        ui.add_space(8.0);
        ui.label(tr!("gui.filter_include"));
        ui.add(egui::TextEdit::singleline(&mut fe.include).hint_text("*.tmp; *.log").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        ui.label(tr!("gui.filter_exclude"));
        ui.add(egui::TextEdit::singleline(&mut fe.exclude).hint_text("*.docx; keep").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        widgets::check_row(ui, &mut fe.contents_only, &tr!("gui.filter_contents"), &tr!("gui.filter_contents_sub"));
        widgets::hint(ui, &tr!("gui.filter_hint"));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, tr!("gui.save")).clicked() {
                save = true;
            }
            if widgets::secondary(ui, tr!("gui.cancel")).clicked() {
                close = true;
            }
        });
    });
    if save {
        let f = DirFilter {
            include: DirFilter::parse_masks(&fe.include),
            exclude: DirFilter::parse_masks(&fe.exclude),
            contents_only: fe.contents_only,
        };
        let i = fe.index;
        if let Some(q) = app.queue.get_mut(i) {
            q.target.filter = (!f.is_empty()).then_some(f);
        }
        app.filter_edit = None;
        app.rescan_queue();
    } else if close || resp.should_close() {
        app.filter_edit = None;
    }
}

/// `(display name, secondary line)` for a target.
pub fn names(t: &freewipe::plan::Target) -> (String, String) {
    match t.kind {
        TargetKind::RecycleBin => (tr!("kind.recycle_bin"), tr!("gui.recycle_sub")),
        TargetKind::FreeSpace => (tr!("gui.freespace_of", vol = t.path.display()), t.path.display().to_string()),
        _ => {
            let name = t
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| t.path.display().to_string());
            let parent = t.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            (name, parent)
        }
    }
}

pub fn media_colors(ui: &Ui, k: MediaKind) -> (egui::Color32, egui::Color32) {
    let p = pal(ui);
    match k {
        MediaKind::Ssd | MediaKind::Flash => (p.flash_text, p.flash_bg),
        MediaKind::Hdd => (p.ok, p.ok_bg),
        _ => (p.muted, p.card_hi),
    }
}

fn drop_zone(ui: &mut Ui) {
    let p = pal(ui);
    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::hover());
    let color = if hovering { p.accent } else { p.border.gamma_multiply(1.4) };
    if hovering {
        ui.painter().rect_filled(rect, 10.0, p.accent_soft);
    }
    let r = rect.shrink(1.0);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    ui.painter().extend(Shape::dashed_line(&pts, Stroke::new(1.2, color), 6.0, 5.0));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("⬇  {}", tr!("gui.drop_hint")),
        egui::FontId::proportional(14.0),
        if hovering { p.text } else { p.muted },
    );
}

/// Bottom bar: method, verify, keep, big Wipe button.
pub fn action_bar(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    // Width of the area beside the navigation; long translations need two
    // rows (settings on top, buttons below) on narrower windows.
    let w = ui.ctx().content_rect().width() - 214.0 - 52.0;
    let two_rows = w < 1010.0;
    let combo_w = if two_rows { (w - 380.0).clamp(170.0, 320.0) } else { 300.0 };
    egui::Panel::bottom("actions")
        .exact_size(if two_rows { 118.0 } else { 76.0 })
        .resizable(false)
        .frame(Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.border)).inner_margin(Margin::symmetric(26, 10)))
        .show(ui, |ui| {
            let settings_row = |app: &mut App, ui: &mut Ui| {
                ui.label(RichText::new(tr!("gui.method")).color(p.muted));
                egui::ComboBox::from_id_salt("method")
                    .width(combo_w)
                    .truncate()
                    .selected_text(method_label(app.method, app.settings.random_passes))
                    .show_ui(ui, |ui| {
                        for m in Method::all() {
                            let r =
                                ui.selectable_value(&mut app.method, m, method_label(m, app.settings.random_passes));
                            r.on_hover_text(m.localized_description());
                        }
                    });
                ui.add_space(6.0);
                ui.checkbox(&mut app.verify, tr!("gui.verify"));
                ui.checkbox(&mut app.keep, tr!("gui.keep"));
            };
            if two_rows {
                ui.horizontal(|ui| settings_row(app, ui));
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| buttons(app, ui));
            } else {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    settings_row(app, ui);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| buttons(app, ui));
                });
            }
        });
}

/// "Wipe N item(s)" and "Move & wipe…", laid out right to left.
fn buttons(app: &mut App, ui: &mut Ui) {
    let runnable: Vec<_> = app
        .queue
        .iter()
        .filter(|q| q.plan.as_ref().map(|p| p.is_ok()).unwrap_or(true))
        .map(|q| q.target.clone())
        .collect();
    let scanning = app.queue.iter().any(|q| q.plan.is_none());
    let label = tr!("gui.wipe_n", n = runnable.len());
    let enabled = !runnable.is_empty() && !scanning && !app.is_running();
    let btn = ui.add_enabled_ui(enabled, |ui| widgets::primary(ui, format!("🗑  {label}"))).inner;
    if btn.clicked() {
        let opts = app.current_options();
        app.request_wipe(runnable.clone(), opts, false);
    }
    // Move & wipe: files and folders only.
    let movable: Vec<std::path::PathBuf> = runnable
        .iter()
        .filter(|t| matches!(t.kind, TargetKind::File | TargetKind::Dir))
        .map(|t| t.path.clone())
        .collect();
    let mv = ui
        .add_enabled_ui(enabled && !movable.is_empty(), |ui| {
            widgets::secondary(ui, format!("➜  {}", tr!("gui.move_wipe")))
        })
        .inner
        .on_hover_text(tr!("gui.move_wipe_hint"));
    if mv.clicked() {
        if let Some(dest) = app.pick_move_destination(&movable) {
            let targets = movable.into_iter().map(|p| freewipe::plan::Target::move_to(p, &dest)).collect();
            let opts = app.current_options();
            app.request_wipe(targets, opts, false);
        }
    }
}

pub fn method_label(m: Method, random_passes: usize) -> String {
    let n = if m == Method::RandomN { random_passes.max(1) } else { m.pass_count(None) };
    format!("{} · {}", m.name(), tr!("cli.passes_n", n = n))
}
