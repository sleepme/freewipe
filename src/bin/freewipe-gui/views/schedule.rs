//! Scheduled wipes (Windows Task Scheduler).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, Align, Layout, RichText, Ui};

use freewipe::plan::DirFilter;
use freewipe::schedule::{self, Recurrence, Schedule, ScheduleTarget, TaskState};
use freewipe::{tr, Method};

use super::queue::method_label;
use super::title;
use crate::app::App;
use crate::theme::pal;
use crate::widgets;

/// Task Scheduler states, loaded in the background.
#[derive(Default)]
pub struct SchedState {
    pub states: HashMap<String, TaskState>,
    pub loading: Option<Receiver<HashMap<String, TaskState>>>,
    pub loaded: bool,
    pub msg: Option<(bool, String)>,
}

impl SchedState {
    fn refresh(&mut self, list: &[Schedule], ctx: &egui::Context) {
        let list = list.to_vec();
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let m: HashMap<String, TaskState> = list.iter().map(|s| (s.id.clone(), schedule::state(s))).collect();
            let _ = tx.send(m);
            ctx.request_repaint();
        });
        self.loading = Some(rx);
        self.loaded = true;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum What {
    FreeSpace,
    Folder,
    RecycleBin,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum When {
    Daily,
    Weekly,
    Monthly,
    AtLogon,
}

/// Editor state.
pub struct SchedEdit {
    index: Option<usize>,
    id: String,
    name: String,
    what: What,
    volume: PathBuf,
    tips: bool,
    records: bool,
    folder: String,
    include: String,
    exclude: String,
    contents_only: bool,
    when: When,
    weekday: u8,
    monthday: u8,
    hour: u8,
    minute: u8,
    method: String,
    show_window: bool,
    enabled: bool,
    error: Option<String>,
}

impl SchedEdit {
    fn new(app: &App) -> Self {
        let volume = freewipe::sys::list_volumes().into_iter().next().unwrap_or_else(|| PathBuf::from("C:\\"));
        SchedEdit {
            index: None,
            id: Schedule::new_id(),
            name: tr!("sched.default_name"),
            what: What::FreeSpace,
            volume,
            tips: false,
            records: false,
            folder: String::new(),
            include: String::new(),
            exclude: String::new(),
            contents_only: true,
            when: When::Weekly,
            weekday: 7,
            monthday: 1,
            hour: 3,
            minute: 0,
            method: app.settings.method.clone(),
            show_window: false,
            enabled: true,
            error: None,
        }
    }

    fn from(index: usize, s: &Schedule) -> Self {
        let (hour, minute) = s.hm();
        let mut e = SchedEdit {
            index: Some(index),
            id: s.id.clone(),
            name: s.name.clone(),
            what: What::RecycleBin,
            volume: PathBuf::new(),
            tips: false,
            records: false,
            folder: String::new(),
            include: String::new(),
            exclude: String::new(),
            contents_only: true,
            when: When::Daily,
            weekday: 7,
            monthday: 1,
            hour,
            minute,
            method: s.method.clone(),
            show_window: s.show_window,
            enabled: s.enabled,
            error: None,
        };
        match &s.target {
            ScheduleTarget::FreeSpace { volume, cluster_tips, file_records } => {
                e.what = What::FreeSpace;
                e.volume = volume.clone();
                e.tips = *cluster_tips;
                e.records = *file_records;
            }
            ScheduleTarget::Folder { path, filter } => {
                e.what = What::Folder;
                e.folder = path.display().to_string();
                e.include = filter.include.join("; ");
                e.exclude = filter.exclude.join("; ");
                e.contents_only = filter.contents_only;
            }
            ScheduleTarget::RecycleBin => e.what = What::RecycleBin,
        }
        match s.when {
            Recurrence::Daily => e.when = When::Daily,
            Recurrence::Weekly { day } => {
                e.when = When::Weekly;
                e.weekday = day;
            }
            Recurrence::Monthly { day } => {
                e.when = When::Monthly;
                e.monthday = day;
            }
            Recurrence::AtLogon => e.when = When::AtLogon,
        }
        e
    }

    fn build(&self) -> Result<Schedule, String> {
        let target = match self.what {
            What::FreeSpace => ScheduleTarget::FreeSpace {
                volume: self.volume.clone(),
                cluster_tips: self.tips,
                file_records: self.records,
            },
            What::Folder => {
                let path = PathBuf::from(self.folder.trim());
                if !path.is_dir() {
                    return Err(tr!("err.not_found", path = path.display()));
                }
                if let Err(r) = freewipe::safety::check(&path, false) {
                    return Err(r.message());
                }
                ScheduleTarget::Folder {
                    path,
                    filter: DirFilter {
                        include: DirFilter::parse_masks(&self.include),
                        exclude: DirFilter::parse_masks(&self.exclude),
                        contents_only: self.contents_only,
                    },
                }
            }
            What::RecycleBin => ScheduleTarget::RecycleBin,
        };
        let when = match self.when {
            When::Daily => Recurrence::Daily,
            When::Weekly => Recurrence::Weekly { day: self.weekday },
            When::Monthly => Recurrence::Monthly { day: self.monthday },
            When::AtLogon => Recurrence::AtLogon,
        };
        let name = if self.name.trim().is_empty() { tr!("sched.default_name") } else { self.name.trim().to_string() };
        Ok(Schedule {
            id: self.id.clone(),
            name,
            target,
            when,
            time: format!("{:02}:{:02}", self.hour, self.minute),
            method: self.method.clone(),
            show_window: self.show_window,
            enabled: self.enabled,
        })
    }
}

fn weekday_name(d: u8) -> String {
    match d {
        1 => tr!("sched.day1"),
        2 => tr!("sched.day2"),
        3 => tr!("sched.day3"),
        4 => tr!("sched.day4"),
        5 => tr!("sched.day5"),
        6 => tr!("sched.day6"),
        _ => tr!("sched.day7"),
    }
}

/// "Every Sunday at 03:00".
pub fn when_text(s: &Schedule) -> String {
    let t = s.time.clone();
    match s.when {
        Recurrence::Daily => tr!("sched.every_day", time = t),
        Recurrence::Weekly { day } => tr!("sched.every_week", day = weekday_name(day), time = t),
        Recurrence::Monthly { day } => tr!("sched.every_month", day = day, time = t),
        Recurrence::AtLogon => tr!("sched.at_logon"),
    }
}

pub fn what_text(s: &Schedule) -> String {
    match &s.target {
        ScheduleTarget::FreeSpace { volume, cluster_tips, file_records } => {
            let mut t = tr!("sched.what_free", volume = volume.display());
            if *cluster_tips {
                t.push_str(&format!(" + {}", tr!("gui.c_tips")));
            }
            if *file_records {
                t.push_str(&format!(" + {}", tr!("gui.c_records")));
            }
            t
        }
        ScheduleTarget::Folder { path, filter } => {
            format!("{} · {}", path.display(), filter.summary())
        }
        ScheduleTarget::RecycleBin => tr!("kind.recycle_bin"),
    }
}

fn result_text(st: &TaskState) -> String {
    match st.last_result.trim() {
        "" => String::new(),
        "0" => tr!("sched.last_ok", time = st.last_run),
        "267011" => tr!("sched.never_ran"),
        "267009" => tr!("sched.running"),
        code => tr!("sched.last_code", time = st.last_run, code = code),
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    let mut new = false;
    let mut refresh = false;
    title(ui, &tr!("sched.title"), |ui| {
        if cfg!(windows) && widgets::secondary(ui, format!("+ {}", tr!("sched.new"))).clicked() {
            new = true;
        }
        if widgets::secondary(ui, tr!("gui.refresh")).clicked() {
            refresh = true;
        }
    });
    widgets::hint(ui, &tr!("sched.hint"));
    ui.add_space(10.0);
    if new {
        app.sched_edit = Some(SchedEdit::new(app));
    }
    if refresh || !app.sched.loaded {
        let list = app.settings.schedules.clone();
        app.sched.refresh(&list, ui.ctx());
    }
    if let Some(rx) = &app.sched.loading {
        if let Ok(m) = rx.try_recv() {
            app.sched.states = m;
            app.sched.loading = None;
        }
    }
    if let Some((ok, m)) = &app.sched.msg {
        ui.label(RichText::new(m).color(if *ok { p.ok } else { p.err }));
        ui.add_space(6.0);
    }
    if app.settings.schedules.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(tr!("sched.empty")).color(p.muted).size(15.0));
        });
    }

    let gui = std::env::current_exe().unwrap_or_default();
    let mut action: Option<(usize, &str)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, s) in app.settings.schedules.iter().enumerate() {
            let st = app.sched.states.get(&s.id).cloned().unwrap_or_default();
            let narrow = ui.available_width() < 720.0;
            widgets::card(ui, |ui| {
                let info = |ui: &mut Ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🕒").size(20.0).color(if s.enabled { p.accent } else { p.muted }));
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&s.name).strong().size(15.5));
                                if !s.enabled {
                                    widgets::badge(ui, tr!("sched.off"), p.muted, p.card_hi);
                                }
                            });
                            ui.add(egui::Label::new(RichText::new(what_text(s)).size(12.5)).wrap());
                            let m =
                                Method::from_key(&s.method).map(|m| m.name().to_string()).unwrap_or(s.method.clone());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{} · {m}", when_text(s))).size(12.5).color(p.muted),
                                )
                                .wrap(),
                            );
                            if app.sched.loading.is_none() {
                                let line = if !st.exists && cfg!(windows) {
                                    RichText::new(tr!("sched.missing")).color(p.err)
                                } else {
                                    let mut t = String::new();
                                    if !st.next_run.is_empty() && s.enabled {
                                        t = tr!("sched.next", time = st.next_run);
                                    }
                                    let r = result_text(&st);
                                    if !r.is_empty() {
                                        if !t.is_empty() {
                                            t.push_str("  ·  ");
                                        }
                                        t.push_str(&r);
                                    }
                                    RichText::new(t).color(p.muted)
                                };
                                ui.add(egui::Label::new(line.size(12.0)).wrap());
                            }
                        });
                    });
                };
                let mut buttons = |ui: &mut Ui| {
                    if ui.small_button("✖").on_hover_text(tr!("sched.delete")).clicked() {
                        action = Some((i, "delete"));
                    }
                    if ui.small_button(tr!("custom.edit")).clicked() {
                        action = Some((i, "edit"));
                    }
                    if !st.exists && cfg!(windows) {
                        if ui.small_button(tr!("sched.recreate")).clicked() {
                            action = Some((i, "recreate"));
                        }
                    } else if ui.small_button(tr!("sched.run_now")).clicked() {
                        action = Some((i, "run"));
                    }
                };
                if narrow {
                    info(ui);
                    ui.add_space(4.0);
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(egui::vec2(w, 26.0), Layout::right_to_left(Align::Center), |ui| {
                        buttons(ui)
                    });
                } else {
                    ui.horizontal(|ui| {
                        // Buttons first (right-aligned) so the text gets what is left.
                        let w = ui.available_width();
                        ui.allocate_ui_with_layout(
                            egui::vec2(w - 290.0, 0.0),
                            Layout::left_to_right(Align::Center),
                            |ui| info(ui),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| buttons(ui));
                    });
                }
            });
            ui.add_space(8.0);
        }
    });

    if let Some((i, what)) = action {
        let s = app.settings.schedules[i].clone();
        let r = match what {
            "delete" => {
                let r = schedule::unregister(&s);
                app.settings.schedules.remove(i);
                r.map(|_| tr!("sched.deleted")).or_else(|_| Ok(tr!("sched.deleted")))
            }
            "edit" => {
                app.sched_edit = Some(SchedEdit::from(i, &s));
                Ok(String::new())
            }
            "recreate" => schedule::register(&s, &gui).map(|_| tr!("sched.saved")),
            _ => schedule::run_now(&s).map(|_| tr!("sched.started")),
        };
        app.sched.msg = match r {
            Ok(m) if m.is_empty() => None,
            Ok(m) => Some((true, m)),
            Err(e) => Some((false, tr!("sched.failed", err = e))),
        };
        let list = app.settings.schedules.clone();
        app.sched.refresh(&list, ui.ctx());
    }
    editor(app, ui, &gui);
}

