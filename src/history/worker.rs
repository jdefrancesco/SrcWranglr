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

use super::changes::{changes, refs, ChangedFile, Changes, RangeCommit, Refs};
use super::{file_history, read, FileHistory, History};
#[cfg(not(test))]
use crate::process::{run_process, Limits, ProcessError};

/// The argument that starts the worker mode.
pub const WORKER_ARG: &str = "--history-worker";
const PROJECT_MODE: &str = "project";
const FILE_MODE: &str = "file";
const CHANGES_MODE: &str = "changes";
const REFS_MODE: &str = "refs";
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
    let history: History = ask_worker(PROJECT_MODE, root, &files, deadline, stop)?;
    Ok(bounded(history))
}

/// Files listed per fix commit, and characters per path, at most: the
/// worker's answer is read from another process.
const MAX_FILES_PER_COMMIT: usize = 200;
const MAX_PATH_CHARS: usize = 1_000;

/// `history` with each fix commit's file list cut to size.
fn bounded(history: History) -> History {
    let fix_commits = history
        .fix_commits
        .into_iter()
        .map(|commit| super::FixCommit {
            files: commit
                .files
                .into_iter()
                .take(MAX_FILES_PER_COMMIT)
                .map(|file| file.chars().take(MAX_PATH_CHARS).collect())
                .collect(),
            ..commit
        })
        .collect();
    History {
        fix_commits,
        ..history
    }
}

/// What a changes request asks: since `base`, for the project's `files`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangesRequest {
    pub base: String,
    pub files: Vec<String>,
}

/// What changed in the checkout at `root` since `base`, for the project's
/// `files`, read by a worker process.
pub fn changes_in_worker(
    root: &Path,
    base: &str,
    files: Vec<String>,
    stop: &AtomicBool,
) -> Result<Changes, String> {
    let deadline = super::Limits::default().budget + PROJECT_GRACE;
    let request = ChangesRequest {
        base: base.to_string(),
        files,
    };
    let changes: Changes = ask_worker(CHANGES_MODE, root, &request, deadline, stop)?;
    Ok(bounded_changes(changes))
}

/// What a changes answer may hold at most: it is read from another
/// process, which read a hostile repository.
const MAX_CHANGED_FILES: usize = 20_000;
const MAX_HUNKS_PER_FILE: usize = 5_000;
const MAX_COMMITS_LISTED: usize = 1_000;
const MAX_FILES_LISTED: usize = 2_000;
const MAX_BASE_CHARS: usize = 256;
const MAX_SHORT_CHARS: usize = 40;

/// `changes` with every list and text cut to size. (Line numbers are
/// kept inside each file where the app reads them.)
fn bounded_changes(changes: Changes) -> Changes {
    let text = |t: String, max: usize| -> String { t.chars().take(max).collect() };
    let path = |p: String| text(p, MAX_PATH_CHARS);
    let paths = |list: Vec<String>| list.into_iter().take(MAX_FILES_LISTED).map(path).collect();
    let commits = changes
        .commits
        .into_iter()
        .take(MAX_COMMITS_LISTED)
        .map(|c| RangeCommit {
            short: text(c.short, MAX_SHORT_CHARS),
            summary: text(c.summary, super::read::MAX_SUMMARY_CHARS + 1),
            author: text(c.author, super::read::MAX_AUTHOR_CHARS + 1),
            ..c
        })
        .collect();
    let files = changes
        .files
        .into_iter()
        .take(MAX_CHANGED_FILES)
        .map(|f| ChangedFile {
            file: path(f.file),
            renamed_from: f.renamed_from.map(path),
            hunks: f.hunks.into_iter().take(MAX_HUNKS_PER_FILE).collect(),
            ..f
        })
        .collect();
    Changes {
        base: text(changes.base, MAX_BASE_CHARS),
        base_short: text(changes.base_short, MAX_SHORT_CHARS),
        head: text(changes.head, MAX_SHORT_CHARS),
        commits,
        files,
        deleted: paths(changes.deleted),
        files_changed_on_disk: paths(changes.files_changed_on_disk),
        ..changes
    }
}

/// The tags and branches of the checkout at `root`, read by a worker
/// process.
pub fn refs_in_worker(root: &Path, stop: &AtomicBool) -> Result<Refs, String> {
    ask_worker(REFS_MODE, root, &(), FILE_DEADLINE, stop)
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
        CHANGES_MODE => serde_json::from_slice::<ChangesRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|request| {
                let limits = super::Limits::default();
                changes(root, &request.base, &request.files, limits)
            })
            .map_or_else(
                |e| serde_json::to_vec(&Err::<Changes, _>(e)),
                |c| serde_json::to_vec(&Ok::<_, String>(c)),
            ),
        REFS_MODE => refs(root).map_or_else(
            |e| serde_json::to_vec(&Err::<Refs, _>(e)),
            |r| serde_json::to_vec(&Ok::<_, String>(r)),
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

    #[test]
    fn an_answer_about_changes_is_bounded_again() {
        use crate::history::changes::{ChangedFile, Hunk, RangeCommit};
        let long = "x".repeat(5_000);
        let changes = Changes {
            base: long.clone(),
            commits: vec![
                RangeCommit {
                    short: long.clone(),
                    summary: long.clone(),
                    author: long.clone(),
                    time: 0,
                };
                3_000
            ],
            files: vec![ChangedFile {
                file: long.clone(),
                renamed_from: Some(long.clone()),
                is_new: false,
                hunks: vec![
                    Hunk {
                        first_line: 1,
                        line_count: 1,
                        removed: 0
                    };
                    20_000
                ],
                added: 0,
                removed: 0,
            }],
            deleted: vec![long.clone(); 5_000],
            files_changed_on_disk: vec![long; 5_000],
            ..Changes::default()
        };
        let bounded = bounded_changes(changes);
        assert!(bounded.base.chars().count() <= 256);
        assert!(bounded.commits.len() <= 1_000);
        assert!(bounded.commits[0].summary.chars().count() <= 201);
        assert!(bounded.files[0].file.chars().count() <= 1_000);
        assert!(bounded.files[0].hunks.len() <= 5_000);
        assert!(bounded.deleted.len() <= 2_000);
        assert!(bounded.files_changed_on_disk.len() <= 2_000);
    }

    #[test]
    fn the_worker_answers_changes_and_refs_requests() {
        let Some(repo) = sample() else {
            return;
        };
        assert!(crate::history::test_repo::git(
            repo.path(),
            &["tag", "v0", "HEAD~2"],
            5
        ));
        let request = ChangesRequest {
            base: "v0".into(),
            files: vec!["a.c".into(), "b.c".into()],
        };
        let input = serde_json::to_vec(&request).expect("json");
        let changes: Result<Changes, String> =
            serde_json::from_slice(&answer(CHANGES_MODE, repo.path(), &input)).expect("json");
        let changes = changes.expect("changes");
        assert_eq!(changes.commit_count, 2);
        assert_eq!(changes.files.len(), 1, "b.c: {changes:?}");
        let refs: Result<Refs, String> =
            serde_json::from_slice(&answer(REFS_MODE, repo.path(), b"null")).expect("json");
        assert_eq!(refs.expect("refs").tags, ["v0"]);
    }
}
