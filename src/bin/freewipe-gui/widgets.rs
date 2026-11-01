//! Small custom widgets: toggle switch, buttons, badges, cards, banners.

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Response, RichText, Sense, Stroke, Ui, Vec2};

use crate::theme::pal;

/// iOS-style switch (from the egui demo, recoloured).
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let p = pal(ui);
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let fill = if *on { p.accent } else { p.card_hi };
        let r = rect.height() / 2.0;
        ui.painter().rect(
            rect,
            r,
            fill,
            Stroke::new(1.0, if *on { p.accent } else { p.border }),
            egui::StrokeKind::Inside,
        );
        let cx = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        let knob = if *on { p.on_accent } else { p.muted };
        ui.painter().circle_filled(egui::pos2(cx, rect.center().y), r - 4.0, knob);
    }
    resp
}

/// Blend two colours (`t` = 0 → `a`, 1 → `b`).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Colours of a button in its normal, hovered and pressed states.
struct Look {
    fill: Color32,
    stroke: Color32,
    hover_fill: Color32,
    hover_stroke: Color32,
    press_fill: Color32,
}

/// A button whose colours change on hover and press. Setting `fill`/`stroke`
/// on an `egui::Button` freezes them for every state, so the colours go into
/// the widget visuals of a local scope instead.
fn styled(ui: &mut Ui, text: RichText, look: Look, min_h: f32) -> Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for (v, fill, stroke) in [
            (&mut w.inactive, look.fill, look.stroke),
            (&mut w.hovered, look.hover_fill, look.hover_stroke),
            (&mut w.active, look.press_fill, look.hover_stroke),
        ] {
            v.bg_fill = fill;
            v.weak_bg_fill = fill;
            v.bg_stroke = Stroke::new(1.0, stroke);
            v.corner_radius = CornerRadius::same(8);
            v.expansion = 0.0;
        }
        ui.add(egui::Button::new(text).min_size(Vec2::new(0.0, min_h))).on_hover_cursor(egui::CursorIcon::PointingHand)
    })
    .inner
}

/// Look of the filled accent button.
fn accent_look(ui: &Ui) -> Look {
    let p = pal(ui);
    Look {
        fill: p.accent,
        stroke: p.accent,
        hover_fill: mix(p.accent, Color32::WHITE, 0.14),
        hover_stroke: mix(p.accent, Color32::WHITE, 0.14),
        press_fill: mix(p.accent, Color32::BLACK, 0.12),
    }
}

/// Big filled accent button.
pub fn primary(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    let look = accent_look(ui);
    styled(ui, RichText::new(text.into()).color(p.on_accent).strong(), look, 42.0)
}

/// Filled accent button of normal height (dialogs).
pub fn accent(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    let look = accent_look(ui);
    styled(ui, RichText::new(text.into()).color(p.on_accent), look, 36.0)
}

/// Outlined secondary button.
pub fn secondary(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    let look = Look {
        fill: p.card,
        stroke: p.border,
        hover_fill: p.card_hi,
        hover_stroke: mix(p.border, p.accent, 0.6),
        press_fill: mix(p.card_hi, p.border, 0.5),
    };
    styled(ui, RichText::new(text.into()).color(p.text), look, 36.0)
}

/// Danger-tinted secondary button (Stop).
pub fn danger(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    let look = Look {
        fill: p.accent_soft,
        stroke: p.accent,
        hover_fill: mix(p.accent_soft, p.accent, 0.22),
        hover_stroke: p.accent,
        press_fill: mix(p.accent_soft, p.accent, 0.4),
    };
    styled(ui, RichText::new(text.into()).color(p.err), look, 40.0)
}

/// Rounded pill with text (painted, so it never stretches with its cell).
pub fn badge(ui: &mut Ui, text: impl Into<String>, fg: Color32, bg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.into(), egui::FontId::proportional(12.0), fg);
    let size = Vec2::new(galley.size().x + 16.0, galley.size().y + 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, bg);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    resp
}

/// Bordered card.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Yellow warning banner.
pub fn warning(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    Frame::new()
        .fill(p.warn_bg)
        .stroke(Stroke::new(1.0, p.warn_border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(RichText::new("⚠").color(p.warn_icon).size(16.0));
                ui.add(egui::Label::new(RichText::new(text).color(p.warn_text).size(13.0)).wrap());
            });
        });
}

/// Small uppercase section heading.
pub fn section(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.label(RichText::new(text.to_uppercase()).color(p.muted).size(12.0).strong());
}

/// First letter upper-case (for messages shared with the CLI).
pub fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Muted helper text.
pub fn hint(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.add(egui::Label::new(RichText::new(text).color(p.muted).size(12.5)).wrap());
}

/// A checkbox row with a muted second line.
pub fn check_row(ui: &mut Ui, value: &mut bool, label: &str, sub: &str) -> Response {
    let r = ui.checkbox(value, label);
    if !sub.is_empty() {
        let pad = ui.spacing().icon_width + ui.spacing().icon_spacing;
        ui.horizontal(|ui| {
            ui.add_space(pad);
            hint(ui, sub);
        });
    }
    r
}

/// Left-aligned navigation entry with an optional counter badge.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, count: Option<usize>) -> Response {
    let p = pal(ui);
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 8.0, p.card_hi);
            let bar = egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
            painter.rect_filled(bar, 2.0, p.accent);
        } else if resp.hovered() {
            painter.rect_filled(rect, 8.0, p.card);
        }
        let color = if selected { p.text } else { p.muted };
        let font = egui::FontId::proportional(14.5);
        painter.text(rect.left_center() + Vec2::new(14.0, 0.0), egui::Align2::LEFT_CENTER, icon, font.clone(), color);
        painter.text(
            rect.left_center() + Vec2::new(40.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            if selected { p.text } else { p.text.gamma_multiply(0.85) },
        );
        if let Some(n) = count {
            let txt = n.to_string();
            let galley = painter.layout_no_wrap(txt, egui::FontId::proportional(12.0), p.on_accent);
            let size = Vec2::new(galley.size().x + 14.0, 20.0);
            let r = egui::Rect::from_center_size(rect.right_center() - Vec2::new(size.x / 2.0 + 10.0, 0.0), size);
            painter.rect_filled(r, 10.0, p.accent);
            painter.galley(r.center() - galley.size() / 2.0, galley, p.on_accent);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
