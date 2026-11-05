//! Settings: General, Wiping, Integration (Explorer context menu), Safety,
//! Reports, About.

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use freewipe::settings::{ConfirmMode, LaunchMode, MenuAction, ShellConfig, Theme};
use freewipe::{i18n, shell, tr, Method};

use super::queue::method_label;
use crate::app::{App, SettingsTab};
use crate::theme::pal;
use crate::widgets;

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    if !super::lock::gate(app, ui, super::lock::Gated::Settings) {
        return;
    }
    // Narrow windows get a narrower tab list.
    let tabs_w = if ui.available_width() < 760.0 { 158.0 } else { 200.0 };
    ui.horizontal_top(|ui| {
        // Tab list.
        ui.allocate_ui_with_layout(egui::vec2(tabs_w, ui.available_height()), Layout::top_down(Align::Min), |ui| {
            ui.set_width(tabs_w);
            ui.heading(tr!("gui.nav_settings"));
            ui.add_space(10.0);
            let tabs = [
                (SettingsTab::General, "🏠", tr!("gui.tab_general")),
                (SettingsTab::Wiping, "🗑", tr!("gui.tab_wiping")),
                (SettingsTab::Integration, "📋", tr!("gui.tab_integration")),
                (SettingsTab::Safety, "🛡", tr!("gui.tab_safety")),
                (SettingsTab::Reports, "🗐", tr!("gui.tab_reports")),
                (SettingsTab::About, "ℹ", tr!("gui.tab_about")),
            ];
            for (t, icon, label) in tabs {
                if widgets::nav_item(ui, icon, &label, app.tab == t, None).clicked() {
                    app.tab = t;
                    if t == SettingsTab::Integration {
                        app.shell_status = None;
                    }
                }
            }
        });
        ui.add_space(14.0);
        ui.painter().vline(ui.cursor().left() - 9.0, ui.max_rect().y_range(), Stroke::new(1.0, p.border));

        let size = ui.available_size();
        ui.allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(860.0));
                match app.tab {
                    SettingsTab::General => general(app, ui),
                    SettingsTab::Wiping => wiping(app, ui),
                    SettingsTab::Integration => integration(app, ui),
                    SettingsTab::Safety => safety(app, ui),
                    SettingsTab::Reports => reports(app, ui),
                    SettingsTab::About => about(ui),
                }
                ui.add_space(16.0);
            });
        });
    });
}

/// A control width that leaves room for the label column of a settings grid.
fn fit(ui: &Ui, preferred: f32) -> f32 {
    preferred.min((ui.available_width() - 40.0).max(140.0))
}

fn header(ui: &mut Ui, title: &str, sub: &str) {
    ui.heading(title);
    if !sub.is_empty() {
        widgets::hint(ui, sub);
    }
    ui.add_space(12.0);
}

// --- General ---------------------------------------------------------------------

