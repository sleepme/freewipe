//! Catalogue of wiping methods, and the editor for custom methods.

use eframe::egui::{self, RichText, Ui};

use freewipe::methods::{CustomMethod, MAX_CUSTOM_PASSES};
use freewipe::{tr, Method};

use super::title;
use crate::app::App;
use crate::theme::pal;
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PassKind {
    Zero,
    One,
    Byte,
    Pattern,
    Random,
}

impl PassKind {
    const ALL: [PassKind; 5] = [PassKind::Zero, PassKind::One, PassKind::Byte, PassKind::Pattern, PassKind::Random];
    fn label(self) -> String {
        match self {
            PassKind::Zero => tr!("custom.k_zero"),
            PassKind::One => tr!("custom.k_one"),
            PassKind::Byte => tr!("custom.k_byte"),
            PassKind::Pattern => tr!("custom.k_pattern"),
            PassKind::Random => tr!("custom.k_random"),
        }
    }
}

/// State of the custom-method editor.
pub struct MethodEdit {
    /// Index in `settings.custom_methods` being edited (`None` = new).
    index: Option<usize>,
    name: String,
    passes: Vec<(PassKind, String)>,
    verify: bool,
    error: Option<String>,
}

impl MethodEdit {
    fn new() -> Self {
        MethodEdit {
            index: None,
            name: String::new(),
            passes: vec![(PassKind::Zero, String::new()), (PassKind::Random, String::new())],
            verify: true,
            error: None,
        }
    }

    fn from(index: usize, c: &CustomMethod) -> Self {
        let passes = c
            .passes
            .iter()
            .map(|p| {
                let t = p.trim();
                let hex: String = t.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                if t.eq_ignore_ascii_case("random") {
                    (PassKind::Random, String::new())
                } else if hex.eq_ignore_ascii_case("00") {
                    (PassKind::Zero, String::new())
                } else if hex.eq_ignore_ascii_case("ff") {
                    (PassKind::One, String::new())
                } else if hex.len() == 2 {
                    (PassKind::Byte, t.to_string())
                } else {
                    (PassKind::Pattern, t.to_string())
                }
            })
            .collect();
        MethodEdit { index: Some(index), name: c.name.clone(), passes, verify: c.verify, error: None }
    }

    fn to_custom(&self) -> CustomMethod {
        let passes = self
            .passes
            .iter()
            .map(|(k, v)| match k {
                PassKind::Zero => "00".to_string(),
                PassKind::One => "FF".to_string(),
                PassKind::Random => "random".to_string(),
                PassKind::Byte | PassKind::Pattern => v.trim().to_string(),
            })
            .collect();
        CustomMethod { name: self.name.trim().to_string(), passes, verify: self.verify }
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    title(ui, &tr!("gui.methods_title"), |ui| {
        if widgets::secondary(ui, format!("+ {}", tr!("custom.new"))).clicked() {
            app.method_edit = Some(MethodEdit::new());
        }
    });
    widgets::hint(ui, &tr!("methods.note"));
    ui.add_space(10.0);

    let mut edit: Option<usize> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // One row per method: name, key and passes on the first line, the
        // description wrapped below — readable at any window width.
        for (n, m) in Method::all().into_iter().enumerate() {
            let fill = if n % 2 == 0 { p.card } else { p.bg };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(m.name()).strong());
                        ui.label(RichText::new(m.key()).monospace().size(12.0).color(p.muted));
                        let passes = if m == Method::RandomN {
                            format!("N ({})", app.settings.random_passes)
                        } else {
                            m.pass_count(None).to_string()
                        };
                        let v = if m.verify_recommended() { format!("{passes} ✔") } else { passes };
                        widgets::badge(ui, tr!("gui.passes_badge", n = v), p.muted, p.card_hi);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if app.settings.method == m.key() {
                                widgets::badge(ui, tr!("gui.default"), p.ok, p.ok_bg);
                            } else if ui.small_button(tr!("gui.make_default")).clicked() {
                                app.settings.method = m.key().to_string();
                                app.method = m;
                            }
                            if let Method::Custom(i) = m {
                                if ui.small_button(tr!("custom.edit")).clicked() {
                                    edit = Some(i as usize);
                                }
                            }
                        });
                    });
                    ui.add(egui::Label::new(RichText::new(m.localized_description()).color(p.muted).size(12.5)).wrap());
                });
        }
        ui.add_space(8.0);
        widgets::hint(ui, &tr!("gui.methods_verify_note"));
    });
    if let Some(i) = edit {
        if let Some(c) = app.settings.custom_methods.get(i) {
            app.method_edit = Some(MethodEdit::from(i, c));
        }
    }
    editor(app, ui);
}

