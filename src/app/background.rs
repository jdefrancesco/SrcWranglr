//! Work done off the window's thread, for a pane or window to poll.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

/// Work on its own thread, for a pane to poll; dropping it asks the work
/// to stop.
pub struct BackgroundJob<T> {
    result: Receiver<Result<T, String>>,
    cancel: Arc<AtomicBool>,
}

impl<T: Send + 'static> BackgroundJob<T> {
    /// Runs `work` (given a flag saying to stop) on its own thread; a panic
    /// in it is reported as an error. `what` names it in errors.
    pub fn spawn(
        what: &'static str,
        work: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static,
        ctx: egui::Context,
    ) -> Self {
        let (tx, result) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| work(&stop)))
                .unwrap_or_else(|_| {
                    Err(format!(
                        "{what} failed unexpectedly; the rest of the app is fine."
                    ))
                });
            // The app may have moved on by now; nothing to report to then.
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
        Self { result, cancel }
    }

    /// The result (or why there is none) once the work is done.
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The work stopped unexpectedly.".into())),
        }
    }
}

impl<T> Drop for BackgroundJob<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