fn general(app: &mut App, ui: &mut Ui) {
    header(ui, &tr!("gui.tab_general"), "");
    widgets::card(ui, |ui| {
        egui::Grid::new("general").num_columns(2).spacing([24.0, 12.0]).show(ui, |ui| {
            ui.label(tr!("gui.language"));
            let langs = i18n::languages();
            let label = |l: &i18n::LangInfo| {
                if l.complete < 0.995 {
                    format!("{}  ({}, {:.0}%)", l.name, l.code, l.complete * 100.0)
                } else {
                    format!("{}  ({})", l.name, l.code)
                }
            };
            let cur = langs
                .iter()
                .find(|l| l.code == app.settings.language)
                .map(label)
                .unwrap_or_else(|| app.settings.language.clone());
            egui::ComboBox::from_id_salt("lang").width(fit(ui, 240.0)).selected_text(cur).show_ui(ui, |ui| {
                for l in &langs {
                    ui.selectable_value(&mut app.settings.language, l.code.clone(), label(l));
                }
            });
            ui.end_row();

            ui.label(tr!("gui.theme"));
            egui::ComboBox::from_id_salt("theme")
                .width(fit(ui, 240.0))
                .selected_text(theme_label(app.settings.theme))
                .show_ui(ui, |ui| {
                    for t in [Theme::System, Theme::Dark, Theme::Light] {
                        ui.selectable_value(&mut app.settings.theme, t, theme_label(t));
                    }
                });
            ui.end_row();
        });
    });
    ui.add_space(10.0);
    widgets::card(ui, |ui| {
        ui.checkbox(&mut app.settings.notify_done, tr!("gui.notify_done"));
        ui.checkbox(&mut app.settings.keep_awake, tr!("gui.keep_awake"));
        if cfg!(windows) {
            widgets::check_row(ui, &mut app.settings.tray_icon, &tr!("gui.tray_icon"), &tr!("gui.tray_icon_sub"));
            ui.add_enabled_ui(app.settings.tray_icon, |ui| {
                ui.checkbox(&mut app.settings.minimize_to_tray, tr!("gui.tray_minimize"));
            });
        }
    });
    ui.add_space(10.0);
    diagnostics(app, ui);
    ui.add_space(8.0);
    widgets::hint(ui, &tr!("gui.lang_hint"));
}

/// Settings → General → Diagnostics: the debug log.
fn diagnostics(app: &mut App, ui: &mut Ui) {
    widgets::card(ui, |ui| {
        widgets::section(ui, &tr!("diag.title"));
        widgets::check_row(ui, &mut app.settings.debug_log, &tr!("diag.log"), &tr!("diag.log_sub"));
        let dir = freewipe::debuglog::dir();
        let size = freewipe::debuglog::size();
        widgets::hint(ui, &tr!("diag.size", size = freewipe::util::human_bytes(size), dir = dir.display()));
        ui.horizontal_wrapped(|ui| {
            if widgets::secondary(ui, tr!("diag.open")).clicked() {
                let _ = std::fs::create_dir_all(&dir);
                crate::app::open_path(&dir);
            }
            if size > 0 && widgets::secondary(ui, tr!("diag.delete")).clicked() {
                let n = freewipe::debuglog::delete_all();
                app.toast(tr!("diag.deleted", n = n));
            }
        });
    });
}

fn theme_label(t: Theme) -> String {
    match t {
        Theme::System => tr!("gui.theme_system"),
        Theme::Dark => tr!("gui.theme_dark"),
        Theme::Light => tr!("gui.theme_light"),
    }
}

// --- Wiping -----------------------------------------------------------------------

fn wiping(app: &mut App, ui: &mut Ui) {
    header(ui, &tr!("gui.tab_wiping"), &tr!("gui.wiping_sub"));
    widgets::card(ui, |ui| {
        egui::Grid::new("wiping").num_columns(2).spacing([24.0, 12.0]).show(ui, |ui| {
            ui.label(tr!("gui.default_method"));
            let before = app.settings.method.clone();
            egui::ComboBox::from_id_salt("defmethod")
                .width(fit(ui, 340.0))
                .truncate()
                .selected_text(method_label(app.settings.method(), app.settings.random_passes))
                .show_ui(ui, |ui| {
                    for m in Method::all() {
                        ui.selectable_value(
                            &mut app.settings.method,
                            m.key().to_string(),
                            method_label(m, app.settings.random_passes),
                        );
                    }
                });
            if before != app.settings.method {
                app.method = app.settings.method();
            }
            ui.end_row();

            ui.label(tr!("gui.random_passes"));
            ui.add(egui::DragValue::new(&mut app.settings.random_passes).range(1..=100));
            ui.end_row();

            ui.label(tr!("gui.rename_passes"));
            ui.add(egui::DragValue::new(&mut app.settings.rename_passes).range(0..=20));
            ui.end_row();
        });
    });
    ui.add_space(10.0);
    widgets::card(ui, |ui| {
        if widgets::check_row(ui, &mut app.settings.verify, &tr!("gui.verify_default"), &tr!("gui.verify_default_sub"))
            .changed()
        {
            app.verify = app.settings.verify;
        }
        widgets::check_row(ui, &mut app.settings.wipe_ads, &tr!("gui.wipe_ads"), &tr!("gui.wipe_ads_sub"));
        widgets::check_row(ui, &mut app.settings.ssd_single_pass, &tr!("gui.ssd_single"), &tr!("gui.ssd_single_sub"));
        if widgets::check_row(ui, &mut app.settings.keep, &tr!("gui.keep_default"), "").changed() {
            app.keep = app.settings.keep;
        }
    });
}

