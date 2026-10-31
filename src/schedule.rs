//! Scheduled wipes, run by the Windows Task Scheduler.
//!
//! A schedule is stored in the settings; the scheduler only knows a task
//! `\freewipe\freewipe-<id>` that starts `freewipe-gui.exe --run-schedule <id>`.
//! The task is created from an XML definition (via `schtasks /Create /XML`)
//! so it can say what the command line cannot: no 72-hour time limit, run
//! as soon as possible after a missed start, also on battery.
//!
//! Tasks run as the current user, only while they are signed in, with
//! normal rights — no stored password and no elevation.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::plan::{DirFilter, Target};

/// What a schedule wipes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ScheduleTarget {
    FreeSpace {
        volume: PathBuf,
        #[serde(default)]
        cluster_tips: bool,
        #[serde(default)]
        file_records: bool,
    },
    /// Files in a folder (e.g. Downloads, Temp), normally with masks and
    /// "contents only".
    Folder {
        path: PathBuf,
        #[serde(default)]
        filter: DirFilter,
    },
    RecycleBin,
}

/// When it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "every")]
pub enum Recurrence {
    Daily,
    /// `day`: 1 = Monday … 7 = Sunday.
    Weekly {
        day: u8,
    },
    /// `day`: 1–31 (months without that day are skipped by Windows).
    Monthly {
        day: u8,
    },
    /// A few minutes after the user signs in.
    AtLogon,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// 8 hex digits; names the task.
    pub id: String,
    pub name: String,
    pub target: ScheduleTarget,
    pub when: Recurrence,
    /// `HH:MM` (ignored for `AtLogon`).
    pub time: String,
    /// Method key (`dod-e`, `custom-…`).
    pub method: String,
    /// Show the small progress window (otherwise only the tray icon and a
    /// notification at the end).
    #[serde(default)]
    pub show_window: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Schedule {
    pub fn new_id() -> String {
        use rand::RngCore;
        format!("{:08x}", rand::rngs::OsRng.next_u32())
    }

    /// Task path in the Task Scheduler library.
    pub fn task_name(&self) -> String {
        format!(r"\freewipe\freewipe-{}", self.id)
    }

    /// What the run wipes.
    pub fn targets(&self) -> Vec<Target> {
        match &self.target {
            ScheduleTarget::FreeSpace { volume, .. } => vec![Target::free_space(volume)],
            ScheduleTarget::Folder { path, filter } => vec![Target::dir_filtered(path, filter.clone())],
            ScheduleTarget::RecycleBin => vec![Target::recycle_bin()],
        }
    }

    /// `(hour, minute)` from `time`, defaulting to 03:00.
    pub fn hm(&self) -> (u8, u8) {
        let mut it = self.time.split(':').map(|x| x.trim().parse::<u8>().ok());
        match (it.next().flatten(), it.next().flatten()) {
            (Some(h), Some(m)) if h < 24 && m < 60 => (h, m),
            _ => (3, 0),
        }
    }

    /// The Task Scheduler definition (Task Scheduler 1.2 schema).
    pub fn task_xml(&self, gui_exe: &std::path::Path, user: &str) -> String {
        let (h, m) = self.hm();
        let start = format!("2026-01-01T{h:02}:{m:02}:00");
        let trigger = match self.when {
            Recurrence::Daily => format!(
                "<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>\
                 <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger>"
            ),
            Recurrence::Weekly { day } => format!(
                "<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>\
                 <ScheduleByWeek><DaysOfWeek><{d} /></DaysOfWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek>\
                 </CalendarTrigger>",
                d = DAYS[(day.clamp(1, 7) - 1) as usize]
            ),
            Recurrence::Monthly { day } => format!(
                "<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>\
                 <ScheduleByMonth><DaysOfMonth><Day>{}</Day></DaysOfMonth><Months>{}</Months></ScheduleByMonth>\
                 </CalendarTrigger>",
                day.clamp(1, 31),
                MONTHS.iter().map(|m| format!("<{m} />")).collect::<String>()
            ),
            Recurrence::AtLogon => format!(
                "<LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId><Delay>PT3M</Delay></LogonTrigger>",
                xml_escape(user)
            ),
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>freewipe</Author>
    <Description>{desc}</Description>
    <URI>{uri}</URI>
  </RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>{enabled}</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--run-schedule {id}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
            desc = xml_escape(&format!("{}: {}", crate::app::name(), self.name)),
            uri = xml_escape(&self.task_name()),
            user = xml_escape(user),
            enabled = self.enabled,
            exe = xml_escape(&gui_exe.display().to_string()),
            id = xml_escape(&self.id),
        )
    }
}

/// State reported by the Task Scheduler (texts as Windows formats them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskState {
    pub exists: bool,
    pub next_run: String,
    pub status: String,
    pub last_run: String,
    /// Raw "Last Result" (`0` = success, `267011` = never ran).
    pub last_result: String,
}

