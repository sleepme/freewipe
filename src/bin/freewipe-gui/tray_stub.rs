//! No notification-area icon on this platform.

use eframe::egui;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    NotificationClicked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

pub struct Tray;

impl Tray {
    pub fn new(_ctx: &egui::Context, _tip: &str) -> Option<Tray> {
        None
    }
    pub fn set_tip(&self, _tip: &str) {}
    pub fn notify(&self, _title: &str, _text: &str, _level: Level) {}
    pub fn take_events(&self) -> Vec<TrayEvent> {
        Vec::new()
    }
}
