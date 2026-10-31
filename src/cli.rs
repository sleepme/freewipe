//! Command-line definition.
//!
//! Defined with clap's derive API; every help string is then replaced at
//! runtime with its translation (see [`localized_command`]), so `--help`
//! follows `--lang` like the rest of the output.

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};

use freewipe::tr;

#[derive(Parser, Debug)]
#[command(name = "freewipe", version, propagate_version = true, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub common: Common,
}

#[derive(Args, Debug, Clone)]
pub struct Common {
    /// Wiping method (see `freewipe methods`)
    #[arg(short = 'm', long = "method", global = true, value_name = "KEY", default_value = "dod-e")]
    pub method: String,

    /// Number of passes for random / randomN
    #[arg(short = 'p', long = "passes", global = true, value_name = "N")]
    pub passes: Option<usize>,

    /// Verify the last pass by reading it back
    #[arg(long = "verify", global = true, overrides_with = "no_verify")]
    pub verify: bool,

    /// Never verify, even if the method's standard asks for it
    #[arg(long = "no-verify", global = true)]
    pub no_verify: bool,

    /// How many times to rename before deleting
    #[arg(long = "rename-passes", global = true, value_name = "N", default_value_t = 3)]
    pub rename_passes: u32,

    /// Overwrite only; do not delete
    #[arg(long = "keep", global = true)]
    pub keep: bool,

    /// Skip NTFS alternate data streams
    #[arg(long = "no-ads", global = true)]
    pub no_ads: bool,

    /// On SSD/flash use a single pass (NIST Clear) instead of the chosen method
    #[arg(long = "ssd-single-pass", global = true)]
    pub ssd_single_pass: bool,

    /// I/O buffer size in MiB (1–64)
    #[arg(long = "buffer", global = true, value_name = "MIB", default_value_t = 4)]
    pub buffer_mib: usize,

    /// Interface language: en (default), uk, ru, …
    #[arg(short = 'l', long = "lang", global = true, value_name = "CODE")]
    pub lang: Option<String>,

    /// Show the plan without writing anything
    #[arg(short = 'n', long = "dry-run", global = true)]
    pub dry_run: bool,

    /// Do not ask for confirmation
    #[arg(short = 'y', long = "yes", global = true)]
    pub yes: bool,

    /// Only errors and the summary
    #[arg(short = 'q', long = "quiet", global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Print every file and stream
    #[arg(short = 'v', long = "verbose", global = true)]
    pub verbose: bool,

    /// Allow protected targets (system folders, drive roots)
    #[arg(long = "force", global = true)]
    pub force: bool,

