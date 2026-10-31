//! `freewipe` — command-line front end.

#![deny(unsafe_code)]

mod cli;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use clap::FromArgMatches;
use indicatif::{ProgressBar, ProgressStyle};

use cli::{Cli, Command, Common, ShellAction, ShellOpts};
use freewipe::plan::{self, DirFilter, PlanItem, Target};
use freewipe::progress::{CancelFlag, Ctx, Progress, Stage};
use freewipe::report::{Report, ReportItem, Status};
use freewipe::settings::ShellConfig;
use freewipe::util::{human_bytes, human_duration};
use freewipe::wipe::{self, WipeOptions};
use freewipe::{i18n, shell, tr, Method};

const EXIT_OK: u8 = 0;
const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_CANCELLED: u8 = 130;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let lang = cli::prescan_lang(&args).or_else(|| std::env::var("FREEWIPE_LANG").ok());
    if let Some(l) = &lang {
        if !i18n::set_language(l) {
            eprintln!("freewipe: unknown language '{l}', using English");
        }
    }

    let matches = cli::localized_command().get_matches_from(args);
    let parsed = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };

    match run(parsed) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("freewipe: {e:#}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    let c = cli.common;
    freewipe::debuglog::set_component("cli");
    // Registers the user's custom methods too.
    let _ = freewipe::settings::Settings::load();
    freewipe::dlog!("cli command: {:?}", cli.command);
    // Wiping commands respect the password lock set in the desktop app.
    if matches!(
        cli.command,
        Command::File { .. }
            | Command::Dir { .. }
            | Command::Move { .. }
            | Command::Freespace { .. }
            | Command::RecycleBin
    ) {
        check_password()?;
    }
    match cli.command {
        Command::Methods => {
            print_methods();
            Ok(EXIT_OK)
        }
        Command::Languages => {
            let dirs: Vec<String> = i18n::locale_dirs().iter().map(|d| d.display().to_string()).collect();
            println!("{}", tr!("cli.languages_dirs", dirs = dirs.join("; ")));
            for l in i18n::languages() {
                let src = l.path.map(|p| p.display().to_string()).unwrap_or_else(|| tr!("cli.languages_builtin"));
                println!("  {:<6} {:<22} {:>4.0}%  {src}", l.code, l.name, l.complete * 100.0);
            }
            Ok(EXIT_OK)
        }
        Command::Shell { action } => run_shell(action),
        Command::File { paths } => run_wipe(paths.into_iter().map(Target::file).collect(), &c, FreeExtras::default()),
        Command::Dir { paths, include, exclude, contents_only } => {
            let filter = DirFilter {
                include: include.as_deref().map(DirFilter::parse_masks).unwrap_or_default(),
                exclude: exclude.as_deref().map(DirFilter::parse_masks).unwrap_or_default(),
                contents_only,
            };
            let targets = paths.into_iter().map(|p| Target::dir_filtered(p, filter.clone())).collect();
            run_wipe(targets, &c, FreeExtras::default())
        }
        Command::Move { paths, to } => {
            run_wipe(paths.into_iter().map(|p| Target::move_to(p, to.clone())).collect(), &c, FreeExtras::default())
        }
        Command::Freespace { volumes, reserve_mib, cluster_tips, file_records } => {
            let extra = FreeExtras { reserve: reserve_mib << 20, cluster_tips, file_records };
            run_wipe(volumes.into_iter().map(Target::free_space).collect(), &c, extra)
        }
        Command::RecycleBin => run_wipe(vec![Target::recycle_bin()], &c, FreeExtras::default()),
        Command::LockMachine { config, clear } => lock_machine(config, clear),
    }
}

/// Ask for the password if freewipe is locked (env `FREEWIPE_PASSWORD` for scripts).
fn check_password() -> Result<()> {
    let lock = freewipe::lock::effective(&freewipe::settings::Settings::load().lock);
    if !(lock.app_locked() || lock.wipe_locked()) {
        return Ok(());
    }
    if let Ok(pw) = std::env::var("FREEWIPE_PASSWORD") {
        return if lock.verify(&pw) { Ok(()) } else { bail!(tr!("lock.wrong")) };
    }
    if !std::io::stdin().is_terminal() {
        bail!(tr!("lock.need_password"));
    }
    for attempt in 1..=3 {
        let pw = rpassword::prompt_password(format!("{} ", tr!("lock.prompt"))).context("password")?;
        if lock.verify(&pw) {
            return Ok(());
        }
        eprintln!("{}", tr!("lock.wrong"));
        std::thread::sleep(std::time::Duration::from_secs(attempt));
    }
    bail!(tr!("lock.too_many"))
}

