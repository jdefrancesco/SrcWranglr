//! Reading the history with gitoxide: walking commits from the checked-out
//! one, keeping those whose message marks a fix with the files they
//! changed, then tracing each such file's lines back to the commit that
//! last changed them (blame).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use gix::bstr::{BStr, ByteSlice};
use gix::hash::ObjectId;
use gix::revision::walk::Sorting;

use super::keywords::keywords_in;
use super::{FixCommit, History, Limits, OwnedLines};
use crate::safe_fs;

/// Hex digits in a short commit id.
const SHORT_ID: usize = 8;
pub(super) const MAX_SUMMARY_CHARS: usize = 200;
pub(super) const MAX_AUTHOR_CHARS: usize = 100;
/// Object cache for the walk and blame, in bytes.
pub(super) const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Threads tracing files' lines at once, at most.
const MAX_WORKERS: usize = 6;

pub const NOT_A_REPOSITORY: &str = "This project isn't a git checkout (no .git folder at its top).";

/// The history of the checkout at `root`, for the project's `files`
/// (project-relative), within `limits`; stopped early if `cancel` is set.
pub fn read(
    root: &Path,
    files: &HashSet<String>,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<History, String> {
    let started = Instant::now();
    let is_over = || cancel.load(Ordering::Relaxed) || started.elapsed() > limits.budget;
    let mut repo = open(root)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    let head = repo
        .head_id()
        .map_err(|_| "This repository has no commits yet.".to_string())?
        .detach();
    let mut walked = walk(&repo, head, files, limits, &is_over)?;
    let mut history = History {
        head: short(head),
        commits_read: walked.commits_read,
        is_walk_cut_short: walked.is_cut_short,
        fix_commits: std::mem::take(&mut walked.fix_commits),
        ..History::default()
    };
    // Tracing goes no further back than the walk did: older lines are
    // blamed on a commit before it, which is never one of the fix commits.
    let since = walked
        .is_cut_short
        .then_some(walked.oldest_time)
        .flatten()
        .map(|seconds| gix::date::Time::new(seconds, 0));
    let tracing = Tracing {
        root,
        head,
        since,
        max_file_bytes: limits.max_file_bytes,
    };
    trace_lines(&repo, &tracing, &walked, limits, &is_over, &mut history);
    history
        .owned
        .sort_by(|a, b| (&a.file, a.first_line).cmp(&(&b.file, b.first_line)));
    Ok(history)
}

/// Opens the repository at `root` (its `.git` must be a real folder there)
/// trusting nothing in it that could run a program.
pub(super) fn open(root: &Path) -> Result<gix::Repository, String> {
    let git_dir = root.join(".git");
    if !safe_fs::is_real_dir(&git_dir).unwrap_or(false) {
        return Err(NOT_A_REPOSITORY.into());
    }
    check_git_dir(&git_dir)?;
    let options = gix::open::Options::isolated().with(gix::sec::Trust::Reduced);
    gix::open_opts(root, options).map_err(|e| format!("Could not read the git repository: {e}"))
}

/// Parts of `.git` read before anything else, with the most bytes each
/// may hold (`None`: a folder). A symlink, or anything else odd, there
/// could point the reading outside the project or at an endless file.
const GIT_PARTS: [(&str, Option<u64>); 9] = [
    ("HEAD", Some(4 * 1024)),
    ("config", Some(1024 * 1024)),
    ("packed-refs", Some(64 * 1024 * 1024)),
    ("shallow", Some(16 * 1024 * 1024)),
    ("objects", None),
    ("objects/pack", None),
    ("objects/info", None),
    ("refs", None),
    ("objects/info/commit-graphs", None),
];
/// Parts that make a repository read from another one's folder.
const BORROWING_PARTS: [&str; 3] = [
    "commondir",
    "objects/info/alternates",
    "objects/info/http-alternates",
];

/// Refuses a `.git` whose main parts are symlinks, not what they should
/// be, or too large, or that borrows from another repository.
fn check_git_dir(git_dir: &Path) -> Result<(), String> {
    for part in BORROWING_PARTS {
        if std::fs::symlink_metadata(git_dir.join(part)).is_ok() {
            return Err(format!(
                "This repository reads from another one's folder (.git/{part}), so its \
                 history isn't read."
            ));
        }
    }
    for (part, max_bytes) in GIT_PARTS {
        let metadata = match std::fs::symlink_metadata(git_dir.join(part)) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Could not look at .git/{part}: {e}")),
        };
        let kind = metadata.file_type();
        let is_expected = match max_bytes {
            Some(max) => kind.is_file() && metadata.len() <= max,
            None => kind.is_dir(),
        };
        if !is_expected {
            return Err(format!(
                ".git/{part} is a symlink, too large, or not what git keeps there, so this \
                 repository's history isn't read."
            ));
        }
    }
    Ok(())
}