// --- Integration ----------------------------------------------------------------------

fn integration(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    header(ui, &tr!("gui.int_title"), &tr!("gui.int_sub"));
    app.foreign_dismissed = false;
    super::legacy::banner(app, ui);

    if !cfg!(windows) {
        widgets::warning(ui, &widgets::cap(&tr!("shell.windows_only")));
        ui.add_space(10.0);
    }
    if app.shell_status.is_none() {
        app.shell_status = Some(shell::status(app.settings.shell.all_users));
    }
    let installed = app.shell_status.as_ref().map(|s| s.installed()).unwrap_or(false);

    // Master switch.
    let mut apply_now = false;
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr!("gui.int_master")).strong());
                    let hint = if installed { tr!("gui.int_on_hint") } else { tr!("gui.int_off_hint") };
                    widgets::hint(ui, &hint);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(cfg!(windows), |ui| {
                        if widgets::toggle(ui, &mut app.settings.shell.enabled).changed() {
                            apply_now = true;
                        }
                    });
                });
            });
        });
    ui.add_space(12.0);

    let cfg = &mut app.settings.shell;
    ui.add_enabled_ui(cfg.enabled, |ui| {
        super::columns2(ui, |ui, col| {
            if col == 0 {
                widgets::card(ui, |ui| {
                    widgets::section(ui, &tr!("gui.int_where"));
                    widgets::check_row(ui, &mut cfg.files, &tr!("gui.int_files"), &tr!("gui.int_files_sub"));
                    widgets::check_row(ui, &mut cfg.dirs, &tr!("gui.int_dirs"), &tr!("gui.int_dirs_sub"));
                    widgets::check_row(ui, &mut cfg.drives, &tr!("gui.int_drives"), &tr!("gui.int_drives_sub"));
                    widgets::check_row(ui, &mut cfg.recycle_bin, &tr!("gui.int_bin"), &tr!("gui.int_bin_sub"));
                });
            } else {
                widgets::card(ui, |ui| {
                    widgets::section(ui, &tr!("gui.int_items"));
                    widgets::check_row(ui, &mut cfg.item_wipe, &tr!("shell.item_wipe"), &tr!("gui.int_item_wipe_sub"));
                    widgets::check_row(
                        ui,
                        &mut cfg.item_method,
                        &format!("{} ▶", tr!("shell.item_method")),
                        &tr!("gui.int_item_method_sub"),
                    );
                    widgets::check_row(
                        ui,
                        &mut cfg.item_queue,
                        &tr!("shell.item_queue"),
                        &tr!("gui.int_item_queue_sub"),
                    );
                    widgets::check_row(ui, &mut cfg.item_move, &tr!("shell.item_move"), &tr!("gui.int_item_move_sub"));
                    widgets::hint(ui, &tr!("gui.int_drive_item_note"));
                });
            }
        });
        ui.add_space(12.0);
        super::columns2(ui, |ui, col| {
            if col == 0 {
                widgets::card(ui, |ui| {
                    widgets::section(ui, &tr!("gui.int_style"));
                    ui.radio_value(&mut cfg.submenu, true, tr!("gui.int_submenu"));
                    ui.radio_value(&mut cfg.submenu, false, tr!("gui.int_flat"));
                    ui.checkbox(&mut cfg.show_icon, tr!("gui.int_icon"));
                    ui.add_space(6.0);
                    widgets::section(ui, &tr!("gui.int_launch"));
                    ui.radio_value(&mut cfg.launch, LaunchMode::ExplorerCommand, tr!("gui.int_launch_ext"));
                    ui.radio_value(&mut cfg.launch, LaunchMode::Command, tr!("gui.int_launch_cmd"));
                    ui.add_enabled_ui(false, |ui| {
                        let mut no = false;
                        ui.checkbox(&mut no, tr!("gui.int_win11"));
                    });
                });
            } else {
                widgets::card(ui, |ui| {
                    widgets::section(ui, &tr!("gui.int_safety"));
                    ui.checkbox(&mut cfg.always_confirm, tr!("gui.int_confirm"));
                    ui.checkbox(&mut cfg.shift_only, tr!("gui.int_shift"));
                    ui.add_space(6.0);
                    widgets::section(ui, &tr!("gui.int_action"));
                    ui.radio_value(&mut cfg.menu_action, MenuAction::WipeNow, tr!("gui.int_wipe_now"));
                    ui.radio_value(&mut cfg.menu_action, MenuAction::Enqueue, tr!("gui.int_enqueue"));
                });
            }
        });
        ui.add_space(12.0);

        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::section(ui, &tr!("gui.int_scope"));
                ui.radio_value(&mut cfg.all_users, false, tr!("gui.int_scope_user"));
                ui.radio_value(&mut cfg.all_users, true, tr!("gui.int_scope_all"));
            });
            let gui = std::env::current_exe().unwrap_or_default();
            let entries = shell::plan_entries(cfg, &gui, shell::DropAction::dll_next_to(&gui).as_deref());
            let hive = if cfg.all_users { "HKLM" } else { "HKCU" };
            let preview: String = entries
                .iter()
                .filter(|e| !e.key.contains(r"\02method\shell\"))
                .map(|e| {
                    let cmd = e
                        .values
                        .iter()
                        .find(|(n, _)| n.is_none())
                        .map(|(_, v)| format!("\n    → {v}"))
                        .unwrap_or_default();
                    format!(r"{hive}\Software\Classes\{}{cmd}", e.key)
                })
                .collect::<Vec<_>>()
                .join("\n");
            Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::both().id_salt("regprev").max_height(150.0).show(ui, |ui| {
                        ui.label(
                            RichText::new(if preview.is_empty() { tr!("gui.int_nothing") } else { preview })
                                .monospace()
                                .size(11.5)
                                .color(p.muted),
                        );
                    });
                });
        });
    });

    ui.add_space(8.0);
    ui.add_enabled_ui(app.settings.shell.enabled && app.settings.shell.menu_action == MenuAction::WipeNow, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr!("gui.finish_close"));
            let label = |s: u32| if s == 0 { tr!("gui.finish_never") } else { tr!("gui.finish_after", s = s) };
            egui::ComboBox::from_id_salt("finish-close").selected_text(label(app.settings.finish_close_secs)).show_ui(
                ui,
                |ui| {
                    for s in [0u32, 3, 5, 10, 30] {
                        ui.selectable_value(&mut app.settings.finish_close_secs, s, label(s));
                    }
                },
            );
        });
        ui.checkbox(&mut app.settings.mini_on_top, tr!("gui.mini_on_top"));
    });

    // Status + buttons.
    ui.add_space(12.0);
    let pending = app.settings.shell.enabled && app.shell_applied.as_ref() != Some(&app.settings.shell);
    ui.horizontal(|ui| {
        let (dot, text) = if installed {
            (p.ok, tr!("gui.int_status_on", n = app.shell_status.as_ref().map(|s| s.verbs.len()).unwrap_or(0)))
        } else {
            (p.muted, tr!("gui.int_status_off"))
        };
        ui.label(RichText::new("●").color(dot));
        ui.label(RichText::new(text).color(dot));
        if pending {
            ui.label(RichText::new(format!("· {}", tr!("gui.int_pending"))).color(p.warn_icon));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(cfg!(windows) && app.settings.shell.enabled, |ui| {
                if widgets::primary(ui, tr!("gui.apply")).clicked() {
                    apply_now = true;
                }
            });
            ui.add_enabled_ui(cfg!(windows) && installed, |ui| {
                if widgets::secondary(ui, tr!("gui.int_remove")).clicked() {
                    app.settings.shell.enabled = false;
                    apply_now = true;
                }
            });
            if widgets::secondary(ui, tr!("gui.int_export")).clicked() {
                export_reg(&app.settings.shell);
            }
        });
    });
    if let Some((ok, msg)) = &app.shell_msg {
        ui.add_space(6.0);
        ui.label(RichText::new(msg).color(if *ok { p.ok } else { p.err }));
    }
    ui.add_space(10.0);
    widgets::hint(ui, &tr!("gui.int_win11_note"));

    if apply_now {
        apply(app);
    }
}