#[cfg(windows)]
fn lock_machine(config: Option<PathBuf>, clear: bool) -> Result<u8> {
    if clear {
        freewipe::lock::save_machine_store(None)?;
        return Ok(EXIT_OK);
    }
    let path = config.ok_or_else(|| anyhow!("--config"))?;
    let lock: freewipe::lock::Lock = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    freewipe::lock::save_machine_store(Some(&lock))?;
    Ok(EXIT_OK)
}

#[cfg(not(windows))]
fn lock_machine(_config: Option<PathBuf>, _clear: bool) -> Result<u8> {
    bail!(tr!("shell.windows_only"))
}

// --- methods -------------------------------------------------------------------

fn print_methods() {
    // Write through a locked handle and ignore errors, so `freewipe methods | head`
    // does not panic on a closed pipe.
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{:<15} {:<28} {:>7} {:>7}  {}",
        tr!("methods.h_key"),
        tr!("methods.h_standard"),
        tr!("methods.h_passes"),
        tr!("methods.h_verify"),
        tr!("methods.h_description")
    );
    let _ = writeln!(out, "{}", "─".repeat(110));
    for m in Method::all() {
        let passes = if m == Method::RandomN { "N".to_string() } else { m.pass_count(None).to_string() };
        let _ = writeln!(
            out,
            "{:<15} {:<28} {:>7} {:>7}  {}",
            m.key(),
            m.name(),
            passes,
            if m.verify_recommended() { tr!("word.yes") } else { "—".into() },
            m.localized_description()
        );
    }
    let _ = writeln!(out, "\n{}", tr!("methods.note"));
}

// --- wipe ----------------------------------------------------------------------

/// Free-space-only options.
#[derive(Default, Clone, Copy)]
struct FreeExtras {
    reserve: u64,
    cluster_tips: bool,
    file_records: bool,
}

fn build_options(c: &Common, extra: FreeExtras) -> Result<WipeOptions> {
    let method = Method::from_key(&c.method).ok_or_else(|| anyhow!(tr!("err.unknown_method", key = c.method)))?;
    let verify = if c.no_verify { false } else { c.verify || method.verify_recommended() };
    if !(1..=64).contains(&c.buffer_mib) {
        bail!(tr!("err.buffer_range"));
    }
    Ok(WipeOptions {
        method,
        passes_override: c.passes,
        verify,
        rename_passes: c.rename_passes,
        keep: c.keep,
        include_ads: !c.no_ads,
        flash_single_pass: c.ssd_single_pass,
        buffer_size: c.buffer_mib << 20,
        force: c.force,
        reserve_bytes: extra.reserve,
        cluster_tips: extra.cluster_tips,
        file_records: extra.file_records,
        filter: None,
    })
}

