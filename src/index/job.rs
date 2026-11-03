//! Runs [`index_project`] on a background thread so the UI stays responsive.
//! The UI polls the job once per frame; the worker wakes the UI via
//! `request_repaint` whenever it has news.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use super::{index_project, IndexError, IndexOptions, IndexOutcome, Progress};

/// tree-sitter's parser recurses on some deeply nested expressions (e.g. long
/// cast chains); a stack overflow aborts the whole process and can't be
/// caught, so the worker gets a generous stack. It's virtual memory, only
/// committed as used.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

enum IndexEvent {
    Progress(Progress),
    Finished(Result<IndexOutcome, IndexError>),
}

pub struct IndexJob {
    root: PathBuf,
    events: Receiver<IndexEvent>,
    cancel: Arc<AtomicBool>,
    progress: Progress,
    is_finished: bool,
}

impl IndexJob {
    pub fn spawn(root: PathBuf, options: IndexOptions, ctx: egui::Context) -> Self {
        let (tx, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_root = root.clone();

        let spawned = std::thread::Builder::new()
            .name("indexer".into())
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || {
                // Send failures mean the UI dropped the job; nothing left to do.
                let mut on_progress = |p: Progress| {
                    let _ = tx.send(IndexEvent::Progress(p));
                    ctx.request_repaint();
                };
                // A panic must still produce a result and wake the UI, or it
                // would wait for the next input event to notice.
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    index_project(&worker_root, options, &mut on_progress, &worker_cancel)
                }))
                .unwrap_or_else(|payload| {
                    let reason = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    eprintln!("indexer panicked: {reason}");
                    Err(IndexError::WorkerCrashed)
                });
                let _ = tx.send(IndexEvent::Finished(result));
                ctx.request_repaint();
            });

        let job = Self {
            root,
            events,
            cancel,
            progress: Progress::default(),
            is_finished: false,
        };
        match spawned {
            Ok(_) => job,
            Err(err) => job.with_immediate_result(Err(IndexError::Spawn(err.to_string()))),
        }
    }

    /// A job whose (failed) result is already known, e.g. the thread didn't start.
    fn with_immediate_result(mut self, result: Result<IndexOutcome, IndexError>) -> Self {
        let (tx, events) = mpsc::channel();
        // The receiver is alive right here, so this send cannot fail.
        let _ = tx.send(IndexEvent::Finished(result));
        self.events = events;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn progress(&self) -> Progress {
        self.progress
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Drains pending events. Returns the final result once, when done.
    pub fn poll(&mut self) -> Option<Result<IndexOutcome, IndexError>> {
        if self.is_finished {
            return None;
        }
        loop {
            match self.events.try_recv() {
                Ok(IndexEvent::Progress(p)) => self.progress = p,
                Ok(IndexEvent::Finished(result)) => {
                    self.is_finished = true;
                    return Some(result);
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    // Worker exited without a result: it panicked.
                    self.is_finished = true;
                    return Some(Err(IndexError::WorkerCrashed));
                }
            }
        }
    }
}

/// A dropped job (e.g. replaced by a newer one) stops its worker at the next
/// cancellation check instead of running to completion in the background.
impl Drop for IndexJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(20);

    fn wait(job: &mut IndexJob) -> Result<IndexOutcome, IndexError> {
        let start = Instant::now();
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            assert!(start.elapsed() < TIMEOUT, "index job timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn runs_to_completion_in_background() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.cpp"), "void a() {}\nvoid b() { a(); }\n")
            .expect("write");

        let mut job = IndexJob::spawn(
            dir.path().to_path_buf(),
            IndexOptions::default(),
            egui::Context::default(),
        );
        let outcome = wait(&mut job).expect("index");
        assert_eq!(outcome.graph.symbols.len(), 2);
        assert_eq!(job.progress(), Progress { done: 1, total: 1 });
    }

    /// tree-sitter's parser recurses on long cast chains; on a default 2 MiB
    /// thread stack this aborted the whole process.
    #[test]
    fn deep_cast_chain_does_not_overflow_worker_stack() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = "(a)(b)(c)*".repeat(30_000);
        std::fs::write(
            dir.path().join("deep.c"),
            format!("void f(){{ {body} x; }}\n"),
        )
        .expect("write");
        let mut job = IndexJob::spawn(
            dir.path().to_path_buf(),
            IndexOptions::default(),
            egui::Context::default(),
        );
        let outcome = wait(&mut job).expect("index");
        assert!(outcome
            .graph
            .symbols
            .values()
            .any(|s| s.qualified_name == "f"));
    }

    #[test]
    fn reports_errors_from_worker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut job = IndexJob::spawn(
            dir.path().join("missing"),
            IndexOptions::default(),
            egui::Context::default(),
        );
        assert!(matches!(wait(&mut job), Err(IndexError::NotADirectory(_))));
    }

    #[test]
    fn poll_after_finish_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut job = IndexJob::spawn(
            dir.path().join("missing"),
            IndexOptions::default(),
            egui::Context::default(),
        );
        let _ = wait(&mut job);
        assert!(job.poll().is_none());
    }
}