/// Store the custom methods and re-resolve the selected ones (indexes move).
fn commit(app: &mut App, list: Vec<CustomMethod>) {
    let current = app.method.key().to_string();
    app.settings.custom_methods = list;
    freewipe::methods::set_custom(&app.settings.custom_methods);
    if Method::from_key(&app.settings.method).is_none() {
        app.settings.method = Method::DodE.key().to_string();
    }
    app.method = Method::from_key(&current).unwrap_or_else(|| app.settings.method());
}

fn editor(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let Some(ed) = app.method_edit.as_mut() else { return };
    let mut save = false;
    let mut delete = false;
    let mut cancel = false;
    let resp = egui::Modal::new(egui::Id::unique("method-editor")).show(ui.ctx(), |ui| {
        ui.set_width(520.0);
        ui.heading(if ed.index.is_some() { tr!("custom.edit_title") } else { tr!("custom.new_title") });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(tr!("custom.name"));
            ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(320.0));
        });
        ui.add_space(6.0);
        widgets::section(ui, &tr!("custom.passes", n = ed.passes.len()));
        let mut remove = None;
        let mut up = None;
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for (i, (kind, val)) in ed.passes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:>2}.", i + 1)).monospace().color(p.muted));
                    egui::ComboBox::from_id_salt(("pass-kind", i)).width(150.0).selected_text(kind.label()).show_ui(
                        ui,
                        |ui| {
                            for k in PassKind::ALL {
                                ui.selectable_value(kind, k, k.label());
                            }
                        },
                    );
                    if matches!(kind, PassKind::Byte | PassKind::Pattern) {
                        let hint = if *kind == PassKind::Byte { "AA" } else { "92 49 24" };
                        ui.add(
                            egui::TextEdit::singleline(val)
                                .hint_text(hint)
                                .desired_width(140.0)
                                .font(egui::TextStyle::Monospace),
                        );
                    }
                    if i > 0 && ui.small_button("↑").clicked() {
                        up = Some(i);
                    }
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = up {
            ed.passes.swap(i - 1, i);
        }
        if let Some(i) = remove {
            ed.passes.remove(i);
        }
        ui.horizontal(|ui| {
            if ed.passes.len() < MAX_CUSTOM_PASSES && ui.small_button(format!("+ {}", tr!("custom.add_pass"))).clicked()
            {
                ed.passes.push((PassKind::Random, String::new()));
            }
        });
        ui.add_space(6.0);
        ui.checkbox(&mut ed.verify, tr!("custom.verify"));
        widgets::hint(ui, &tr!("custom.hint"));
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
            if ed.index.is_some() && widgets::danger(ui, tr!("custom.delete")).clicked() {
                delete = true;
            }
        });
    });

    if save {
        let c = ed.to_custom();
        let clash =
            app.settings.custom_methods.iter().enumerate().any(|(i, o)| Some(i) != ed.index && o.key() == c.key())
                || Method::ALL.iter().any(|m| m.name().eq_ignore_ascii_case(&c.name));
        match c.validate() {
            Err(e) => ed.error = Some(e),
            Ok(_) if clash => ed.error = Some(tr!("custom.err_exists")),
            Ok(_) => {
                let mut list = app.settings.custom_methods.clone();
                match ed.index {
                    Some(i) if i < list.len() => {
                        // Keep "default method" pointing at a renamed method.
                        if app.settings.method == list[i].key() {
                            app.settings.method = c.key();
                        }
                        list[i] = c;
                    }
                    _ => list.push(c),
                }
                app.method_edit = None;
                commit(app, list);
            }
        }
    } else if delete {
        let mut list = app.settings.custom_methods.clone();
        if let Some(i) = ed.index.filter(|i| *i < list.len()) {
            list.remove(i);
        }
        app.method_edit = None;
        commit(app, list);
    } else if cancel || resp.should_close() {
        app.method_edit = None;
    }
}