fn run_wipe(targets: Vec<Target>, c: &Common, extra: FreeExtras) -> Result<u8> {
    let opts = build_options(c, extra)?;
    if c.passes.is_some() && !matches!(opts.method, Method::RandomN | Method::Random) {
        eprintln!("{}", tr!("warn.passes_ignored", method = opts.method.key()));
    }
    let passes = opts.pass_count();

    // 1. Scan.
    let plans: Vec<PlanItem> = targets.iter().map(|t| plan::scan(t, opts.force)).collect();
    print_plan(&plans, &opts, c);

    let runnable: Vec<&PlanItem> = plans.iter().filter(|p| p.is_ok()).collect();
    let mut report = Report::new(opts.method, passes, opts.verify, opts.keep, c.dry_run);
    for p in plans.iter().filter(|p| !p.is_ok()) {
        let mut it = ReportItem::new(&p.target.path, p.target.kind.key());
        it.status = Status::Skipped;
        it.error = p.blocked.clone();
        report.push(it);
    }

    // 2. Dry run.
    if c.dry_run {
        if !runnable.is_empty() {
            print_steps(&opts);
        }
        for p in &runnable {
            let mut it = ReportItem::new(&p.target.path, p.target.kind.key());
            it.status = Status::Planned;
            it.passes = passes;
            it.filesystem = p.volume.as_ref().map(|v| v.fs.display());
            it.media = p.media.as_ref().map(|m| m.label());
            report.push(it);
        }
        println!("\n{}", tr!("cli.dry_run_done"));
        save_log(&mut report, c)?;
        return Ok(if runnable.len() == plans.len() { EXIT_OK } else { EXIT_FAILED });
    }

    if runnable.is_empty() {
        eprintln!("{}", tr!("cli.nothing_to_do"));
        save_log(&mut report, c)?;
        return Ok(EXIT_FAILED);
    }

    // 3. Confirm.
    if !c.yes && !confirm(&runnable)? {
        println!("{}", tr!("cli.aborted"));
        return Ok(EXIT_CANCELLED);
    }

    // 4. Ctrl+C: first press cancels gracefully (temp files are removed),
    //    second press exits immediately.
    let cancel = CancelFlag::new();
    install_ctrlc(cancel.clone());

    // 5. Run.
    let total: u64 = runnable.iter().map(|p| p.work_bytes(passes, opts.verify)).sum();
    let bar = make_bar(total, c.quiet);
    let started = Instant::now();
    let mut progress = CliProgress { bar: bar.clone(), verbose: c.verbose, item: String::new(), pass: String::new() };
    for p in &runnable {
        if cancel.is_cancelled() {
            let mut it = ReportItem::new(&p.target.path, p.target.kind.key());
            it.status = Status::Cancelled;
            report.push(it);
            continue;
        }
        let mut ctx = Ctx::new(&cancel, &mut progress);
        let item = wipe::execute(&p.target, &opts, &mut ctx);
        print_result(&bar, &item, c.quiet);
        report.push(item);
    }
    bar.finish_and_clear();

    // 6. Summary.
    report.finish();
    let s = report.summary.clone();
    println!(
        "\n{}",
        tr!(
            "cli.summary",
            ok = s.ok,
            partial = s.partial,
            failed = s.failed,
            cancelled = s.cancelled,
            skipped = s.skipped,
            bytes = human_bytes(s.bytes_overwritten),
            time = human_duration(started.elapsed())
        )
    );
    // Files open in another program: say who holds them.
    let locked: Vec<std::path::PathBuf> =
        report.items.iter().flat_map(|i| i.locked.iter().map(std::path::PathBuf::from)).collect();
    if !locked.is_empty() {
        let who: Vec<String> = freewipe::sys::lockers(&locked)
            .unwrap_or_default()
            .into_iter()
            .map(|l| format!("{} ({})", l.name, l.pid))
            .collect();
        if who.is_empty() {
            eprintln!("{}", tr!("cli.in_use_unknown", n = locked.len()));
        } else {
            eprintln!("{}", tr!("cli.in_use", n = locked.len(), apps = who.join(", ")));
        }
    }
    save_log(&mut report, c)?;

    Ok(if s.cancelled > 0 {
        EXIT_CANCELLED
    } else if s.failed + s.partial + s.skipped > 0 {
        EXIT_FAILED
    } else {
        EXIT_OK
    })
}

fn print_plan(plans: &[PlanItem], opts: &WipeOptions, c: &Common) {
    if c.quiet {
        for p in plans.iter().filter(|p| !p.is_ok()) {
            eprintln!("{}  {}", tr!("cli.skip"), p.blocked.as_deref().unwrap_or(""));
        }
        return;
    }
    println!(
        "{}",
        tr!(
            "cli.method_line",
            name = opts.method.name(),
            key = opts.method.key(),
            passes = tr!("cli.passes_n", n = opts.pass_count()),
            verify = if opts.verify { tr!("cli.with_verify") } else { String::new() },
            keep = if opts.keep { tr!("cli.keep_note") } else { String::new() }
        )
    );
    println!();
    let mut warnings: Vec<String> = Vec::new();
    for p in plans {
        let size = match p.target.kind {
            plan::TargetKind::FreeSpace => tr!("cli.free_n", size = human_bytes(p.bytes)),
            plan::TargetKind::Dir | plan::TargetKind::RecycleBin | plan::TargetKind::Move => {
                let mut s = format!("{} · {}", human_bytes(p.bytes), tr!("cli.files_n", n = p.files));
                if let Some(f) = &p.target.filter {
                    s.push_str(&format!(" · {}", f.summary()));
                }
                s
            }
            plan::TargetKind::File => human_bytes(p.bytes),
        };
        let fs = p.volume.as_ref().map(|v| v.fs.display()).unwrap_or_else(|| "?".into());
        let media = p.media.as_ref().map(|m| m.label()).unwrap_or_else(|| "?".into());
        match &p.blocked {
            None => {
                println!(
                    "  {:<10} {}\n             {size} · {fs} · {media}",
                    p.target.kind.label(),
                    p.target.display()
                );
                if let Some(n) = p.note() {
                    println!("             {n}");
                }
            }
            Some(why) => println!("  {:<10} {}\n             ✗ {why}", tr!("cli.skip"), p.target.display()),
        }
        if p.is_ok() {
            for w in p.warnings() {
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
        }
    }
    if !warnings.is_empty() {
        println!();
        for w in warnings {
            println!("{} {w}", tr!("cli.warning"));
        }
    }
}

fn print_steps(opts: &WipeOptions) {
    println!("\n{}", tr!("cli.steps"));
    for (i, p) in opts.method.passes(opts.passes_override).iter().enumerate() {
        println!("  {} {}/{}: {}", tr!("cli.pass"), i + 1, opts.pass_count(), p.label());
    }
    if opts.verify {
        println!("  {}", tr!("stage.verify"));
    }
    if !opts.keep {
        println!("  {} ×{}", tr!("stage.rename"), opts.rename_passes);
        println!("  {}", tr!("stage.truncate"));
        println!("  {}", tr!("stage.delete"));
    }
}

fn confirm(runnable: &[&PlanItem]) -> Result<bool> {
    let total: u64 = runnable.iter().map(|p| p.bytes).sum();
    println!("\n{}", tr!("cli.confirm_q", n = runnable.len(), size = human_bytes(total)));
    if !std::io::stdin().is_terminal() {
        bail!(tr!("cli.need_yes"));
    }
    let word = tr!("cli.confirm_word");
    print!("{} ", tr!("cli.confirm_prompt", word = word));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).context("stdin")?;
    let a = line.trim().to_lowercase();
    Ok(a == word.to_lowercase() || a == "yes")
}