/// What walking the commits found.
struct Walked {
    commits_read: usize,
    is_cut_short: bool,
    fix_commits: Vec<FixCommit>,
    /// The fix commits' ids, in the same order.
    fix_ids: Vec<ObjectId>,
    /// How many fix commits changed each of the project's files.
    touched: HashMap<String, usize>,
    /// The commit time of the oldest commit read.
    oldest_time: Option<i64>,
}

fn walk(
    repo: &gix::Repository,
    head: ObjectId,
    files: &HashSet<String>,
    limits: Limits,
    is_over: &(dyn Fn() -> bool + Sync),
) -> Result<Walked, String> {
    let commits = repo
        .rev_walk([head])
        .sorting(Sorting::ByCommitTime(Default::default()))
        .all()
        .map_err(|e| format!("Could not walk the history: {e}"))?;
    let mut walked = Walked {
        commits_read: 0,
        is_cut_short: false,
        fix_commits: Vec::new(),
        fix_ids: Vec::new(),
        touched: HashMap::new(),
        oldest_time: None,
    };
    for info in commits {
        if walked.commits_read >= limits.max_commits || is_over() {
            walked.is_cut_short = true;
            break;
        }
        // A broken object ends the walk: what was read still counts.
        let Ok(commit) = info.and_then(|info| info.object()) else {
            walked.is_cut_short = true;
            break;
        };
        walked.commits_read += 1;
        // A date that can't be read mustn't lift the bound on tracing.
        if let Ok(time) = commit.time() {
            walked.oldest_time = Some(
                walked
                    .oldest_time
                    .map_or(time.seconds, |t| t.min(time.seconds)),
            );
        }
        if walked.fix_commits.len() >= limits.max_fix_commits {
            continue;
        }
        let message = commit.message_raw_sloppy().to_str_lossy();
        let Some((keywords, strength)) = keywords_in(&message) else {
            continue;
        };
        // A merge's diff against its first parent is the other branch's
        // work, whose lines blame gives to the commits that wrote them.
        let is_merge = commit.parent_ids().nth(1).is_some();
        let changed = if is_merge {
            Vec::new()
        } else {
            changed_paths(repo, &commit)
        };
        for path in changed {
            if files.contains(&path) {
                *walked.touched.entry(path).or_default() += 1;
            }
        }
        walked.fix_ids.push(commit.id);
        walked.fix_commits.push(FixCommit {
            id: commit.id.to_hex().to_string(),
            short: short(commit.id),
            summary: cut_short(message.lines().next().unwrap_or(""), MAX_SUMMARY_CHARS),
            author: commit
                .author()
                .map(|a| cut_short(&a.name.to_str_lossy(), MAX_AUTHOR_CHARS))
                .unwrap_or_default(),
            time: commit.time().map_or(0, |t| t.seconds),
            keywords,
            strength,
        });
    }
    Ok(walked)
}

