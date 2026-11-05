//! Native file dialogs, run off the UI code.
//!
//! A dialog opened from inside the UI code runs its own event loop while
//! the app's event handler is still busy; when macOS delivers a window
//! event to that handler meanwhile, winit panics inside an Objective-C
//! callback and the process aborts. So each dialog runs on a thread of its
//! own (rfd hands it to the main thread at a safe moment), and the app asks
//! for the answer every frame.

use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// A dialog that is open, or has just closed.
pub struct PendingDialog<T> {
    answer: Receiver<Option<T>>,
}

/// What a dialog has come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogState<T> {
    Open,
    Picked(T),
    Cancelled,
    /// The dialog's thread stopped without answering.
    Failed(String),
}

/// Runs `show` (which shows a blocking dialog and returns what was picked)
/// on its own thread, repainting `ctx` once it answers.
pub fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    show: impl FnOnce() -> Option<T> + Send + 'static,
) -> Result<PendingDialog<T>, String> {
    let (tx, answer) = channel();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            // The app may have stopped waiting; then nobody needs the answer.
            let _ = tx.send(show());
            ctx.request_repaint();
        })
        .map_err(|e| format!("Could not open the dialog: {e}"))?;
    Ok(PendingDialog { answer })
}

impl<T> PendingDialog<T> {
    /// Where the dialog is at. Drop it once it isn't open any more.
    pub fn poll(&self) -> DialogState<T> {
        match self.answer.try_recv() {
            Ok(Some(picked)) => DialogState::Picked(picked),
            Ok(None) => DialogState::Cancelled,
            Err(TryRecvError::Empty) => DialogState::Open,
            Err(TryRecvError::Disconnected) => {
                DialogState::Failed("the dialog closed without an answer".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    /// Polls `dialog` until it closes.
    fn outcome<T>(dialog: &PendingDialog<T>) -> DialogState<T> {
        let start = Instant::now();
        loop {
            match dialog.poll() {
                DialogState::Open if start.elapsed() < WAIT => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                state => return state,
            }
        }
    }

    #[test]
    fn a_pick_is_reported_once_the_dialog_answers() {
        let ctx = egui::Context::default();
        let dialog = spawn(&ctx, || Some("/project".to_string())).expect("spawn");
        assert_eq!(
            outcome(&dialog),
            DialogState::Picked("/project".to_string())
        );
    }

    #[test]
    fn cancelling_is_reported_as_such() {
        let ctx = egui::Context::default();
        let dialog = spawn(&ctx, || None::<String>).expect("spawn");
        assert_eq!(outcome(&dialog), DialogState::Cancelled);
    }

    #[test]
    fn the_dialog_stays_open_until_it_answers() {
        let ctx = egui::Context::default();
        let (release, wait) = channel::<()>();
        let dialog = spawn(&ctx, move || {
            let _ = wait.recv();
            Some(1)
        })
        .expect("spawn");
        assert_eq!(dialog.poll(), DialogState::Open);
        release.send(()).expect("release");
        assert_eq!(outcome(&dialog), DialogState::Picked(1));
    }

    #[test]
    fn a_dialog_thread_that_dies_is_a_failure_not_a_hang() {
        let ctx = egui::Context::default();
        let dialog = spawn(&ctx, || -> Option<u8> { panic!("no dialog backend") }).expect("spawn");
        assert!(matches!(outcome(&dialog), DialogState::Failed(_)));
    }
}