fn install_ctrlc(cancel: CancelFlag) {
    static PRESSES: AtomicU32 = AtomicU32::new(0);
    let _ = ctrlc::set_handler(move || {
        if PRESSES.fetch_add(1, Ordering::SeqCst) == 0 {
            cancel.cancel();
            eprintln!("\n{}", tr!("cli.ctrlc_once"));
        } else {
            eprintln!("\n{}", tr!("cli.ctrlc_twice"));
            std::process::exit(EXIT_CANCELLED as i32);
        }
    });
}

fn make_bar(total: u64, quiet: bool) -> ProgressBar {
    if quiet || !std::io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new(total.max(1));
    let tpl = "{spinner} [{elapsed_precise}] {wide_bar} {percent:>3}%  {bytes}/{total_bytes}  {binary_bytes_per_sec}  ETA {eta}\n  {msg}";
    if let Ok(style) = ProgressStyle::with_template(tpl) {
        bar.set_style(style.progress_chars("█▓░"));
    }
    bar.enable_steady_tick(std::time::Duration::from_millis(120));
    bar
}

fn print_result(bar: &ProgressBar, it: &ReportItem, quiet: bool) {
    let line = match it.status {
        Status::Ok | Status::Planned => {
            if quiet {
                return;
            }
            let mut extra = Vec::new();
            if it.streams > 1 && it.kind == "file" {
                extra.push(tr!("cli.streams_n", n = it.streams));
            }
            if it.files_ok > 1 || it.kind != "file" {
                extra.push(tr!("cli.files_n", n = it.files_ok));
            }
            if it.verified {
                extra.push(tr!("cli.verified"));
            }
            if it.deleted {
                extra.push(tr!("cli.deleted"));
            }
            extra.insert(0, human_bytes(it.bytes_overwritten));
            format!("{}  {} — {}", tr!("cli.ok"), it.target, extra.join(" · "))
        }
        Status::Partial => format!(
            "{}  {} — {}",
            tr!("cli.partial"),
            it.target,
            tr!("cli.partial_detail", ok = it.files_ok, err = it.files_err)
        ),
        _ => format!("{}  {} — {}", tr!("cli.err"), it.target, it.error.as_deref().unwrap_or("")),
    };
    bar.suspend(|| println!("{line}"));
    if !quiet {
        for f in it.failures.iter().take(20) {
            bar.suspend(|| println!("      {f}"));
        }
    }
}

fn save_log(report: &mut Report, c: &Common) -> Result<()> {
    if let Some(path) = &c.log {
        report.finish();
        report.save(path).with_context(|| tr!("err.log_write", path = path.display()))?;
        if !c.quiet {
            println!("{}", tr!("cli.log_saved", path = path.display()));
        }
    }
    Ok(())
}

struct CliProgress {
    bar: ProgressBar,
    verbose: bool,
    item: String,
    pass: String,
}

impl CliProgress {
    fn refresh(&self) {
        self.bar.set_message(format!("{}  {}", self.pass, self.item));
    }
}