/// The paths `commit` changed from its first parent (all of them for a
/// first commit).
fn changed_paths(repo: &gix::Repository, commit: &gix::Commit<'_>) -> Vec<String> {
    let Ok(tree) = commit.tree() else {
        return Vec::new();
    };
    let parent_tree = commit
        .parent_ids()
        .next()
        .and_then(|id| id.object().ok())
        .and_then(|object| object.peel_to_tree().ok());
    let mut options = gix::diff::Options::default();
    options.track_path().track_rewrites(None);
    repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(options))
        .map(|changes| {
            changes
                .iter()
                .map(|change| change.location().to_str_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// What tracing a file's lines needs, besides the repository.
pub(super) struct Tracing<'r> {
    pub root: &'r Path,
    pub head: ObjectId,
    pub since: Option<gix::date::Time>,
    pub max_file_bytes: usize,
}

/// Traces the lines of the files fix commits changed (most changed first,
/// on several threads) to the commit that last changed them, keeping those
/// of fix commits.
fn trace_lines(
    repo: &gix::Repository,
    tracing: &Tracing<'_>,
    walked: &Walked,
    limits: Limits,
    is_over: &(dyn Fn() -> bool + Sync),
    history: &mut History,
) {
    let fix_index: HashMap<ObjectId, usize> = walked
        .fix_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    let mut order: Vec<(&String, &usize)> = walked.touched.iter().collect();
    order.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let files: Vec<&String> = order
        .into_iter()
        .map(|(file, _)| file)
        .take(limits.max_files)
        .collect();
    let blamed = blame_all(repo, tracing, &files, is_over);
    history.files_not_traced = walked.touched.len() - blamed.len();
    for (index, outcome) in blamed {
        let file = files[index];
        match outcome {
            Blamed::Lines(entries) => {
                history.files_traced += 1;
                history
                    .owned
                    .extend(entries.into_iter().filter_map(|(lines, id)| {
                        let commit = *fix_index.get(&id)?;
                        Some(OwnedLines {
                            file: file.clone(),
                            first_line: lines.0,
                            line_count: lines.1,
                            commit,
                        })
                    }));
            }
            Blamed::ChangedSinceCommit => history.files_changed.push(file.clone()),
            Blamed::Not => history.files_not_traced += 1,
        }
    }
    history.files_changed.sort();
}

/// Blames `files` in order, spread over a few threads, until done or
/// `is_over`. Each outcome comes with its file's index.
fn blame_all(
    repo: &gix::Repository,
    tracing: &Tracing<'_>,
    files: &[&String],
    is_over: &(dyn Fn() -> bool + Sync),
) -> Vec<(usize, Blamed)> {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_WORKERS);
    let next = AtomicUsize::new(0);
    let shared = repo.clone().into_sync();
    let mut blamed: Vec<(usize, Blamed)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut repo = shared.to_thread_local();
                    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES / MAX_WORKERS);
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= files.len() || is_over() {
                            return done;
                        }
                        done.push((index, blame(&repo, tracing, files[index])));
                    }
                })
            })
            .collect();
        // A worker that panicked traced nothing that counts.
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    blamed.sort_by_key(|(index, _)| *index);
    blamed
}

pub(super) enum Blamed {
    /// Each hunk: (first line, 1-based; line count), and its commit.
    Lines(Vec<((u32, u32), ObjectId)>),
    /// The file on disk isn't the committed one.
    ChangedSinceCommit,
    /// It couldn't be traced (too large, gone, unreadable).
    Not,
}

pub(super) fn blame(repo: &gix::Repository, tracing: &Tracing<'_>, file: &str) -> Blamed {
    let on_disk = match safe_fs::read_regular_file(&tracing.root.join(file), tracing.max_file_bytes)
    {
        Ok(Some(text)) => text,
        _ => return Blamed::Not,
    };
    let path = file.replace('\\', "/");
    let path: &BStr = path.as_bytes().as_bstr();
    let options = gix::repository::blame_file::Options {
        since: tracing.since,
        ..Default::default()
    };
    let Ok(outcome) = repo.blame_file(path, tracing.head, options) else {
        return Blamed::Not;
    };
    if outcome.blob != on_disk.as_bytes() {
        return Blamed::ChangedSinceCommit;
    }
    let hunks = outcome
        .entries
        .into_iter()
        .filter_map(|entry| {
            let first = entry.start_in_blamed_file.checked_add(1)?;
            Some(((first, entry.len.get()), entry.commit_id))
        })
        .collect();
    Blamed::Lines(hunks)
}

pub(super) fn short(id: ObjectId) -> String {
    id.to_hex_with_len(SHORT_ID).to_string()
}

pub(super) fn cut_short(text: &str, max_chars: usize) -> String {
    let mut chars = text.trim().chars();
    let kept: String = chars.by_ref().take(max_chars).collect();
    match chars.next() {
        Some(_) => kept + "…",
        None => kept,
    }
}
