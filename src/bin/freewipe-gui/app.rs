//! Application state, navigation and the glue between views, the worker
//! thread and context-menu requests.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use freewipe::plan::{PlanItem, Target, TargetKind};
use freewipe::report::{Report, ReportItem, Status};
use freewipe::settings::{ConfirmMode, MenuAction, Settings, ShellConfig};
use freewipe::shell::ShellStatus;
use freewipe::wipe::WipeOptions;
use freewipe::{i18n, tr, Method};

use crate::inbox::{self, Action, Request};
use crate::theme::{self, pal};
use crate::tray::{Level, Tray, TrayEvent};
use crate::views;
use crate::widgets;
use crate::worker::{self, Job, Msg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Job,
    Queue,
    FreeSpace,
    Methods,
    Schedule,
    Reports,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    General,
    Wiping,
    Integration,
    Safety,
    Reports,
    About,
}

pub struct QueueEntry {
    pub target: Target,
    pub plan: Option<PlanItem>,
    /// Error from the last run, shown under the row.
    pub last_error: Option<String>,
}

/// The confirmation dialog.
pub struct Confirm {
    pub targets: Vec<Target>,
    pub plans: Vec<PlanItem>,
    pub rx: Option<Receiver<PlanItem>>,
    pub opts: WipeOptions,
    pub typed: String,
    pub focused: bool,
    /// Password entered in the dialog (when wiping is password-protected).
    pub password: String,
    pub pw_error: Option<String>,
}

/// A running or finished job as shown by the progress view.
pub struct JobView {
    pub job: Job,
    pub targets: Vec<Target>,
    pub opts: WipeOptions,
    pub total: u64,
    pub done: u64,
    pub current: Option<usize>,
    pub item: String,
    pub pass: String,
    pub pass_i: usize,
    pub stage: String,
    pub log: VecDeque<String>,
    pub results: Vec<Option<ReportItem>>,
    pub started: Instant,
    pub paused_since: Option<Instant>,
    pub paused_total: Duration,
    pub finished: bool,
    pub report: Option<Report>,
    pub report_path: Option<PathBuf>,
    /// Files another program had open, and the targets they belong to.
    pub locked_files: Vec<PathBuf>,
    pub locked_targets: Vec<usize>,
    /// Who holds them (Restart Manager).
    pub lockers: Vec<freewipe::sys::Locker>,
    /// Waiting for "close the programs" to finish.
    pub closing: Option<Receiver<Result<(), String>>>,
    pub inuse_msg: Option<(bool, String)>,
    /// Frozen elapsed time once the job is over.
    pub elapsed_at_finish: Option<Duration>,
}

impl JobView {
    pub fn active_elapsed(&self) -> Duration {
        if let Some(d) = self.elapsed_at_finish {
            return d;
        }
        let paused = self.paused_total + self.paused_since.map(|p| p.elapsed()).unwrap_or_default();
        self.started.elapsed().saturating_sub(paused)
    }
    pub fn log(&mut self, line: String) {
        freewipe::dlog!("job log: {line}");
        let stamp = freewipe::util::human_duration(self.started.elapsed());
        self.log.push_back(format!("{stamp:>7}  {line}"));
        while self.log.len() > 400 {
            self.log.pop_front();
        }
    }
}

/// One row of the free-space view.
pub struct VolumeRow {
    pub root: PathBuf,
    pub info: Option<freewipe::filesystem::VolumeInfo>,
    pub media: freewipe::media::MediaInfo,
}

/// One row of the reports view.
pub struct ReportRow {
    pub path: PathBuf,
    pub report: Option<Report>,
}

pub struct App {
    pub settings: Settings,
    saved: Settings,
    pub view: View,
    pub tab: SettingsTab,
    pub queue: Vec<QueueEntry>,
    scans: Vec<Receiver<PlanItem>>,
    pub method: Method,
    pub verify: bool,
    pub keep: bool,
    pub confirm: Option<Confirm>,
    pub job: Option<JobView>,
    pub confirm_exit: bool,
    exit_now: bool,
    pending: Vec<Request>,
    last_arrival: Instant,
    last_poll: Instant,
    pub volumes: Option<Vec<VolumeRow>>,
    pub reports: Option<Vec<ReportRow>>,
    pub shell_status: Option<ShellStatus>,
    pub shell_applied: Option<ShellConfig>,
    pub shell_msg: Option<(bool, String)>,
    pub toast: Option<(String, Instant)>,
    pub ctx: egui::Context,
    /// Compact window opened from Explorer.
    pub quick: bool,
    /// Password screen shown before anything else.
    pub locked: bool,
    /// Settings already unlocked in this session.
    pub settings_unlocked: bool,
    pub lock: LockInput,
    /// Password fields on the Safety tab.
    pub pw: PasswordForm,
    /// Context-menu entries from another freewipe version/copy: (all_users, status).
    pub foreign: Vec<(bool, ShellStatus)>,
    pub foreign_dismissed: bool,
    /// The compact window has been shrunk to the progress size.
    pub mini_applied: bool,
    /// The user asked to keep the small window open.
    pub mini_keep_open: bool,
    /// Started with a valid unlock token (elevated retry): no second prompt.
    pub wipe_preauthorized: bool,
    /// Notification-area icon while wiping.
    pub tray: Option<Tray>,
    /// Folder-filter dialog in the queue.
    pub filter_edit: Option<views::queue::FilterEdit>,
    /// Custom-method editor.
    pub method_edit: Option<views::methods::MethodEdit>,
    pub prot_edit: Option<views::protected::ProtEdit>,
    /// Scheduled wipes: Task Scheduler state and the editor.
    pub sched: views::schedule::SchedState,
    pub sched_edit: Option<views::schedule::SchedEdit>,
    /// Reports page: confirm "delete all" (false) / "wipe all" (true).
    pub reports_confirm: Option<bool>,
    /// The next finished job saves no report (wiping the reports).
    pub skip_report_once: bool,
    /// The window is hidden to that icon.
    pub hidden: bool,
    tray_updated: Instant,
    hidden_done_at: Option<Instant>,
    shown_once: bool,
}

