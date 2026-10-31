//! Free space: every volume with its file system, medium and usage.

use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Ui};

use freewipe::plan::Target;
use freewipe::util::human_bytes;
use freewipe::{filesystem, media, sys, tr};

use super::queue::media_colors;
use super::title;
use crate::app::{App, VolumeRow};
use crate::theme::pal;
use crate::widgets;

fn load() -> Vec<VolumeRow> {
    sys::list_volumes()
        .into_iter()
        .filter_map(|root| {
            let info = filesystem::probe(&root).ok();
            // Skip empty card readers / optical drives without media and
            // read-only pseudo file systems.
            let i = info.as_ref()?;
            if i.total == 0 || matches!(i.fs_name.as_str(), "squashfs" | "iso9660" | "overlay" | "erofs") {
                return None;
            }
            Some(VolumeRow { media: media::probe(&root), info, root })
        })
        .collect()
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let mut refresh = false;
    title(ui, &tr!("gui.fs_title"), |ui| {
        if widgets::secondary(ui, tr!("gui.refresh")).clicked() {
            refresh = true;
        }
    });
    if refresh || app.volumes.is_none() {
        app.volumes = Some(load());
    }
    widgets::hint(ui, &tr!("gui.fs_hint"));
    ui.add_space(10.0);

    let mut add: Vec<Target> = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::card(ui, |ui| {
            ui.label(RichText::new(tr!("gui.fs_extra")).strong());
            ui.add_space(4.0);
            widgets::check_row(ui, &mut app.settings.freespace_tips, &tr!("gui.fs_tips"), &tr!("gui.fs_tips_sub"));
            widgets::check_row(
                ui,
                &mut app.settings.freespace_records,
                &tr!("gui.fs_records"),
                &tr!("gui.fs_records_sub"),
            );
        });
        ui.add_space(10.0);
        for v in app.volumes.as_deref().unwrap_or_default() {
            let Some(info) = &v.info else { continue };
            let queued = app.queue.iter().any(|q| q.target == Target::free_space(&v.root));
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🖴").size(22.0).color(p.muted));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(v.root.display().to_string()).strong().size(16.0));
                            if !info.label.is_empty() {
                                ui.label(RichText::new(&info.label).color(p.muted));
                            }
                            widgets::badge(ui, info.fs.display(), p.muted, p.card_hi);
                            let (fg, bg) = media_colors(ui, v.media.kind);
                            widgets::badge(ui, v.media.label(), fg, bg);
                            if let Some(m) = v.media.model.as_ref().filter(|_| ui.available_width() > 260.0) {
                                ui.add(egui::Label::new(RichText::new(m).size(12.0).color(p.muted)).truncate());
                            }
                        });
                        let used = info.total.saturating_sub(info.free);
                        let frac = if info.total > 0 { used as f32 / info.total as f32 } else { 0.0 };
                        ui.add(
                            egui::ProgressBar::new(frac)
                                .desired_width((ui.available_width() - 150.0).clamp(120.0, 360.0))
                                .desired_height(6.0)
                                .fill(p.accent)
                                .corner_radius(CornerRadius::same(3)),
                        );
                        ui.label(
                            RichText::new(tr!(
                                "gui.fs_free",
                                free = human_bytes(info.free),
                                total = human_bytes(info.total)
                            ))
                            .size(12.5)
                            .color(p.muted),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if queued {
                            ui.label(RichText::new(format!("✔ {}", tr!("gui.in_queue"))).color(p.ok));
                        } else if widgets::secondary(ui, tr!("gui.add_to_queue")).clicked() {
                            add.push(Target::free_space(&v.root));
                        }
                    });
                });
                if v.media.is_flash_likely() && app.settings.ssd_warning {
                    widgets::hint(ui, &tr!("gui.fs_flash_hint"));
                }
                for w in info.fs.warnings().into_iter().filter(|_| info.fs.is_copy_on_write()) {
                    widgets::hint(ui, &w);
                }
            });
            ui.add_space(8.0);
        }

        // Recycle Bin.
        let queued = app.queue.iter().any(|q| q.target == Target::recycle_bin());
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🗑").size(22.0).color(p.muted));
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr!("kind.recycle_bin")).strong().size(16.0));
                    widgets::hint(ui, &tr!("gui.recycle_hint"));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if queued {
                        ui.label(RichText::new(format!("✔ {}", tr!("gui.in_queue"))).color(p.ok));
                    } else if widgets::secondary(ui, tr!("gui.add_to_queue")).clicked() {
                        add.push(Target::recycle_bin());
                    }
                });
            });
        });
    });
    if !add.is_empty() {
        app.add_targets(add);
        app.toast(tr!("gui.added"));
    }
}