fn export_reg(cfg: &ShellConfig) {
    let gui = std::env::current_exe().unwrap_or_default();
    let reg = shell::to_reg_file(
        &shell::plan_entries(cfg, &gui, shell::DropAction::dll_next_to(&gui).as_deref()),
        cfg.all_users,
    );
    if let Some(path) =
        rfd::FileDialog::new().add_filter("Registry", &["reg"]).set_file_name("freewipe-context-menu.reg").save_file()
    {
        let mut bytes = vec![0xFF, 0xFE];
        for u in reg.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let _ = std::fs::write(path, bytes);
    }
}

/// Install / remove according to the settings, handling scope changes and
/// elevation for "all users".
fn apply(app: &mut App) {
    let cfg = app.settings.shell.clone();
    let previous = app.shell_applied.clone();
    let res = crate::shellops::apply_impl(&cfg, previous.as_ref());
    match res {
        Ok(msg) => {
            app.shell_applied = cfg.enabled.then(|| cfg.clone());
            app.shell_msg = Some((true, msg));
        }
        Err(e) => {
            app.shell_msg = Some((false, e));
            if cfg.enabled && !app.shell_status.as_ref().map(|s| s.installed()).unwrap_or(false) {
                app.settings.shell.enabled = false;
            }
        }
    }
    app.shell_status = None;
    app.foreign = crate::shellops::foreign_entries();
}

