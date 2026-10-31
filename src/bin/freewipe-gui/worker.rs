//! Background jobs: the wipe runs on its own thread and streams progress to
//! the UI thread over a channel.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;

use freewipe::plan::{self, PlanItem, Target};
use freewipe::progress::{CancelFlag, Ctx, Progress, Stage};
use freewipe::report::ReportItem;
use freewipe::wipe::{self, WipeOptions};

pub enum Msg {
    Started(usize),
    Item(String),
    Pass(usize, usize, String),
    Stage(Stage),
    Advance(u64),
    Note(String),
    Done(usize, Box<ReportItem>),
    Finished,
}

pub struct Job {
    pub rx: Receiver<Msg>,
    pub cancel: CancelFlag,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct ChanProgress {
    tx: Sender<Msg>,
    ctx: egui::Context,
    last: Instant,
}

impl ChanProgress {
    fn send(&mut self, m: Msg, force: bool) {
        let _ = self.tx.send(m);
        if force || self.last.elapsed() > Duration::from_millis(40) {
            self.last = Instant::now();
            self.ctx.request_repaint();
        }
    }
}

impl Progress for ChanProgress {
    fn item(&mut self, name: &str) {
        self.send(Msg::Item(name.to_string()), false);
    }
    fn pass(&mut self, i: usize, n: usize, label: &str) {
        self.send(Msg::Pass(i, n, label.to_string()), false);
    }
    fn stage(&mut self, s: Stage) {
        self.send(Msg::Stage(s), false);
    }
    fn advance(&mut self, b: u64) {
        self.send(Msg::Advance(b), false);
    }
    fn note(&mut self, m: &str) {
        self.send(Msg::Note(m.to_string()), false);
    }
}

/// Wipe `targets` in order on a worker thread.
pub fn start(targets: Vec<Target>, opts: WipeOptions, keep_awake: bool, ctx: egui::Context) -> Job {
    let (tx, rx) = channel();
    let cancel = CancelFlag::new();
    let c2 = cancel.clone();
    let thread = std::thread::Builder::new()
        .name("freewipe-worker".into())
        .spawn(move || {
            #[cfg(windows)]
            if keep_awake {
                freewipe::sys::keep_awake(true);
            }
            #[cfg(not(windows))]
            let _ = keep_awake;
            let mut prog = ChanProgress { tx: tx.clone(), ctx: ctx.clone(), last: Instant::now() };
            for (i, t) in targets.iter().enumerate() {
                if c2.is_cancelled() {
                    break;
                }
                prog.send(Msg::Started(i), true);
                let item = wipe::execute(t, &opts, &mut Ctx::new(&c2, &mut prog));
                prog.send(Msg::Done(i, Box::new(item)), true);
            }
            #[cfg(windows)]
            freewipe::sys::keep_awake(false);
            let _ = tx.send(Msg::Finished);
            ctx.request_repaint();
        })
        .ok();
    Job { rx, cancel, thread }
}

/// Scan targets off the UI thread (folders can take a while).
pub fn scan(targets: Vec<Target>, force: bool, ctx: egui::Context) -> Receiver<PlanItem> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for t in targets {
            let _ = tx.send(plan::scan(&t, force));
            ctx.request_repaint();
        }
    });
    rx
}