/// State of a password prompt (shared by the app lock and settings lock).
#[derive(Default)]
pub struct LockInput {
    pub text: String,
    pub failures: u32,
    pub wait_until: Option<Instant>,
    pub error: Option<String>,
    pub focus_requested: bool,
}

impl LockInput {
    /// Check `text`; on failure apply an increasing delay.
    pub fn try_unlock(&mut self, lock: &freewipe::lock::Lock) -> bool {
        if self.wait_until.map(|t| Instant::now() < t).unwrap_or(false) {
            return false;
        }
        let ok = lock.verify(&self.text);
        self.text.clear();
        if ok {
            *self = LockInput::default();
        } else {
            self.failures += 1;
            self.error = Some(tr!("lock.wrong"));
            if self.failures >= 3 {
                self.wait_until = Some(Instant::now() + Duration::from_secs(5 * u64::from(self.failures - 2)));
            }
        }
        ok
    }
}

/// Set / change / remove password form.
#[derive(Default)]
pub struct PasswordForm {
    pub current: String,
    pub new1: String,
    pub new2: String,
    pub msg: Option<(bool, String)>,
}

impl App {
    pub fn new(settings: Settings, request: Request, ctx: egui::Context, quick: bool, unlocked: bool) -> Self {
        let method = settings.method();
        let locked = settings.lock.app_locked() && !unlocked;
        let mut app = App {
            quick,
            locked,
            settings_unlocked: unlocked,
            lock: LockInput::default(),
            pw: PasswordForm::default(),
            foreign: if quick { Vec::new() } else { crate::shellops::foreign_entries() },
            foreign_dismissed: false,
            mini_applied: false,
            mini_keep_open: false,
            wipe_preauthorized: unlocked,
            verify: settings.verify,
            keep: settings.keep,
            saved: settings.clone(),
            settings,
            view: View::Queue,
            tab: SettingsTab::General,
            queue: Vec::new(),
            scans: Vec::new(),
            method,
            confirm: None,
            job: None,
            confirm_exit: false,
            exit_now: false,
            pending: Vec::new(),
            last_arrival: Instant::now(),
            last_poll: Instant::now(),
            volumes: None,
            reports: None,
            shell_status: None,
            shell_applied: None,
            shell_msg: None,
            toast: None,
            ctx,
            tray: None,
            filter_edit: None,
            method_edit: None,
            prot_edit: None,
            sched: Default::default(),
            sched_edit: None,
            reports_confirm: None,
            skip_report_once: false,
            hidden: false,
            tray_updated: Instant::now(),
            hidden_done_at: None,
            shown_once: false,
        };
        if app.settings.shell.enabled {
            app.shell_applied = Some(app.settings.shell.clone());
        }
        // Requests that arrived before this window existed.
        app.pending.extend(inbox::drain());
        inbox::watch(app.ctx.clone());
        if !request.is_empty() {
            if request.batched && app.pending.is_empty() {
                // Complete selection already: act without the batching wait.
                app.last_arrival = Instant::now() - Duration::from_secs(5);
            }
            app.pending.push(request);
        }
        app
    }

    // --- queue ---------------------------------------------------------------

    pub fn add_targets(&mut self, targets: Vec<Target>) {
        let fresh: Vec<Target> = targets.into_iter().filter(|t| !self.queue.iter().any(|q| q.target == *t)).collect();
        if fresh.is_empty() {
            return;
        }
        for t in &fresh {
            self.queue.push(QueueEntry { target: t.clone(), plan: None, last_error: None });
        }
        self.scans.push(worker::scan(fresh, self.settings.force, self.ctx.clone()));
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        self.add_targets(paths.into_iter().map(Target::auto).collect());
    }