// --- Safety -------------------------------------------------------------------------

fn safety(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    header(ui, &tr!("gui.tab_safety"), &tr!("gui.safety_sub"));
    widgets::card(ui, |ui| super::protected::show(app, ui));
    ui.add_space(12.0);
    // Confirmation style.
    widgets::card(ui, |ui| {
        widgets::section(ui, &tr!("gui.confirm_style"));
        ui.radio_value(&mut app.settings.confirm_mode, ConfirmMode::Click, tr!("gui.confirm_click"));
        ui.radio_value(&mut app.settings.confirm_mode, ConfirmMode::Typed, tr!("gui.confirm_typed"));
        ui.radio_value(&mut app.settings.confirm_mode, ConfirmMode::None, tr!("gui.confirm_none"));
        if app.settings.confirm_mode == ConfirmMode::None {
            widgets::warning(ui, &tr!("gui.confirm_none_warn"));
        }
    });
    ui.add_space(12.0);

    // Administrator rights.
    widgets::card(ui, |ui| {
        widgets::section(ui, &tr!("gui.admin_title"));
        ui.add_enabled_ui(cfg!(windows), |ui| {
            widgets::check_row(
                ui,
                &mut app.settings.always_admin,
                &tr!("gui.always_admin"),
                &tr!("gui.always_admin_sub"),
            );
        });
        #[cfg(windows)]
        let elevated = freewipe::sys::is_elevated();
        #[cfg(not(windows))]
        let elevated = false;
        let state = if elevated { tr!("gui.admin_yes") } else { tr!("gui.admin_no") };
        widgets::hint(ui, &format!("{} {state}", tr!("gui.admin_now")));
        widgets::hint(ui, &tr!("gui.admin_retry_note"));
    });
    ui.add_space(12.0);

    password(app, ui);
    ui.add_space(12.0);

    widgets::card(ui, |ui| {
        ui.checkbox(&mut app.settings.ssd_warning, tr!("gui.ssd_warning"));
        let force_color = if app.settings.force { p.err } else { p.text };
        ui.checkbox(&mut app.settings.force, RichText::new(tr!("gui.force")).color(force_color));
        if app.settings.force {
            widgets::warning(ui, &tr!("gui.force_warn"));
        }
    });
}

