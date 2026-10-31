//! Settings → Safety: the editable list of protected locations.
//!
//! Built-in entries (system folders, the profile folder) can be switched off
//! and restored; the user's own files and folders can be added, edited and
//! removed. Drive roots and freewipe itself are always protected.

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Stroke, Ui};

use freewipe::safety::{self, ProtectedPath};
use freewipe::tr;

use crate::app::App;
use crate::theme::pal;
use crate::widgets;

/// State of the add/edit dialog.
pub struct ProtEdit {
    /// Index in `settings.protection.extra` (`None` = new entry).
    index: Option<usize>,
    path: String,
    inside: bool,
    error: Option<String>,
}

impl ProtEdit {
    fn new() -> Self {
        ProtEdit { index: None, path: String::new(), inside: true, error: None }
    }
}

fn same(a: &str, b: &str) -> bool {
    let n = |s: &str| {
        let s = safety::expand_env(s.trim());
        let s = s.trim_end_matches(['\\', '/']).to_string();
        if cfg!(windows) {
            s.to_lowercase().replace('/', "\\")
        } else {
            s
        }
    };
    n(a) == n(b)
}

/// The list with its buttons.
pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let builtin = safety::builtin_entries();
    let prot = app.settings.protection.clone();

    ui.horizontal_wrapped(|ui| {
        widgets::section(ui, &tr!("prot.title"));
        if widgets::secondary(ui, format!("+ {}", tr!("prot.add"))).clicked() {
            app.prot_edit = Some(ProtEdit::new());
        }
        if !prot.disabled.is_empty() && ui.small_button(tr!("prot.restore_all")).clicked() {
            app.settings.protection.disabled.clear();
        }
    });
    ui.add_space(4.0);

    let mut toggle: Option<(String, bool)> = None;
    let mut edit: Option<usize> = None;
    let mut remove: Option<usize> = None;
    Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            let row = |ui: &mut Ui, path: &str, on: bool, inside: bool, add: &mut dyn FnMut(&mut Ui)| {
                let color = if on { p.text } else { p.muted };
                let mut text = RichText::new(path).monospace().size(12.0).color(color);
                if !on {
                    text = text.strikethrough();
                }
                ui.add(egui::Label::new(text).wrap());
                ui.horizontal_wrapped(|ui| {
                    let scope = if inside { tr!("prot.inside") } else { tr!("prot.exact") };
                    ui.label(RichText::new(scope).size(11.5).color(p.muted));
                    add(ui);
                });
                ui.add_space(4.0);
                ui.separator();
            };
            for e in &builtin {
                let on = !prot.disabled.iter().any(|d| same(d, &e.path));
                row(ui, &e.path, on, e.inside, &mut |ui| {
                    widgets::badge(ui, tr!("prot.builtin"), p.muted, p.card_hi);
                    if on {
                        if ui.small_button(tr!("prot.disable")).clicked() {
                            toggle = Some((e.path.clone(), false));
                        }
                    } else {
                        widgets::badge(ui, tr!("prot.off"), p.err, p.card_hi);
                        if ui.small_button(tr!("prot.restore")).clicked() {
                            toggle = Some((e.path.clone(), true));
                        }
                    }
                });
            }
            for (i, e) in prot.extra.iter().enumerate() {
                row(ui, &e.path, true, e.inside, &mut |ui| {
                    widgets::badge(ui, tr!("prot.user"), p.ok, p.ok_bg);
                    if ui.small_button(tr!("prot.edit")).clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button(tr!("prot.remove")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            for fixed in [tr!("gui.safety_roots"), tr!("gui.safety_self")] {
                ui.add(egui::Label::new(RichText::new(fixed).size(12.0).color(p.muted)).wrap());
                ui.horizontal_wrapped(|ui| {
                    widgets::badge(ui, tr!("prot.fixed"), p.muted, p.card_hi);
                });
                ui.add_space(4.0);
            }
        });
    widgets::hint(ui, &tr!("prot.hint"));

    let list = &mut app.settings.protection;
    if let Some((path, on)) = toggle {
        list.disabled.retain(|d| !same(d, &path));
        if !on {
            list.disabled.push(path);
        }
    }
    if let Some(i) = remove.filter(|i| *i < list.extra.len()) {
        list.extra.remove(i);
    }
    if let Some(i) = edit {
        if let Some(e) = list.extra.get(i) {
            app.prot_edit = Some(ProtEdit { index: Some(i), path: e.path.clone(), inside: e.inside, error: None });
        }
    }
    editor(app, ui);
}

fn editor(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let Some(ed) = app.prot_edit.as_mut() else { return };
    let (mut save, mut cancel) = (false, false);
    let resp = egui::Modal::new(egui::Id::unique("prot-editor")).show(ui.ctx(), |ui| {
        ui.set_width(ui.ctx().content_rect().width().clamp(300.0, 560.0) - 40.0);
        ui.heading(if ed.index.is_some() { tr!("prot.edit_title") } else { tr!("prot.add_title") });
        ui.add_space(6.0);
        ui.label(tr!("prot.path"));
        let r = ui.add(
            egui::TextEdit::singleline(&mut ed.path)
                .hint_text(if cfg!(windows) { r"D:\Archive" } else { "/srv/archive" })
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );
        if r.changed() {
            ed.error = None;
        }
        ui.horizontal_wrapped(|ui| {
            if ui.small_button(tr!("prot.folder")).clicked() {
                if let Some(d) = rfd::FileDialog::new().set_title(tr!("prot.folder")).pick_folder() {
                    ed.path = d.display().to_string();
                    ed.inside = true;
                    ed.error = None;
                }
            }
            if ui.small_button(tr!("prot.file")).clicked() {
                if let Some(f) = rfd::FileDialog::new().set_title(tr!("prot.file")).pick_file() {
                    ed.path = f.display().to_string();
                    ed.inside = false;
                    ed.error = None;
                }
            }
        });
        ui.add_space(6.0);
        ui.checkbox(&mut ed.inside, tr!("prot.inside_check"));
        let expanded = safety::expand_env(ed.path.trim());
        if !ed.path.trim().is_empty() {
            if expanded != ed.path.trim() {
                ui.label(RichText::new(format!("→ {expanded}")).monospace().size(11.5).color(p.muted));
            }
            if !std::path::Path::new(&expanded).exists() {
                ui.label(RichText::new(tr!("prot.missing")).size(11.5).color(p.warn_text));
            }
        }
        widgets::hint(ui, &tr!("prot.hint"));
        if let Some(e) = &ed.error {
            ui.label(RichText::new(e).color(p.err));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, tr!("gui.save")).clicked() {
                save = true;
            }
            if widgets::secondary(ui, tr!("gui.cancel")).clicked() {
                cancel = true;
            }
        });
    });

    if save {
        let path = ed.path.trim().to_string();
        let expanded = safety::expand_env(&path);
        let builtin = safety::builtin_entries();
        let extra = &app.settings.protection.extra;
        let dup = builtin.iter().any(|b| same(&b.path, &path))
            || extra.iter().enumerate().any(|(i, e)| Some(i) != ed.index && same(&e.path, &path));
        ed.error = if path.is_empty() {
            Some(tr!("prot.err_empty"))
        } else if !std::path::Path::new(&expanded).is_absolute() {
            Some(tr!("prot.err_relative"))
        } else if dup {
            Some(tr!("prot.err_exists"))
        } else {
            None
        };
        if ed.error.is_none() {
            let entry = ProtectedPath { path, inside: ed.inside };
            let list = &mut app.settings.protection.extra;
            match ed.index {
                Some(i) if i < list.len() => list[i] = entry,
                _ => list.push(entry),
            }
            app.prot_edit = None;
        }
    } else if cancel || resp.should_close() {
        app.prot_edit = None;
    }
}