    fn poll_scans(&mut self) {
        self.scans.retain(|rx| {
            let mut alive = true;
            loop {
                match rx.try_recv() {
                    Ok(p) => {
                        if let Some(q) = self.queue.iter_mut().find(|q| q.target == p.target) {
                            q.plan = Some(p);
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        alive = false;
                        break;
                    }
                }
            }
            alive
        });
    }

    /// Re-scan everything (e.g. after toggling `force`).
    pub fn rescan_queue(&mut self) {
        let targets: Vec<Target> = self.queue.iter().map(|q| q.target.clone()).collect();
        for q in &mut self.queue {
            q.plan = None;
        }
        if !targets.is_empty() {
            self.scans.push(worker::scan(targets, self.settings.force, self.ctx.clone()));
        }
    }

    pub fn current_options(&self) -> WipeOptions {
        let mut o = self.settings.wipe_options();
        o.method = self.method;
        o.verify = self.verify;
        o.keep = self.keep;
        o
    }

    // --- confirm & run -----------------------------------------------------------

    pub fn ask_confirm(&mut self, targets: Vec<Target>, opts: WipeOptions) {
        if targets.is_empty() {
            return;
        }
        // Reuse plans we already have from the queue.
        let mut plans = Vec::new();
        let mut missing = Vec::new();
        for t in &targets {
            match self.queue.iter().find(|q| q.target == *t).and_then(|q| q.plan.clone()) {
                Some(p) => plans.push(p),
                None => missing.push(t.clone()),
            }
        }
        let rx = (!missing.is_empty()).then(|| worker::scan(missing, opts.force, self.ctx.clone()));
        self.confirm = Some(Confirm {
            targets,
            plans,
            rx,
            opts,
            typed: String::new(),
            focused: false,
            password: String::new(),
            pw_error: None,
        });
    }

    /// Wipe `targets`: with confirmation unless the settings say otherwise.
    pub fn request_wipe(&mut self, targets: Vec<Target>, opts: WipeOptions, confirmed: bool) {
        if targets.is_empty() {
            return;
        }
        let password_needed = self.settings.lock.wipe_locked() && !self.wipe_preauthorized;
        freewipe::dlog!(
            "wipe requested: {} targets, confirmed={confirmed}, password_needed={password_needed}, confirm_mode={:?}",
            targets.len(),
            self.settings.confirm_mode
        );
        if !password_needed && (confirmed || self.settings.confirm_mode == ConfirmMode::None) {
            let plans: Vec<PlanItem> = targets
                .iter()
                .map(|t| {
                    self.queue
                        .iter()
                        .find(|q| q.target == *t)
                        .and_then(|q| q.plan.clone())
                        .unwrap_or_else(|| freewipe::plan::scan(t, opts.force))
                })
                .collect();
            let ok: Vec<Target> = plans.iter().filter(|p| p.is_ok()).map(|p| p.target.clone()).collect();
            let ok_plans: Vec<PlanItem> = plans.into_iter().filter(|p| p.is_ok()).collect();
            if ok.is_empty() {
                self.expand();
                self.toast(tr!("cli.nothing_to_do"));
                return;
            }
            self.start_job(ok, &ok_plans, opts);
        } else {
            self.ask_confirm(targets, opts);
        }
    }

    /// Relaunch freewipe-gui as administrator for the items that failed with
    /// "access denied". Returns `false` if UAC was declined.
    #[cfg(windows)]
    pub fn retry_as_admin(&mut self) -> bool {
        let Some(jv) = &self.job else { return false };
        let Ok(exe) = std::env::current_exe() else { return false };
        let method = jv.opts.method.key().to_string();
        // Whole targets (with folder filters and move destinations).
        let denied: Vec<Target> = jv
            .targets
            .iter()
            .zip(jv.results.iter())
            .filter(|(_, r)| r.as_ref().map(|r| r.access_denied).unwrap_or(false))
            .map(|(t, _)| t.clone())
            .collect();
        let groups = vec![Request {
            action: Action::Wipe,
            method: Some(method),
            targets: denied,
            confirmed: true,
            ..Default::default()
        }];
        let mut launched = false;
        for g in groups {
            let mut args = g.to_args();
            args.push("--elevated".into());
            if self.settings.lock.app_locked() {
                if let Some(tok) = inbox::issue_unlock_token() {
                    args.push("--unlock-token".into());
                    args.push(tok);
                }
            }
            let params: Vec<String> = args.iter().map(|a| freewipe::util::quote_arg(a)).collect();
            match freewipe::sys::launch_elevated(&exe, &params.join(" ")) {
                Ok(()) => launched = true,
                Err(_) => {
                    self.toast(tr!("gui.uac_cancelled"));
                    return false;
                }
            }
        }
        launched
    }

    /// Show the full main window (leave the compact Explorer window).
    pub fn expand(&mut self) {
        if self.quick {
            self.quick = false;
            self.ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(860.0, 560.0)));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1120.0, 720.0)));
        }
    }

    pub fn start_job(&mut self, targets: Vec<Target>, plans: &[PlanItem], opts: WipeOptions) {
        let passes = opts.pass_count();
        let total = plans.iter().filter(|p| p.is_ok()).map(|p| p.work_bytes(passes, opts.verify)).sum();
        freewipe::dlog!(
            "job start: {} targets, {} work bytes, keep_awake={}",
            targets.len(),
            total,
            self.settings.keep_awake
        );
        for p in plans {
            freewipe::dlog!(
                "  plan: {} bytes={} files={} dirs={} link={} shortcut={} blocked={:?}",
                p.target.display(),
                p.bytes,
                p.files,
                p.dirs,
                p.link,
                p.shortcut,
                p.blocked
            );
        }
        let job = worker::start(targets.clone(), opts.clone(), self.settings.keep_awake, self.ctx.clone());
        let mut jv = JobView {
            job,
            results: vec![None; targets.len()],
            targets,
            opts,
            total,
            done: 0,
            current: None,
            item: String::new(),
            pass: String::new(),
            pass_i: 0,
            stage: String::new(),
            log: VecDeque::new(),
            started: Instant::now(),
            paused_since: None,
            paused_total: Duration::ZERO,
            finished: false,
            report: None,
            report_path: None,
            locked_files: Vec::new(),
            locked_targets: Vec::new(),
            lockers: Vec::new(),
            closing: None,
            inuse_msg: None,
            elapsed_at_finish: None,
        };
        jv.log(tr!("gui.log_started", method = jv.opts.method.name(), passes = passes));
        self.job = Some(jv);
        self.view = View::Job;
        self.mini_keep_open = false;
        self.hidden_done_at = None;
        self.ctx.data_mut(|d| {
            d.remove::<Instant>(egui::Id::unique("mini-close-at"));
            d.remove::<bool>(egui::Id::unique("mini-grown"));
        });
        if self.quick && self.mini_applied {
            let size = egui::vec2(views::quick::MINI_W, views::quick::MINI_H);
            self.ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        if (self.settings.tray_icon || self.hidden) && self.tray.is_none() {
            self.tray = Tray::new(&self.ctx, "freewipe");
        }
        if self.hidden && self.tray.is_none() {
            // No icon to come back from: show the window after all.
            self.show_window();
        }
    }

    fn poll_job(&mut self) {
        let Some(jv) = self.job.as_mut() else { return };
        let mut finished = false;
        while let Ok(m) = jv.job.rx.try_recv() {
            match m {
                Msg::Started(i) => {
                    jv.current = Some(i);
                    let name = jv.targets[i].display();
                    jv.log(tr!("gui.log_target", name = name));
                }
                Msg::Item(s) => jv.item = s,
                Msg::Pass(i, n, l) => {
                    jv.pass_i = i;
                    jv.pass = tr!("gui.pass_of", i = i + 1, n = n, label = l);
                }
                Msg::Stage(s) => jv.stage = s.label(),
                Msg::Advance(b) => jv.done += b,
                Msg::Note(n) => jv.log(n),
                Msg::Done(i, item) => {
                    let line = match item.status {
                        Status::Ok => tr!(
                            "gui.log_ok",
                            name = item.target,
                            bytes = freewipe::util::human_bytes(item.bytes_overwritten)
                        ),
                        _ => tr!("gui.log_fail", name = item.target, err = item.error.clone().unwrap_or_default()),
                    };
                    jv.log(line);
                    for f in item.failures.iter().take(20) {
                        jv.log(format!("   {f}"));
                    }
                    if let Some(slot) = jv.results.get_mut(i) {
                        *slot = Some(*item);
                    }
                }
                Msg::Finished => finished = true,
            }
        }
        if finished && !jv.finished {
            jv.elapsed_at_finish = Some(jv.active_elapsed());
            jv.finished = true;
            jv.current = None;
            self.finish_job();
        }
    }

    fn finish_job(&mut self) {
        let settings = self.settings.clone();
        let Some(jv) = self.job.as_mut() else { return };
        let mut report = Report::new(jv.opts.method, jv.opts.pass_count(), jv.opts.verify, jv.opts.keep, false);
        for (i, r) in jv.results.iter().enumerate() {
            match r {
                Some(item) => report.push(item.clone()),
                None => {
                    let mut it = ReportItem::new(&jv.targets[i].path, jv.targets[i].kind.key());
                    it.status = Status::Cancelled;
                    report.push(it);
                }
            }
        }
        report.finish();
        freewipe::dlog!("job finished: {:?}", report.summary);
        let skip_report = std::mem::take(&mut self.skip_report_once);
        let Some(jv) = self.job.as_mut() else { return };
        if settings.save_reports && !skip_report {
            let mut to_save = report.clone();
            if settings.redact_reports {
                to_save.redact();
            }
            let path = Report::default_path(&settings.report_dir());
            if to_save.save(&path).is_ok() {
                jv.log(tr!("gui.log_report", path = path.display()));
                jv.report_path = Some(path);
            }
        }
        let s = &report.summary;
        jv.log(tr!(
            "gui.log_summary",
            ok = s.ok,
            failed = s.failed + s.partial,
            cancelled = s.cancelled,
            skipped = s.skipped
        ));
        // Files that were open in another program.
        for (i, r) in jv.results.iter().enumerate() {
            if let Some(r) = r.as_ref().filter(|r| !r.locked.is_empty()) {
                jv.locked_targets.push(i);
                jv.locked_files.extend(r.locked.iter().map(PathBuf::from));
            }
        }
        if !jv.locked_files.is_empty() {
            jv.lockers = freewipe::sys::lockers(&jv.locked_files).unwrap_or_default();
        }
        let in_use = jv.locked_files.len();
        jv.report = Some(report);
        self.reports = None;
        self.notify_finished(in_use);
        if settings.notify_done {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        if self.exit_now {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Tell the user via the notification-area icon (when the window is
    /// hidden or in the background); drop the icon otherwise.
    fn notify_finished(&mut self, in_use: usize) {
        let Some(tray) = &self.tray else { return };
        let Some(s) = self.job.as_ref().and_then(|j| j.report.as_ref()).map(|r| r.summary.clone()) else { return };
        let focused = self.ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if !self.hidden && focused {
            self.tray = None;
            return;
        }
        let (title, text, level) = if in_use > 0 {
            (tr!("gui.tray_problems"), tr!("gui.tray_in_use", n = in_use), Level::Warning)
        } else if s.failed + s.partial > 0 {
            (tr!("gui.tray_problems"), tr!("gui.tray_failed", n = s.failed + s.partial), Level::Error)
        } else if s.cancelled > 0 {
            (tr!("gui.tray_stopped"), tr!("gui.tray_click"), Level::Info)
        } else {
            (tr!("gui.tray_done"), tr!("gui.tray_ok", n = s.ok), Level::Info)
        };
        tray.set_tip(&format!("freewipe — {title}"));
        tray.notify(&title, &text, level);
    }

    /// Bring the window back from the notification area.
    pub fn show_window(&mut self) {
        freewipe::dlog!("window shown");
        self.hidden = false;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if !self.is_running() {
            self.tray = None;
        }
    }

    /// Hide the window; the notification-area icon stays.
    pub fn hide_to_tray(&mut self) {
        if self.tray.is_some() && !self.hidden {
            self.hidden = true;
            freewipe::dlog!("window hidden to the notification area");
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    /// Icon clicks, tooltip, minimise-to-tray; auto-close of a hidden
    /// compact window after a clean run. Runs even while hidden.
    fn tick_tray(&mut self) {
        let Some(tray) = &self.tray else { return };
        let events = tray.take_events();
        if events.iter().any(|e| matches!(e, TrayEvent::Show | TrayEvent::NotificationClicked)) {
            self.show_window();
            return;
        }
        let running = self.is_running();
        if running && self.tray_updated.elapsed() >= Duration::from_secs(1) {
            self.tray_updated = Instant::now();
            if let Some(jv) = &self.job {
                let pct = if jv.total > 0 { (jv.done as f64 / jv.total as f64 * 100.0).min(100.0) } else { 0.0 };
                let name = jv.current.map(|c| views::queue::names(&jv.targets[c]).0).unwrap_or_default();
                tray.set_tip(&format!("freewipe — {pct:.0}% · {name}"));
            }
            self.ctx.request_repaint_after(Duration::from_secs(1));
        }
        let minimized = self.ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if running && minimized && self.settings.minimize_to_tray && !self.hidden {
            self.hide_to_tray();
        }
        // A hidden compact window closes itself after a clean run.
        if self.hidden && self.quick && !running {
            let clean = self.job.as_ref().map(|j| {
                j.locked_files.is_empty()
                    && j.results.iter().all(|r| r.as_ref().map(|r| r.status == Status::Ok).unwrap_or(false))
            });
            let secs = u64::from(self.settings.finish_close_secs);
            if clean == Some(true) && secs > 0 {
                let at = *self.hidden_done_at.get_or_insert_with(Instant::now);
                let wait = Duration::from_secs(secs.max(8));
                if at.elapsed() >= wait {
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.ctx.request_repaint_after(wait - at.elapsed());
                }
            }
        }
    }

    // --- files in use --------------------------------------------------------------

    /// Ask the programs holding the files to close, then retry.
    pub fn close_lockers_and_retry(&mut self) {
        let Some(jv) = self.job.as_mut() else { return };
        let files = jv.locked_files.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = freewipe::sys::close_lockers(&files).map_err(|e| e.to_string());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        jv.closing = Some(rx);
        jv.inuse_msg = Some((true, tr!("gui.inuse_closing")));
    }

    fn poll_closing(&mut self) {
        let Some(jv) = self.job.as_mut() else { return };
        let Some(rx) = &jv.closing else { return };
        let Ok(r) = rx.try_recv() else { return };
        jv.closing = None;
        match r {
            Ok(()) => self.retry_locked(),
            Err(e) => {
                jv.lockers = freewipe::sys::lockers(&jv.locked_files).unwrap_or_default();
                jv.inuse_msg = Some((false, tr!("gui.inuse_close_failed", err = e)));
            }
        }
    }

    /// Run the targets that had files in use again (already confirmed).
    pub fn retry_locked(&mut self) {
        let Some(jv) = &self.job else { return };
        let targets: Vec<Target> = jv.locked_targets.iter().map(|&i| jv.targets[i].clone()).collect();
        let opts = jv.opts.clone();
        let plans: Vec<PlanItem> = targets.iter().map(|t| freewipe::plan::scan(t, opts.force)).collect();
        let ok: Vec<Target> = plans.iter().filter(|p| p.is_ok()).map(|p| p.target.clone()).collect();
        let ok_plans: Vec<PlanItem> = plans.into_iter().filter(|p| p.is_ok()).collect();
        if ok.is_empty() {
            return;
        }
        self.start_job(ok, &ok_plans, opts);
    }

    /// Wipe the files in use at the next Windows sign-in.
    pub fn wipe_at_next_signin(&mut self) {
        let Some(jv) = self.job.as_mut() else { return };
        let targets: Vec<Target> = jv.locked_targets.iter().map(|&i| jv.targets[i].clone()).collect();
        jv.inuse_msg = Some(match inbox::schedule_pending(&targets, jv.opts.method.key()) {
            Ok(()) => (true, tr!("gui.inuse_scheduled", n = targets.len())),
            Err(e) => (false, e.to_string()),
        });
    }

    /// Leave the progress view: drop wiped items from the queue.
    pub fn close_job(&mut self) {
        let Some(jv) = self.job.take() else { return };
        self.tray = None;
        for (t, r) in jv.targets.iter().zip(jv.results.iter()) {
            match r {
                Some(item)
                    if matches!(item.status, Status::Ok)
                        || (item.status == Status::Skipped
                            && !Path::new(&item.target).exists()
                            && t.kind != TargetKind::FreeSpace) =>
                {
                    self.queue.retain(|q| q.target != *t);
                }
                Some(item) => {
                    if let Some(q) = self.queue.iter_mut().find(|q| q.target == *t) {
                        q.last_error = item.error.clone();
                    }
                }
                None => {}
            }
        }
        // Folders that were partially wiped changed size.
        self.rescan_queue();
        self.view = View::Queue;
    }

    pub fn is_running(&self) -> bool {
        self.job.as_ref().map(|j| !j.finished).unwrap_or(false)
    }

    // --- context-menu requests -----------------------------------------------------

    fn poll_inbox(&mut self) {
        if self.last_poll.elapsed() > Duration::from_millis(200) {
            self.last_poll = Instant::now();
            let new = inbox::drain();
            if !new.is_empty() {
                self.pending.extend(new);
                self.last_arrival = Instant::now();
                if self.hidden {
                    self.show_window();
                }
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // Explorer starts one process per selected item: wait until the burst
        // settles, then treat everything as one batch.
        if !self.locked && !self.pending.is_empty() && self.last_arrival.elapsed() > Duration::from_millis(450) {
            let batch = std::mem::take(&mut self.pending);
            self.handle_batch(batch);
        }
        // While a burst settles, look again soon. Otherwise the watcher
        // thread wakes us when a request arrives — no idle repainting.
        if !self.pending.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn handle_batch(&mut self, batch: Vec<Request>) {
        freewipe::dlog!("request batch ({}): {:?}", batch.len(), batch);
        let mut enqueue = Vec::new();
        let mut wipe = Vec::new();
        let mut moves: Vec<PathBuf> = Vec::new();
        let mut method: Option<Method> = None;
        let mut confirmed = false;
        for r in batch {
            if let Some(m) = r.method.as_deref().and_then(Method::from_key) {
                method = Some(m);
            }
            confirmed |= r.confirmed;
            match r.action {
                Action::Focus => {}
                Action::Enqueue => enqueue.extend(r.paths.into_iter().map(Target::auto)),
                Action::Wipe => wipe.extend(r.paths.into_iter().map(Target::auto)),
                Action::FreeSpace => wipe.extend(r.paths.into_iter().map(Target::free_space)),
                Action::RecycleBin => wipe.push(Target::recycle_bin()),
                Action::Move => moves.extend(r.paths),
            }
            wipe.extend(r.targets);
        }
        if !moves.is_empty() {
            match self.pick_move_destination(&moves) {
                Some(dest) => wipe.extend(moves.into_iter().map(|p| Target::move_to(p, dest.clone()))),
                None if wipe.is_empty() && enqueue.is_empty() && self.quick && self.job.is_none() => {
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                None => {}
            }
        }
        wipe.dedup();
        if let Some(m) = method {
            self.method = m;
        }
        if !enqueue.is_empty() {
            self.expand();
            self.add_targets(enqueue);
        }
        let busy = self.is_running() || self.confirm.is_some();
        if wipe.is_empty() {
            if !busy && !self.quick {
                self.view = View::Queue;
            }
            return;
        }
        let sc = self.settings.shell.clone();
        if busy || (sc.menu_action == MenuAction::Enqueue && !confirmed) {
            // Busy, or the user prefers the queue: add and show it.
            self.expand();
            self.add_targets(wipe);
            if busy {
                self.toast(tr!("gui.added"));
            } else {
                self.view = View::Queue;
            }
            return;
        }
        let mut opts = self.current_options();
        if let Some(m) = method {
            opts.method = m;
        }
        let skip_confirm = confirmed || !sc.always_confirm;
        self.request_wipe(wipe, opts, skip_confirm);
    }

    /// Ask where to move `items` (folder picker).
    pub fn pick_move_destination(&self, items: &[PathBuf]) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_title(tr!("gui.move_pick", n = items.len()));
        if let Some(parent) = items.first().and_then(|p| p.parent()) {
            d = d.set_directory(parent);
        }
        d.pick_folder()
    }

    /// Run a schedule started by the Task Scheduler (no confirmation: it
    /// was set up by the user). `hidden`: only the tray icon is shown.
    pub fn run_schedule(&mut self, s: &freewipe::schedule::Schedule, hidden: bool) {
        freewipe::dlog!("scheduled run: {:?} hidden={hidden}", s);
        self.hidden = hidden;
        self.wipe_preauthorized = true;
        let mut opts = self.settings.wipe_options();
        if let Some(m) = Method::from_key(&s.method) {
            opts.method = m;
        }
        opts.verify = self.settings.verify || opts.method.verify_recommended();
        opts.keep = false;
        if let freewipe::schedule::ScheduleTarget::FreeSpace { cluster_tips, file_records, .. } = &s.target {
            opts.cluster_tips = *cluster_tips;
            opts.file_records = *file_records;
        }
        let plans: Vec<PlanItem> = s.targets().iter().map(|t| freewipe::plan::scan(t, opts.force)).collect();
        let ok: Vec<Target> = plans.iter().filter(|p| p.is_ok()).map(|p| p.target.clone()).collect();
        let ok_plans: Vec<PlanItem> = plans.into_iter().filter(|p| p.is_ok()).collect();
        if ok.is_empty() {
            // Nothing there (e.g. the folder is gone): end quietly.
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.start_job(ok, &ok_plans, opts);
    }

    // --- settings persistence -------------------------------------------------------

    fn persist_settings(&mut self) {
        if self.settings != self.saved {
            if self.settings.lock != self.saved.lock {
                // Any change to the lock is dated so it outranks older copies.
                if self.settings.lock.updated == self.saved.lock.updated {
                    self.settings.lock.touch();
                }
                let _ = freewipe::lock::save_user_stores(&self.settings.lock);
                #[cfg(windows)]
                if self.settings.lock.machine || self.saved.lock.machine {
                    self.push_machine_lock();
                }
            }
            if self.settings.language != self.saved.language {
                i18n::set_language(&self.settings.language);
                self.volumes = None;
            }
            if self.settings.theme != self.saved.theme {
                theme::apply_theme(&self.ctx, self.settings.theme);
            }
            if self.settings.debug_log != self.saved.debug_log {
                freewipe::debuglog::set_enabled(self.settings.debug_log);
            }
            if self.settings.force != self.saved.force || self.settings.protection != self.saved.protection {
                freewipe::safety::set_protection(&self.settings.protection);
                freewipe::dlog!("protected paths changed: {:?}", self.settings.protection);
                self.rescan_queue();
            }
            let r = self.settings.save();
            freewipe::dlog!("settings saved ({:?})", r.err());
            self.saved = self.settings.clone();
        }
    }

    /// Write (or remove) the machine-wide copy of the lock via an elevated
    /// `freewipe.exe lock-machine`. Reverts the "machine" flag if UAC is declined.
    #[cfg(windows)]
    pub fn push_machine_lock(&mut self) {
        let Ok(gui) = std::env::current_exe() else { return };
        let res = if self.settings.lock.machine && self.settings.lock.has_password() {
            let tmp = std::env::temp_dir().join(format!("freewipe-lock-{}.json", std::process::id()));
            let r = std::fs::write(&tmp, serde_json::to_string(&self.settings.lock).unwrap_or_default())
                .map_err(|e| e.to_string())
                .and_then(|_| crate::shellops::elevated(&gui, &format!("lock-machine --config \"{}\"", tmp.display())));
            let _ = std::fs::remove_file(&tmp);
            r
        } else {
            crate::shellops::elevated(&gui, "lock-machine --clear")
        };
        match res {
            Ok(()) => self.toast(tr!("gui.pw_machine_ok")),
            Err(e) => {
                self.settings.lock.machine = freewipe::lock::machine_lock_present();
                self.pw.msg = Some((false, e));
            }
        }
    }

    pub fn toast(&mut self, msg: String) {
        freewipe::dlog!("toast: {msg}");
        self.toast = Some((msg, Instant::now()));
    }

    // --- layout --------------------------------------------------------------------

    fn nav(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        egui::Panel::left("nav")
            .exact_size(214.0)
            .resizable(false)
            .frame(
                Frame::new().fill(p.panel).inner_margin(Margin::symmetric(10, 16)).stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("freewipe").size(20.0).strong());
                    ui.label(RichText::new(freewipe::VERSION).color(p.muted).size(12.0));
                });
                ui.add_space(14.0);
                let running = self.is_running();
                let n = self.queue.len();
                let ns = self.settings.schedules.len();
                let items: [(View, &str, String, Option<usize>); 5] = [
                    (View::Queue, "☰", tr!("gui.nav_queue"), (n > 0).then_some(n)),
                    (View::FreeSpace, "🖴", tr!("gui.nav_freespace"), None),
                    (View::Methods, "🔧", tr!("gui.nav_methods"), None),
                    (View::Schedule, "🕒", tr!("gui.nav_schedule"), (ns > 0).then_some(ns)),
                    (View::Reports, "🗐", tr!("gui.nav_reports"), None),
                ];
                let mut go: Option<View> = None;
                for (v, icon, label, count) in items {
                    if widgets::nav_item(ui, icon, &label, self.view == v, count).clicked() {
                        go = Some(v);
                    }
                }
                if self.job.is_some() {
                    let label = if running { tr!("gui.nav_running") } else { tr!("gui.nav_done") };
                    if widgets::nav_item(ui, "⟳", &label, self.view == View::Job, None).clicked() {
                        go = Some(View::Job);
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    if widgets::nav_item(ui, "⚙", &tr!("gui.nav_settings"), self.view == View::Settings, None).clicked()
                    {
                        go = Some(View::Settings);
                    }
                });
                if let Some(v) = go {
                    // Leaving a finished job's screen closes it.
                    if v != View::Job
                        && self.view == View::Job
                        && self.job.as_ref().map(|j| j.finished).unwrap_or(false)
                    {
                        self.close_job();
                    }
                    self.view = v;
                }
            });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 26, right: 26, top: 22, bottom: 12 }))
            .show(ui, |ui| match self.view {
                View::Job if self.job.is_some() => views::job::show(self, ui),
                View::Job => views::queue::show(self, ui),
                View::Queue => views::queue::show(self, ui),
                View::FreeSpace => views::volumes::show(self, ui),
                View::Methods => views::methods::show(self, ui),
                View::Schedule => views::schedule::show(self, ui),
                View::Reports => views::reports::show(self, ui),
                View::Settings => views::settings::show(self, ui),
            });
    }

    fn handle_close(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && self.is_running() && !self.exit_now {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_exit = true;
        }
        if !self.confirm_exit {
            return;
        }
        let resp = egui::Modal::new(egui::Id::unique("confirm-exit")).show(&ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(tr!("gui.exit_title"));
            ui.add_space(4.0);
            widgets::hint(ui, &tr!("gui.exit_body"));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::secondary(ui, tr!("gui.exit_continue")).clicked() {
                    self.confirm_exit = false;
                }
                if widgets::accent(ui, tr!("gui.exit_stop")).clicked() {
                    self.exit_now = true;
                    self.confirm_exit = false;
                    if let Some(j) = &self.job {
                        j.job.cancel.set_paused(false);
                        j.job.cancel.cancel();
                    }
                }
            });
        });
        if resp.should_close() {
            self.confirm_exit = false;
        }
    }

    fn handle_drops(&mut self, ui: &egui::Ui) {
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
            if self.view != View::Job {
                self.view = View::Queue;
            }
        }
    }

    fn show_toast(&mut self, ui: &mut egui::Ui) {
        let Some((msg, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_secs(4) {
            self.toast = None;
            return;
        }
        let p = pal(ui);
        let msg = msg.clone();
        egui::Area::new(egui::Id::unique("toast")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -96.0)).show(
            ui.ctx(),
            |ui| {
                Frame::new()
                    .fill(p.card_hi)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| ui.label(msg));
            },
        );
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

impl eframe::App for App {
    /// Runs before every frame and also while the window is hidden in the
    /// notification area, so the job keeps being followed.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Make sure the first window really appears (and comes to the front),
        // however the process was started.
        if !self.shown_once {
            self.shown_once = true;
            if !self.hidden {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        self.poll_scans();
        self.poll_job();
        self.poll_closing();
        self.poll_inbox();
        self.tick_tray();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.locked {
            views::lock::app_lock_screen(self, ui);
            return;
        }
        if self.quick {
            views::quick::show(self, ui);
            self.handle_close(ui);
            self.persist_settings();
            return;
        }
        self.handle_drops(ui);

        self.nav(ui);
        if self.view == View::Queue || (self.view == View::Job && self.job.is_none()) {
            views::queue::action_bar(self, ui);
        }
        self.central(ui);

        if self.confirm.is_some() {
            views::job::confirm_dialog(self, ui);
        }
        self.handle_close(ui);
        self.show_toast(ui);
        self.persist_settings();
    }
}

/// Open a file or folder with the system's default handler.
pub fn open_path(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}