// --- Reports --------------------------------------------------------------------------

fn reports(app: &mut App, ui: &mut Ui) {
    header(ui, &tr!("gui.tab_reports"), &tr!("gui.reports_sub"));
    widgets::card(ui, |ui| {
        ui.checkbox(&mut app.settings.save_reports, tr!("gui.save_reports"));
        ui.add_space(6.0);
        ui.label(tr!("gui.report_dir"));
        ui.horizontal(|ui| {
            let mut s = app.settings.report_dir().display().to_string();
            let r = ui.add(
                egui::TextEdit::singleline(&mut s)
                    .desired_width(ui.available_width() - 220.0)
                    .font(egui::TextStyle::Monospace),
            );
            if r.changed() {
                app.settings.report_dir = Some(s.into());
            }
            if widgets::secondary(ui, tr!("gui.browse")).clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    app.settings.report_dir = Some(d);
                }
            }
            if widgets::secondary(ui, tr!("gui.reset")).clicked() {
                app.settings.report_dir = None;
            }
        });
        ui.add_space(6.0);
        widgets::check_row(ui, &mut app.settings.redact_reports, &tr!("gui.redact"), &tr!("gui.redact_sub"));
    });
    app.reports = None;
}

// --- About ------------------------------------------------------------------------------

fn about(ui: &mut Ui) {
    let p = pal(ui);
    header(ui, &tr!("gui.tab_about"), "");
    widgets::card(ui, |ui| {
        ui.label(RichText::new(format!("freewipe {}", freewipe::VERSION)).size(18.0).strong());
        ui.label(RichText::new(tr!("gui.about_line")).color(p.muted));
        ui.add_space(8.0);
        let link_row = |ui: &mut Ui, label: String, text: &str, url: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).color(p.muted));
                ui.hyperlink_to(text, url);
            });
        };
        let strip = |u: &str| u.trim_start_matches("https://").to_string();
        link_row(
            ui,
            tr!("gui.about_author"),
            &format!("sleepme · {}", strip(freewipe::AUTHOR_URL)),
            freewipe::AUTHOR_URL,
        );
        link_row(ui, tr!("gui.about_project"), &strip(freewipe::REPOSITORY), freewipe::REPOSITORY);
        link_row(ui, tr!("gui.about_donate"), &format!("☕ {}", strip(freewipe::DONATE_URL)), freewipe::DONATE_URL);
        ui.label(RichText::new(tr!("gui.about_donate_sub")).color(p.muted).small());
        ui.add_space(8.0);
        ui.add(egui::Label::new(tr!("gui.about_limits")).wrap());
    });
}

// --- Password ----------------------------------------------------------------------------