fn editor(app: &mut App, ui: &mut Ui, gui: &std::path::Path) {
    let p = pal(ui);
    let random_passes = app.settings.random_passes;
    let Some(ed) = app.sched_edit.as_mut() else { return };
    let mut save = false;
    let mut cancel = false;
    let resp = egui::Modal::new(egui::Id::unique("sched-editor")).show(ui.ctx(), |ui| {
        ui.set_width(560.0);
        ui.heading(if ed.index.is_some() { tr!("sched.edit_title") } else { tr!("sched.new_title") });
        ui.add_space(6.0);
        egui::Grid::new("sched-grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label(tr!("custom.name"));
            ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(360.0));
            ui.end_row();

            ui.label(tr!("sched.what"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut ed.what, What::FreeSpace, tr!("kind.freespace"));
                    ui.radio_value(&mut ed.what, What::Folder, tr!("sched.folder"));
                    ui.radio_value(&mut ed.what, What::RecycleBin, tr!("kind.recycle_bin"));
                });
                match ed.what {
                    What::FreeSpace => {
                        egui::ComboBox::from_id_salt("sched-vol")
                            .selected_text(ed.volume.display().to_string())
                            .show_ui(ui, |ui| {
                                for v in freewipe::sys::list_volumes() {
                                    let label = v.display().to_string();
                                    ui.selectable_value(&mut ed.volume, v, label);
                                }
                            });
                        ui.checkbox(&mut ed.tips, tr!("gui.fs_tips"));
                        ui.checkbox(&mut ed.records, tr!("gui.fs_records"));
                    }
                    What::Folder => {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut ed.folder).desired_width(280.0));
                            if ui.button(tr!("sched.browse")).clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    ed.folder = d.display().to_string();
                                }
                            }
                        });
                        ui.label(RichText::new(tr!("gui.filter_include")).size(12.5));
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.include).hint_text("*.tmp; *.log").desired_width(360.0),
                        );
                        ui.label(RichText::new(tr!("gui.filter_exclude")).size(12.5));
                        ui.add(egui::TextEdit::singleline(&mut ed.exclude).desired_width(360.0));
                        ui.checkbox(&mut ed.contents_only, tr!("gui.filter_contents"));
                    }
                    What::RecycleBin => {}
                }
            });
            ui.end_row();

            ui.label(tr!("sched.when"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut ed.when, When::Daily, tr!("sched.daily"));
                    ui.radio_value(&mut ed.when, When::Weekly, tr!("sched.weekly"));
                    ui.radio_value(&mut ed.when, When::Monthly, tr!("sched.monthly"));
                    ui.radio_value(&mut ed.when, When::AtLogon, tr!("sched.logon"));
                });
                ui.horizontal(|ui| {
                    match ed.when {
                        When::Weekly => {
                            egui::ComboBox::from_id_salt("sched-day").selected_text(weekday_name(ed.weekday)).show_ui(
                                ui,
                                |ui| {
                                    for d in 1..=7u8 {
                                        ui.selectable_value(&mut ed.weekday, d, weekday_name(d));
                                    }
                                },
                            );
                        }
                        When::Monthly => {
                            ui.label(tr!("sched.day_of_month"));
                            ui.add(egui::DragValue::new(&mut ed.monthday).range(1..=31));
                        }
                        _ => {}
                    }
                    if ed.when != When::AtLogon {
                        ui.label(tr!("sched.at"));
                        ui.add(
                            egui::DragValue::new(&mut ed.hour).range(0..=23).custom_formatter(|v, _| format!("{v:02}")),
                        );
                        ui.label(":");
                        ui.add(
                            egui::DragValue::new(&mut ed.minute)
                                .range(0..=59)
                                .custom_formatter(|v, _| format!("{v:02}")),
                        );
                    }
                });
            });
            ui.end_row();

            ui.label(tr!("gui.method"));
            let current = Method::from_key(&ed.method).unwrap_or(Method::DodE);
            egui::ComboBox::from_id_salt("sched-method")
                .width(340.0)
                .selected_text(method_label(current, random_passes))
                .show_ui(ui, |ui| {
                    for m in Method::all() {
                        ui.selectable_value(&mut ed.method, m.key().to_string(), method_label(m, random_passes));
                    }
                });
            ui.end_row();

            ui.label(String::new());
            ui.vertical(|ui| {
                ui.checkbox(&mut ed.show_window, tr!("sched.show_window"));
                ui.checkbox(&mut ed.enabled, tr!("sched.enabled"));
            });
            ui.end_row();
        });
        widgets::hint(ui, &tr!("sched.editor_hint"));
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
        match ed.build() {
            Err(e) => ed.error = Some(e),
            Ok(s) => match schedule::register(&s, gui) {
                Err(e) => ed.error = Some(tr!("sched.failed", err = e)),
                Ok(()) => {
                    match ed.index {
                        Some(i) if i < app.settings.schedules.len() => app.settings.schedules[i] = s,
                        _ => app.settings.schedules.push(s),
                    }
                    app.sched_edit = None;
                    app.sched.msg = Some((true, tr!("sched.saved")));
                    let list = app.settings.schedules.clone();
                    app.sched.refresh(&list, ui.ctx());
                }
            },
        }
    } else if cancel || resp.should_close() {
        app.sched_edit = None;
    }
}