    /// Save a JSON report to this file
    #[arg(long = "log", global = true, value_name = "FILE")]
    pub log: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Wipe one or more files
    File {
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
    },
    /// Wipe directories recursively
    Dir {
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Only files matching these masks, e.g. "*.tmp;*.log"
        #[arg(long = "include", value_name = "MASKS")]
        include: Option<String>,
        /// Skip files and sub-folders matching these masks
        #[arg(long = "exclude", value_name = "MASKS")]
        exclude: Option<String>,
        /// Keep the folder itself, wipe only what is inside
        #[arg(long = "contents-only")]
        contents_only: bool,
    },
    /// Move files or folders to another folder, then wipe the originals
    Move {
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Destination folder
        #[arg(long = "to", value_name = "DIR", required = true)]
        to: PathBuf,
    },
    /// Wipe the free space of a volume
    Freespace {
        #[arg(required = true, value_name = "VOLUME")]
        volumes: Vec<PathBuf>,
        /// Leave this many MiB unfilled
        #[arg(long = "reserve", value_name = "MIB", default_value_t = 0)]
        reserve_mib: u64,
        /// Also overwrite the slack at the end of every file (cluster tips)
        #[arg(long = "cluster-tips")]
        cluster_tips: bool,
        /// NTFS: also overwrite free file records (old names, small files)
        #[arg(long = "file-records")]
        file_records: bool,
    },
    /// Wipe the contents of the Recycle Bin
    #[command(name = "recycle-bin")]
    RecycleBin,
    /// List all wiping methods
    Methods,
    /// List the installed interface languages (language packs)
    Languages,
    /// Write or remove the machine-wide password lock (used by the GUI, elevated)
    #[command(name = "lock-machine", hide = true)]
    LockMachine {
        #[arg(long = "config", value_name = "FILE")]
        config: Option<PathBuf>,
        #[arg(long = "clear")]
        clear: bool,
    },
    /// Explorer context-menu integration
    Shell {
        #[command(subcommand)]
        action: ShellAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ShellAction {
    /// Add freewipe to the Explorer context menu
    Install(ShellOpts),
    /// Remove freewipe from the Explorer context menu
    Uninstall {
        /// All users (HKLM, needs administrator rights)
        #[arg(long = "all-users")]
        all_users: bool,
    },
    /// Show what is registered
    Status {
        /// All users (HKLM, needs administrator rights)
        #[arg(long = "all-users")]
        all_users: bool,
    },
    /// Print the registry entries as a .reg file
    Export {
        #[command(flatten)]
        opts: ShellOpts,
        /// Write to this file instead of stdout
        #[arg(short = 'o', long = "output", value_name = "FILE")]
        output: Option<PathBuf>,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ShellOpts {
    /// All users (HKLM, needs administrator rights)
    #[arg(long = "all-users")]
    pub all_users: bool,
    /// Do not add to files
    #[arg(long = "no-files")]
    pub no_files: bool,
    /// Do not add to folders
    #[arg(long = "no-dirs")]
    pub no_dirs: bool,
    /// Do not add "wipe free space" to drives
    #[arg(long = "no-drives")]
    pub no_drives: bool,
    /// Add "wipe Recycle Bin"
    #[arg(long = "recycle-bin")]
    pub recycle_bin: bool,
    /// Top-level items instead of a "freewipe" submenu
    #[arg(long = "flat")]
    pub flat: bool,
    /// Show only with Shift held
    #[arg(long = "shift-only")]
    pub shift_only: bool,
    /// No icon
    #[arg(long = "no-icon")]
    pub no_icon: bool,
    /// Omit the "wipe with method" submenu
    #[arg(long = "no-method-menu")]
    pub no_method_menu: bool,
    /// Omit "add to queue"
    #[arg(long = "no-queue")]
    pub no_queue: bool,
    /// Omit "wipe permanently"
    #[arg(long = "no-wipe")]
    pub no_wipe: bool,
    /// Path to freewipe-gui.exe (default: next to freewipe.exe)
    #[arg(long = "gui", value_name = "EXE")]
    pub gui: Option<PathBuf>,
    /// Read the whole configuration from a JSON file (used by the GUI)
    #[arg(long = "config", value_name = "FILE", hide = true)]
    pub config: Option<PathBuf>,
}

/// The clap command with every help text translated.
pub fn localized_command() -> clap::Command {
    localize(Cli::command().about(tr!("help.about")).after_help(tr!("help.after")), "", true)
}

fn help_text(id: &str) -> Option<String> {
    let key = format!("help.{id}");
    let s = freewipe::i18n::t(&key);
    (s != key).then_some(s)
}

/// Translate one command level and recurse into its subcommands.
fn localize(mut c: clap::Command, path: &str, root: bool) -> clap::Command {
    use clap::{Arg, ArgAction};

    c = c
        .help_template(tr!("help.template"))
        .subcommand_help_heading(tr!("help.h_commands"))
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(tr!("help.help"))
                .help_heading(tr!("help.h_options")),
        );
    if root {
        c = c.disable_version_flag(true).arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(tr!("help.version"))
                .help_heading(tr!("help.h_options")),
        );
    }

    let ids: Vec<(String, bool)> = c.get_arguments().map(|a| (a.get_id().to_string(), a.is_positional())).collect();
    for (id, positional) in ids {
        if id == "help" || id == "version" {
            continue;
        }
        let text = help_text(&format!("{path}{id}")).or_else(|| help_text(&id));
        let heading = if positional { tr!("help.h_args") } else { tr!("help.h_options") };
        c = c.mut_arg(id, |mut a| {
            if let Some(t) = text {
                a = a.help(t);
            }
            a.help_heading(heading)
        });
    }

    let subs: Vec<String> = c.get_subcommands().map(|s| s.get_name().to_string()).collect();
    for name in subs {
        let full = format!("{path}{name}");
        let about = help_text(&format!("cmd.{full}"));
        c = c.mut_subcommand(&name, |mut s| {
            if let Some(a) = about {
                s = s.about(a);
            }
            localize(s, &format!("{full}."), false)
        });
    }
    c
}

/// Find `--lang`/`-l` in raw args before clap runs, so help is translated.
pub fn prescan_lang(args: &[std::ffi::OsString]) -> Option<String> {
    let mut it = args.iter().map(|a| a.to_string_lossy().to_string());
    while let Some(a) = it.next() {
        if a == "--lang" || a == "-l" {
            return it.next();
        }
        if let Some(v) = a.strip_prefix("--lang=") {
            return Some(v.to_string());
        }
        if let Some(v) = a.strip_prefix("-l").filter(|v| !v.is_empty() && !a.starts_with("--")) {
            return Some(v.trim_start_matches('=').to_string());
        }
    }
    None
}
