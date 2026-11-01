//! Look and feel: the dark palette from the approved mockup, a matching
//! light palette, type scale and spacing.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference, Visuals,
};

use freewipe::settings::Theme as SettingsTheme;

/// Semantic colours used by the views.
#[derive(Clone, Copy)]
pub struct Pal {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card_hi: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub warn_bg: Color32,
    pub warn_border: Color32,
    pub warn_text: Color32,
    pub warn_icon: Color32,
    pub ok: Color32,
    pub ok_bg: Color32,
    pub err: Color32,
    pub flash_bg: Color32,
    pub flash_text: Color32,
}

pub const DARK: Pal = Pal {
    bg: Color32::from_rgb(0x16, 0x19, 0x1e),
    panel: Color32::from_rgb(0x12, 0x14, 0x18),
    card: Color32::from_rgb(0x1e, 0x22, 0x28),
    card_hi: Color32::from_rgb(0x23, 0x27, 0x2f),
    border: Color32::from_rgb(0x2c, 0x31, 0x3a),
    text: Color32::from_rgb(0xe8, 0xea, 0xed),
    muted: Color32::from_rgb(0x9a, 0xa3, 0xae),
    accent: Color32::from_rgb(0xff, 0x6b, 0x3d),
    on_accent: Color32::from_rgb(0x1a, 0x0e, 0x08),
    accent_soft: Color32::from_rgb(0x3b, 0x1f, 0x15),
    warn_bg: Color32::from_rgb(0x2a, 0x24, 0x12),
    warn_border: Color32::from_rgb(0x4a, 0x3d, 0x18),
    warn_text: Color32::from_rgb(0xf2, 0xdc, 0xa8),
    warn_icon: Color32::from_rgb(0xf2, 0xb8, 0x4b),
    ok: Color32::from_rgb(0x5c, 0xc8, 0xa0),
    ok_bg: Color32::from_rgb(0x1d, 0x30, 0x29),
    err: Color32::from_rgb(0xff, 0x8a, 0x80),
    flash_bg: Color32::from_rgb(0x3a, 0x2e, 0x14),
    flash_text: Color32::from_rgb(0xf2, 0xc4, 0x6b),
};

pub const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(0xf6, 0xf7, 0xf9),
    panel: Color32::from_rgb(0xec, 0xee, 0xf1),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    card_hi: Color32::from_rgb(0xe3, 0xe6, 0xea),
    border: Color32::from_rgb(0xd4, 0xd8, 0xde),
    text: Color32::from_rgb(0x1a, 0x1d, 0x22),
    muted: Color32::from_rgb(0x5a, 0x63, 0x6e),
    accent: Color32::from_rgb(0xd9, 0x48, 0x1c),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(0xfb, 0xe3, 0xda),
    warn_bg: Color32::from_rgb(0xfd, 0xf3, 0xd8),
    warn_border: Color32::from_rgb(0xe8, 0xcf, 0x8a),
    warn_text: Color32::from_rgb(0x5c, 0x45, 0x0a),
    warn_icon: Color32::from_rgb(0xb7, 0x7b, 0x00),
    ok: Color32::from_rgb(0x15, 0x80, 0x5c),
    ok_bg: Color32::from_rgb(0xdc, 0xf3, 0xe9),
    err: Color32::from_rgb(0xb4, 0x23, 0x18),
    flash_bg: Color32::from_rgb(0xfd, 0xec, 0xc8),
    flash_text: Color32::from_rgb(0x7a, 0x52, 0x00),
};

/// Palette for the theme currently in effect.
pub fn pal(ui: &egui::Ui) -> Pal {
    if ui.visuals().dark_mode {
        DARK
    } else {
        LIGHT
    }
}

fn visuals(p: &Pal, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = if dark { Color32::from_rgb(0x0f, 0x11, 0x14) } else { Color32::WHITE };
    v.faint_bg_color = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent;
    v.override_text_color = Some(p.text);

    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.bg_fill = p.card_hi;
    v.widgets.hovered.weak_bg_fill = p.card_hi;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.muted);
    v.widgets.active.bg_fill = p.card_hi;
    v.widgets.active.weak_bg_fill = p.card_hi;
    v
}

/// Apply fonts sizes, spacing and both palettes; pick the theme.
pub fn install(ctx: &egui::Context, theme: SettingsTheme) {
    // Bundled fonts only: Ubuntu for text, Noto Emoji + emoji-icon-font for
    // icons, Hack as a last resort for arrows and box-drawing symbols.
    let mut fonts = egui::FontDefinitions::default();
    if fonts.font_data.contains_key("Hack") {
        let prop = fonts.families.entry(FontFamily::Proportional).or_default();
        if !prop.iter().any(|f| f == "Hack") {
            prop.push("Hack".into());
        }
    }
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.combo_height = 400.0;
        // Like a native Windows app: labels are not selectable. Text worth
        // copying (log, errors, paths) opts in with `Label::selectable(true)`.
        s.interaction.selectable_labels = false;
    });
    apply_theme(ctx, theme);
}

pub fn apply_theme(ctx: &egui::Context, theme: SettingsTheme) {
    ctx.set_theme(match theme {
        SettingsTheme::System => ThemePreference::System,
        SettingsTheme::Dark => ThemePreference::Dark,
        SettingsTheme::Light => ThemePreference::Light,
    });
}