/// Parse one CSV line (quoted fields, `""` escapes).
pub fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// From `schtasks /Query /FO CSV /NH /V` output: columns are fixed
/// (HostName, TaskName, Next Run Time, Status, Logon Mode, Last Run Time,
/// Last Result, …) while the header texts are localised, so go by index.
pub fn parse_query(out: &str) -> TaskState {
    let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with('"')) else { return TaskState::default() };
    let f = parse_csv_line(line);
    let get = |i: usize| f.get(i).cloned().unwrap_or_default();
    TaskState { exists: true, next_run: get(2), status: get(3), last_run: get(5), last_result: get(6) }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn schtasks(args: &[&str]) -> io::Result<String> {
        let out = Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW).output();
        let out = out.inspect_err(|e| crate::dlog!("schtasks {args:?}: cannot run: {e}"))?;
        let text = crate::sys::oem_to_string(&out.stdout);
        if out.status.success() {
            if args.first() != Some(&"/Query") {
                crate::dlog!("schtasks {args:?}: ok");
            }
            Ok(text)
        } else {
            let err = crate::sys::oem_to_string(&out.stderr).trim().to_string();
            let err = if err.is_empty() { text.trim().to_string() } else { err };
            crate::dlog!("schtasks {args:?}: failed ({}): {err}", out.status);
            Err(io::Error::other(err))
        }
    }

    fn current_user() -> String {
        let user = std::env::var("USERNAME").unwrap_or_default();
        match std::env::var("USERDOMAIN") {
            Ok(d) if !d.is_empty() => format!(r"{d}\{user}"),
            _ => user,
        }
    }

    /// Create or replace the task for `s`.
    pub fn register(s: &Schedule, gui_exe: &std::path::Path) -> io::Result<()> {
        let xml = s.task_xml(gui_exe, &current_user());
        let tmp = std::env::temp_dir().join(format!("freewipe-task-{}.xml", s.id));
        // schtasks expects UTF-16 with a byte-order mark.
        let mut bytes = vec![0xFF, 0xFE];
        for u in xml.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&tmp, bytes)?;
        let tmp_s = tmp.display().to_string();
        let r = schtasks(&["/Create", "/TN", &s.task_name(), "/XML", &tmp_s, "/F"]).map(|_| ());
        let _ = std::fs::remove_file(&tmp);
        r
    }

    pub fn unregister(s: &Schedule) -> io::Result<()> {
        schtasks(&["/Delete", "/TN", &s.task_name(), "/F"]).map(|_| ())
    }

    pub fn run_now(s: &Schedule) -> io::Result<()> {
        schtasks(&["/Run", "/TN", &s.task_name()]).map(|_| ())
    }

    pub fn state(s: &Schedule) -> TaskState {
        schtasks(&["/Query", "/TN", &s.task_name(), "/FO", "CSV", "/NH", "/V"])
            .map(|o| parse_query(&o))
            .unwrap_or_default()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, crate::tr!("shell.windows_only"))
    }
    pub fn register(_s: &Schedule, _gui_exe: &std::path::Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn unregister(_s: &Schedule) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn run_now(_s: &Schedule) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn state(_s: &Schedule) -> TaskState {
        TaskState::default()
    }
}

pub use imp::{register, run_now, state, unregister};

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(when: Recurrence) -> Schedule {
        Schedule {
            id: "0a1b2c3d".into(),
            name: "Downloads <weekly> & co".into(),
            target: ScheduleTarget::Folder {
                path: r"C:\Users\me\Downloads".into(),
                filter: DirFilter { include: vec!["*.tmp".into()], exclude: vec![], contents_only: true },
            },
            when,
            time: "21:05".into(),
            method: "dod-e".into(),
            show_window: false,
            enabled: true,
        }
    }

    #[test]
    fn xml_has_triggers_and_escapes() {
        let exe = std::path::Path::new(r"C:\Program Files\freewipe\freewipe-gui.exe");
        let x = sample(Recurrence::Weekly { day: 3 }).task_xml(exe, r"PC\me");
        assert!(x.contains("<Wednesday />"));
        assert!(x.contains("T21:05:00"));
        assert!(x.contains("&lt;weekly&gt; &amp; co"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<Arguments>--run-schedule 0a1b2c3d</Arguments>"));
        let x = sample(Recurrence::Monthly { day: 40 }).task_xml(exe, "me");
        assert!(x.contains("<Day>31</Day>") && x.contains("<December />"));
        let x = sample(Recurrence::AtLogon).task_xml(exe, "me");
        assert!(x.contains("<LogonTrigger>"));
        assert_eq!(sample(Recurrence::Daily).task_name(), r"\freewipe\freewipe-0a1b2c3d");
    }

    #[test]
    fn targets_and_time() {
        let s = sample(Recurrence::Daily);
        let t = s.targets();
        assert_eq!(t.len(), 1);
        assert!(t[0].filter.as_ref().unwrap().contents_only);
        assert_eq!(s.hm(), (21, 5));
        assert_eq!(Schedule { time: "x".into(), ..s }.hm(), (3, 0));
    }

    #[test]
    fn query_parsing() {
        let out = "\r\n\"PC\",\"\\freewipe\\freewipe-1\",\"30.10.2026 3:00:00\",\"Ready\",\"Interactive only\",\"N/A\",\"267011\",\"me\"\r\n";
        let s = parse_query(out);
        assert!(s.exists);
        assert_eq!(s.next_run, "30.10.2026 3:00:00");
        assert_eq!(s.last_result, "267011");
        assert_eq!(parse_csv_line(r#""a ""b""",c"#), vec![r#"a "b""#.to_string(), "c".to_string()]);
        assert!(!parse_query("").exists);
    }
}
