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
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{file_history, read, FileHistory, History};
#[cfg(not(test))]
use crate::process::{run_process, Limits, ProcessError};

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

/// The history of the checkout at `root` for its `files`, read by a worker
/// process; stopped (killed) if `stop` is set.
pub fn read_in_worker(
    root: &Path,
    files: Vec<String>,
    stop: &AtomicBool,
) -> Result<History, String> {
    let deadline = super::Limits::default().budget + PROJECT_GRACE;
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
    let limits = Limits {
        deadline,
        max_output: MAX_MESSAGE_BYTES,
    };
    run_process(
        "the history reader",
        &program,
        &args,
        &[],
        input,
        limits,
        stop,
    )
    .map_err(|e| match e {
        ProcessError::TookTooLong => {
            "Reading the git history took too long and was stopped.".to_string()
        }
        other => other.to_string(),
    })
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
                read(
                    root,
                    &files,
                    super::Limits::default(),
                    &AtomicBool::new(false),
                )
            })
            .map_or_else(
                |e| serde_json::to_vec(&Err::<History, _>(e)),
                |h| serde_json::to_vec(&Ok::<_, String>(h)),
            ),
        FILE_MODE => serde_json::from_slice::<String>(input)
            .map_err(|e| e.to_string())
            .and_then(|file| file_history(root, &file, super::Limits::default().max_file_bytes))
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