fn password(app: &mut App, ui: &mut Ui) {
    let p = pal(ui);
    widgets::card(ui, |ui| {
        widgets::section(ui, &tr!("gui.pw_title"));
        let has = app.settings.lock.has_password();
        let field = |ui: &mut Ui, label: String, v: &mut String| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(200.0, 24.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(200.0);
                    ui.label(label);
                });
                ui.add(egui::TextEdit::singleline(v).password(true).desired_width(fit(ui, 240.0)));
            });
        };
        if has {
            widgets::hint(ui, &tr!("gui.pw_when"));
            ui.checkbox(&mut app.settings.lock.protect_app, tr!("gui.pw_protect_app"));
            ui.checkbox(&mut app.settings.lock.protect_settings, tr!("gui.pw_protect_settings"));
            ui.checkbox(&mut app.settings.lock.protect_wipe, tr!("gui.pw_protect_wipe"));
            ui.checkbox(&mut app.settings.lock.protect_schedule, tr!("gui.pw_protect_schedule"));
            ui.checkbox(&mut app.settings.lock.protect_methods, tr!("gui.pw_protect_methods"));
            ui.checkbox(&mut app.settings.lock.protect_reports, tr!("gui.pw_protect_reports"));
            if cfg!(windows) {
                ui.add_space(4.0);
                widgets::check_row(
                    ui,
                    &mut app.settings.lock.machine,
                    &tr!("gui.pw_machine"),
                    &tr!("gui.pw_machine_sub"),
                );
            }
            ui.add_space(8.0);
            field(ui, tr!("gui.pw_current"), &mut app.pw.current);
            field(ui, tr!("gui.pw_new"), &mut app.pw.new1);
            field(ui, tr!("gui.pw_repeat"), &mut app.pw.new2);
            ui.horizontal(|ui| {
                if widgets::secondary(ui, tr!("gui.pw_change")).clicked() {
                    app.pw.msg = Some(if !app.settings.lock.verify(&app.pw.current) {
                        (false, tr!("lock.wrong"))
                    } else if let Err(e) = check_new(&app.pw.new1, &app.pw.new2) {
                        (false, e)
                    } else {
                        let old = app.settings.lock.clone();
                        app.settings.lock.set_password(&app.pw.new1);
                        app.settings.lock.protect_app = old.protect_app;
                        app.settings.lock.protect_settings = old.protect_settings;
                        app.settings.lock.protect_wipe = old.protect_wipe;
                        app.settings.lock.protect_schedule = old.protect_schedule;
                        app.settings.lock.protect_methods = old.protect_methods;
                        app.settings.lock.protect_reports = old.protect_reports;
                        app.settings.lock.machine = old.machine;
                        (true, tr!("gui.pw_changed"))
                    });
                    clear(app);
                }
                if widgets::secondary(ui, tr!("gui.pw_remove")).clicked() {
                    app.pw.msg = Some(if app.settings.lock.verify(&app.pw.current) {
                        app.settings.lock.clear();
                        (true, tr!("gui.pw_removed"))
                    } else {
                        (false, tr!("lock.wrong"))
                    });
                    clear(app);
                }
            });
        } else {
            widgets::hint(ui, &tr!("gui.pw_none"));
            ui.add_space(6.0);
            field(ui, tr!("gui.pw_new"), &mut app.pw.new1);
            field(ui, tr!("gui.pw_repeat"), &mut app.pw.new2);
            if widgets::secondary(ui, tr!("gui.pw_set")).clicked() {
                app.pw.msg = Some(match check_new(&app.pw.new1, &app.pw.new2) {
                    Err(e) => (false, e),
                    Ok(()) => {
                        app.settings.lock.set_password(&app.pw.new1);
                        app.settings.lock.protect_app = false;
                        app.settings.lock.protect_settings = true;
                        app.settings.lock.protect_wipe = true;
                        app.settings.lock.protect_schedule = true;
                        app.settings.lock.protect_methods = true;
                        app.settings.lock.protect_reports = true;
                        app.session_unlocked = true;
                        (true, tr!("gui.pw_set_ok"))
                    }
                });
                clear(app);
            }
        }
        if let Some((ok, m)) = &app.pw.msg {
            ui.label(RichText::new(m).color(if *ok { p.ok } else { p.err }));
        }
        ui.add_space(4.0);
        widgets::hint(ui, &tr!("gui.pw_scope"));
    });
}

fn check_new(a: &str, b: &str) -> Result<(), String> {
    if a.chars().count() < 4 {
        Err(tr!("gui.pw_short"))
    } else if a != b {
        Err(tr!("gui.pw_mismatch"))
    } else {
        Ok(())
    }
}

fn clear(app: &mut App) {
    app.pw.current.clear();
    app.pw.new1.clear();
    app.pw.new2.clear();
}
