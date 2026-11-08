//! Reading a history in a separate process: the audited repository may be
//! crafted to exhaust memory or never finish (a symlink to `/dev/zero`, a
//! delta chain expanding to gigabytes, a file whose blame never ends), and
//! nothing inside gitoxide can be interrupted. So the app runs this same
//! program in a worker mode, hands it the request on stdin, reads the
//! answer from stdout, and kills it at a deadline or when the project
//! closes. A crash or hang there costs only that read.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{file_history, read, FileHistory, History, Limits};

/// The argument that starts the worker mode.
pub const WORKER_ARG: &str = "--history-worker";
const PROJECT_MODE: &str = "project";
const FILE_MODE: &str = "file";
/// A request or an answer larger than this is refused.
const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;
/// How long the app waits for a project's history, past the worker's own
/// budget, before killing it.
const PROJECT_GRACE: Duration = Duration::from_secs(30);
/// How long the app waits for one file's history.
const FILE_DEADLINE: Duration = Duration::from_secs(60);
/// A worker whose app is gone exits after this, whatever it is doing.
const WORKER_LIFETIME: Duration = Duration::from_secs(300);
/// How often the app checks on a running worker.
const POLL: Duration = Duration::from_millis(50);

/// The history of the checkout at `root` for its `files`, read by a worker
/// process; stopped (killed) if `stop` is set.
pub fn read_in_worker(
    root: &Path,
    files: Vec<String>,
    stop: &AtomicBool,
) -> Result<History, String> {
    let deadline = Limits::default().budget + PROJECT_GRACE;
    ask_worker(PROJECT_MODE, root, &files, deadline, stop)
}

/// Who last changed each line of `file`, read by a worker process.
pub fn file_history_in_worker(
    root: &Path,
    file: &str,
    stop: &AtomicBool,
) -> Result<FileHistory, String> {
    ask_worker(FILE_MODE, root, &file, FILE_DEADLINE, stop)
}

fn ask_worker<Q: Serialize, A: DeserializeOwned>(
    mode: &str,
    root: &Path,
    request: &Q,
    deadline: Duration,
    stop: &AtomicBool,
) -> Result<A, String> {
    let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let output = exchange(mode, root, &input, deadline, stop)?;
    let answer: Result<A, String> = serde_json::from_slice(&output)
        .map_err(|_| "The history reader gave no usable answer.".to_string())?;
    answer
}

/// Sends `input` to a worker for `mode` and `root`; its answer.
#[cfg(not(test))]
fn exchange(
    mode: &str,
    root: &Path,
    input: &[u8],
    deadline: Duration,
    stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let program = std::env::current_exe()
        .map_err(|e| format!("Could not find SrcWranglr's own program: {e}"))?;
    let args = [
        OsString::from(WORKER_ARG),
        mode.into(),
        root.as_os_str().to_owned(),
    ];
    run_process(&program, &args, input, deadline, stop)
}

/// Tests answer in-process: their program is the test runner, which has
/// no worker mode. (`run_process` is tested on its own.)
#[cfg(test)]
fn exchange(
    mode: &str,
    root: &Path,
    input: &[u8],
    _deadline: Duration,
    _stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    Ok(answer(mode, root, input))
}

/// Runs `program` with `args`, `input` on its stdin; its stdout once it
/// exits. Killed if it outlives `deadline` or `stop` is set.
pub fn run_process(
    program: &Path,
    args: &[OsString],
    input: &[u8],
    deadline: Duration,
    stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the history reader: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("No pipe to the history reader")?;
    let input = input.to_vec();
    // Written and read on threads of their own, so neither pipe can stall
    // the other.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child
        .stdout
        .take()
        .ok_or("No pipe from the history reader")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout
            .by_ref()
            .take(MAX_MESSAGE_BYTES)
            .read_to_end(&mut out);
        out
    });
    loop {
        if stop.load(Ordering::Relaxed) || started.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            let _ = reader.join();
            return Err(match stop.load(Ordering::Relaxed) {
                true => "Stopped.".into(),
                false => "Reading the git history took too long and was stopped.".into(),
            });
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(format!("Lost the history reader: {e}")),
        }
    }
    let _ = writer.join();
    reader
        .join()
        .map_err(|_| "Could not read the history reader's answer.".to_string())
}