impl Progress for CliProgress {
    fn item(&mut self, name: &str) {
        self.item = shorten(name, 80);
        if self.verbose {
            self.bar.suspend(|| println!("  → {name}"));
        }
        self.refresh();
    }
    fn pass(&mut self, index: usize, count: usize, label: &str) {
        self.pass = format!("[{} {}/{} {label}]", tr!("cli.pass"), index + 1, count);
        self.refresh();
    }
    fn stage(&mut self, stage: Stage) {
        if matches!(stage, Stage::Verify | Stage::Cleanup | Stage::Fill) {
            self.pass = format!("[{}]", stage.label());
            self.refresh();
        }
    }
    fn advance(&mut self, bytes: u64) {
        self.bar.inc(bytes);
    }
    fn note(&mut self, msg: &str) {
        if self.verbose {
            self.bar.suspend(|| println!("    {msg}"));
        }
    }
}

fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}

// --- shell -----------------------------------------------------------------------

fn shell_config(o: &ShellOpts) -> Result<ShellConfig> {
    if let Some(p) = &o.config {
        let s = std::fs::read_to_string(p).with_context(|| p.display().to_string())?;
        let mut cfg: ShellConfig = serde_json::from_str(&s).context("config")?;
        cfg.all_users = cfg.all_users || o.all_users;
        return Ok(cfg);
    }
    Ok(ShellConfig {
        enabled: true,
        files: !o.no_files,
        dirs: !o.no_dirs,
        drives: !o.no_drives,
        recycle_bin: o.recycle_bin,
        item_wipe: !o.no_wipe,
        item_method: !o.no_method_menu,
        item_queue: !o.no_queue,
        submenu: !o.flat,
        show_icon: !o.no_icon,
        shift_only: o.shift_only,
        all_users: o.all_users,
        ..ShellConfig::default()
    })
}

fn gui_path(o: &ShellOpts) -> Result<PathBuf> {
    match &o.gui {
        Some(p) => Ok(std::fs::canonicalize(p)
            .map(|c| PathBuf::from(freewipe::util::strip_verbatim(&c.display().to_string())))
            .unwrap_or_else(|_| p.clone())),
        None => shell::sibling_gui_exe().ok_or_else(|| anyhow!(tr!("shell.no_gui"))),
    }
}

fn run_shell(action: ShellAction) -> Result<u8> {
    match action {
        ShellAction::Install(o) => {
            let cfg = shell_config(&o)?;
            let gui = gui_path(&o)?;
            let n = shell::install(&cfg, &gui).map_err(|e| anyhow!(tr!("shell.failed", err = e)))?;
            println!("{}", tr!("shell.installed", n = n, scope = scope(cfg.all_users)));
            println!("{}", tr!("shell.win11_note"));
            Ok(EXIT_OK)
        }
        ShellAction::Uninstall { all_users } => {
            let n = shell::uninstall(all_users).map_err(|e| anyhow!(tr!("shell.failed", err = e)))?;
            println!("{}", tr!("shell.removed", n = n, scope = scope(all_users)));
            Ok(EXIT_OK)
        }
        ShellAction::Status { all_users } => {
            let st = shell::status(all_users);
            if st.installed() {
                println!("{}", tr!("shell.status_on", scope = scope(all_users)));
                for v in &st.verbs {
                    println!("  {v}");
                }
                if let Some(cmd) = st.command {
                    println!("  → {cmd}");
                }
            } else {
                println!("{}", tr!("shell.status_off", scope = scope(all_users)));
            }
            Ok(EXIT_OK)
        }
        ShellAction::Export { opts, output } => {
            let cfg = shell_config(&opts)?;
            let gui = opts.gui.clone().map(Ok).unwrap_or_else(|| gui_path(&opts))?;
            let reg = shell::to_reg_file(
                &shell::plan_entries(&cfg, &gui, shell::DropAction::dll_next_to(&gui).as_deref()),
                cfg.all_users,
            );
            match output {
                Some(p) => write_utf16_reg(&p, &reg)?,
                None => print!("{reg}"),
            }
            Ok(EXIT_OK)
        }
    }
}

fn scope(all_users: bool) -> String {
    if all_users {
        tr!("shell.scope_all")
    } else {
        tr!("shell.scope_user")
    }
}

/// regedit expects UTF-16LE with BOM for non-ASCII content.
fn write_utf16_reg(p: &Path, text: &str) -> Result<()> {
    let mut bytes = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(p, bytes).with_context(|| p.display().to_string())
}
