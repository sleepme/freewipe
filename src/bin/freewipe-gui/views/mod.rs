//! One module per screen of the approved mockup.

pub mod inuse;
pub mod job;
pub mod legacy;
pub mod lock;
pub mod methods;
pub mod protected;
pub mod queue;
pub mod quick;
pub mod reports;
pub mod schedule;
pub mod settings;
pub mod volumes;

use eframe::egui::{self, Layout, Ui, Vec2};

/// Lay out a fixed-width cell in a horizontal row.
pub fn cell<R>(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(Vec2::new(width, height), Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(Vec2::new(width, height));
        ui.set_max_width(width);
        add(ui)
    })
    .inner
}

/// A page title with optional right-aligned controls.
pub fn title(ui: &mut Ui, text: &str, right: impl FnOnce(&mut Ui)) {
    if ui.available_width() < 780.0 {
        // Narrow: the controls go on their own line instead of overlapping.
        ui.heading(text);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.with_layout(Layout::right_to_left(egui::Align::Center), right);
        });
    } else {
        ui.horizontal(|ui| {
            ui.heading(text);
            ui.with_layout(Layout::right_to_left(egui::Align::Center), right);
        });
    }
    ui.add_space(6.0);
}

/// Two columns side by side, or one below the other when the window is
/// narrow. `add` is called with the column index (0, 1).
pub fn columns2(ui: &mut Ui, mut add: impl FnMut(&mut Ui, usize)) {
    if ui.available_width() < 720.0 {
        add(ui, 0);
        ui.add_space(12.0);
        add(ui, 1);
    } else {
        ui.columns(2, |cols| {
            let (a, b) = cols.split_at_mut(1);
            add(&mut a[0], 0);
            add(&mut b[0], 1);
        });
    }
}

/// Two columns side by side at any width.
pub fn columns2_fixed(ui: &mut Ui, mut add: impl FnMut(&mut Ui, usize)) {
    ui.columns(2, |cols| {
        let (a, b) = cols.split_at_mut(1);
        add(&mut a[0], 0);
        add(&mut b[0], 1);
    });
}
