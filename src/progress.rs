//! Progress reporting and cancellation shared by the CLI and the GUI.
//!
//! The wipe engine never prints. It reports through a [`Progress`]
//! implementation (an `indicatif` bar in the CLI, a channel to the UI thread
//! in the GUI) and polls a [`CancelFlag`] between buffers, so Ctrl+C or the
//! GUI's Stop button take effect within one buffer write.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What the engine is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Overwrite,
    Verify,
    Rename,
    Truncate,
    Delete,
    /// Free-space: creating fill files.
    Fill,
    /// Free-space: removing fill files.
    Cleanup,
    /// Free-space: overwriting the slack at the end of existing files.
    Tips,
    /// Free-space: overwriting free NTFS file records.
    Residue,
    /// Move: copying and checking the copy.
    Copy,
}

impl Stage {
    pub fn label(&self) -> String {
        match self {
            Stage::Overwrite => crate::tr!("stage.overwrite"),
            Stage::Verify => crate::tr!("stage.verify"),
            Stage::Rename => crate::tr!("stage.rename"),
            Stage::Truncate => crate::tr!("stage.truncate"),
            Stage::Delete => crate::tr!("stage.delete"),
            Stage::Fill => crate::tr!("stage.fill"),
            Stage::Cleanup => crate::tr!("stage.cleanup"),
            Stage::Tips => crate::tr!("stage.tips"),
            Stage::Residue => crate::tr!("stage.residue"),
            Stage::Copy => crate::tr!("stage.copy"),
        }
    }
}

/// Receiver of engine progress. Every method has a no-op default.
pub trait Progress {
    /// A new file / stream / fill file starts.
    fn item(&mut self, _name: &str) {}
    /// Pass `index` (0-based) of `count` starts, with a short label.
    fn pass(&mut self, _index: usize, _count: usize, _label: &str) {}
    /// Engine stage changed.
    fn stage(&mut self, _stage: Stage) {}
    /// `bytes` more were written (or read back when verifying).
    fn advance(&mut self, _bytes: u64) {}
    /// A one-line note for the log (already localised).
    fn note(&mut self, _msg: &str) {}
}

/// Discards everything.
pub struct NoProgress;
impl Progress for NoProgress {}

/// Cheap clonable cancel + pause flag, shared between the engine and the UI.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<Flags>);

#[derive(Debug, Default)]
struct Flags {
    cancel: AtomicBool,
    pause: AtomicBool,
}

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::SeqCst);
    }
    pub fn reset(&self) {
        self.0.cancel.store(false, Ordering::SeqCst);
        self.0.pause.store(false, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::SeqCst)
    }
    pub fn set_paused(&self, paused: bool) {
        self.0.pause.store(paused, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.0.pause.load(Ordering::SeqCst)
    }
    /// Called by the engine between buffers: blocks while paused, then
    /// returns `true` if the work should stop.
    pub fn checkpoint(&self) -> bool {
        while self.is_paused() && !self.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.is_cancelled()
    }
}

/// Everything the engine needs besides the target and the options.
pub struct Ctx<'a> {
    pub cancel: &'a CancelFlag,
    pub progress: &'a mut dyn Progress,
}

impl<'a> Ctx<'a> {
    pub fn new(cancel: &'a CancelFlag, progress: &'a mut dyn Progress) -> Self {
        Ctx { cancel, progress }
    }
}
