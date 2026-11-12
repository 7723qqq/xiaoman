//! The cooperative stop signal of the overlay's Stop button and the debug bridge.

use std::sync::atomic::{AtomicBool, Ordering};

use xiaoman_api::StopSignal;

/// A thread-safe [`StopSignal`]; every clone-free `&self` call goes through one atomic flag, so the
/// operator loop, the UI thread and the JNI thread can share it behind an `Arc`.
#[derive(Debug, Default)]
pub struct MutableStopSignal {
    flag: AtomicBool,
}

impl MutableStopSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the running task to stop before its next step.
    pub fn stop(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Re-arms the signal for the next task.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

impl StopSignal for MutableStopSignal {
    fn stopped(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}