/// The worker mode's main: `<mode> <root>` from `args`, the request on
/// stdin, the answer on stdout. Returns the exit code.
pub fn worker_main(mut args: impl Iterator<Item = OsString>) -> i32 {
    // Exit on our own if the app that started us is gone and never kills us.
    std::thread::spawn(|| {
        std::thread::sleep(WORKER_LIFETIME);
        std::process::exit(3);
    });
    let (Some(mode), Some(root)) = (args.next(), args.next()) else {
        return 2;
    };
    let mut input = Vec::new();
    if std::io::stdin()
        .take(MAX_MESSAGE_BYTES)
        .read_to_end(&mut input)
        .is_err()
    {
        return 2;
    }
    let answer = answer(&mode.to_string_lossy(), Path::new(&root), &input);
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(&answer).and_then(|()| stdout.flush()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The JSON answer to a `mode` request about `root`.
pub fn answer(mode: &str, root: &Path, input: &[u8]) -> Vec<u8> {
    let encoded = match mode {
        PROJECT_MODE => serde_json::from_slice::<Vec<String>>(input)
            .map_err(|e| e.to_string())
            .and_then(|files| {
                let files = files.into_iter().collect();
                read(root, &files, Limits::default(), &AtomicBool::new(false))
            })
            .map_or_else(
                |e| serde_json::to_vec(&Err::<History, _>(e)),
                |h| serde_json::to_vec(&Ok::<_, String>(h)),
            ),
        FILE_MODE => serde_json::from_slice::<String>(input)
            .map_err(|e| e.to_string())
            .and_then(|file| file_history(root, &file, Limits::default().max_file_bytes))
            .map_or_else(
                |e| serde_json::to_vec(&Err::<FileHistory, _>(e)),
                |h| serde_json::to_vec(&Ok::<_, String>(h)),
            ),
        _ => serde_json::to_vec(&Err::<(), _>(format!("unknown mode {mode:?}"))),
    };
    encoded.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::test_repo::sample;

    #[cfg(unix)]
    #[test]
    fn a_process_past_its_deadline_is_killed() {
        let started = Instant::now();
        let args = ["-c".into(), "sleep 30".into()];
        let error = run_process(
            Path::new("/bin/sh"),
            &args,
            b"",
            Duration::from_millis(200),
            &AtomicBool::new(false),
        )
        .expect_err("killed");
        assert!(error.contains("too long"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_process_is_killed_and_an_answer_is_read() {
        let args = ["-c".into(), "sleep 30".into()];
        let stop = AtomicBool::new(true);
        let error = run_process(
            Path::new("/bin/sh"),
            &args,
            b"",
            Duration::from_secs(60),
            &stop,
        )
        .expect_err("stopped");
        assert_eq!(error, "Stopped.");
        let echo = ["-c".into(), "cat".into()];
        let out = run_process(
            Path::new("/bin/sh"),
            &echo,
            b"[1,2]",
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .expect("ran");
        assert_eq!(out, b"[1,2]");
    }

    #[test]
    fn the_worker_answers_both_requests_as_json() {
        let Some(repo) = sample() else {
            return;
        };
        let files = serde_json::to_vec(&["a.c", "b.c"]).expect("json");
        let history: Result<History, String> =
            serde_json::from_slice(&answer(PROJECT_MODE, repo.path(), &files)).expect("json");
        assert_eq!(history.expect("history").fix_commits.len(), 2);
        let file = serde_json::to_vec("a.c").expect("json");
        let lines: Result<FileHistory, String> =
            serde_json::from_slice(&answer(FILE_MODE, repo.path(), &file)).expect("json");
        assert_eq!(lines.expect("file").commits.len(), 2);
        let bad: Result<(), String> =
            serde_json::from_slice(&answer("nope", repo.path(), b"")).expect("json");
        assert!(bad.is_err());
    }
}
