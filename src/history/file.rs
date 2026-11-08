//! One file's history as checked out: which commit last changed each of
//! its lines, for any commit (not just fixes). Read on demand, for the
//! function you're looking at.

use std::collections::HashMap;
use std::path::Path;

use gix::bstr::ByteSlice;
use gix::hash::ObjectId;
use serde::{Deserialize, Serialize};

use super::keywords::keywords_in;
use super::read::{
    blame, cut_short, open, short, Blamed, Tracing, MAX_AUTHOR_CHARS, MAX_SUMMARY_CHARS,
    OBJECT_CACHE_BYTES,
};
use super::Strength;

/// A commit that last changed some of a file's lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub short: String,
    /// The message's first line, cut short if long.
    pub summary: String,
    pub author: String,
    /// Seconds since 1970, UTC.
    pub time: i64,
    /// How strongly its message says it fixed something, if it does.
    pub strength: Option<Strength>,
}

/// Lines a commit last changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRun {
    /// 1-based.
    pub first_line: u32,
    pub line_count: u32,
    /// Index into `FileHistory::commits`.
    pub commit: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHistory {
    pub commits: Vec<CommitInfo>,
    /// In line order, covering the file.
    pub runs: Vec<LineRun>,
}

/// Who last changed each line of `file` (project-relative) in the checkout
/// at `root`.
pub fn file_history(root: &Path, file: &str, max_file_bytes: usize) -> Result<FileHistory, String> {
    let mut repo = open(root)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    let head = repo
        .head_id()
        .map_err(|_| "This repository has no commits yet.".to_string())?
        .detach();
    let tracing = Tracing {
        root,
        head,
        since: None,
        max_file_bytes,
    };
    let hunks = match blame(&repo, &tracing, file) {
        Blamed::Lines(hunks) => hunks,
        Blamed::ChangedSinceCommit => {
            return Err(
                "This file differs from the checked-out commit, so its lines can't be \
                        matched to commits."
                    .into(),
            )
        }
        Blamed::Not => {
            return Err(
                "This file's history couldn't be read: it isn't in the checked-out \
                        commit, or it is too large."
                    .into(),
            )
        }
    };
    let mut history = FileHistory::default();
    let mut index_of: HashMap<ObjectId, usize> = HashMap::new();
    for ((first_line, line_count), id) in hunks {
        let commit = *index_of.entry(id).or_insert_with(|| {
            history.commits.push(commit_info(&repo, id));
            history.commits.len() - 1
        });
        history.runs.push(LineRun {
            first_line,
            line_count,
            commit,
        });
    }
    Ok(history)
}

fn commit_info(repo: &gix::Repository, id: ObjectId) -> CommitInfo {
    let Ok(commit) = repo.find_commit(id) else {
        return CommitInfo {
            short: short(id),
            summary: "(commit not readable)".into(),
            author: String::new(),
            time: 0,
            strength: None,
        };
    };
    let message = commit.message_raw_sloppy().to_str_lossy();
    CommitInfo {
        short: short(id),
        summary: cut_short(message.lines().next().unwrap_or(""), MAX_SUMMARY_CHARS),
        author: commit
            .author()
            .map(|a| cut_short(&a.name.to_str_lossy(), MAX_AUTHOR_CHARS))
            .unwrap_or_default(),
        time: commit.time().map_or(0, |t| t.seconds),
        strength: keywords_in(&message).map(|(_, strength)| strength),
    }
}

/// The commits that last changed lines `lines` (1-based, end excluded) of
/// a file with `history`: each with how many of those lines and the first
/// of them; newest first.
pub fn commits_in(history: &FileHistory, lines: std::ops::Range<u32>) -> Vec<(usize, u32, u32)> {
    let mut per_commit: HashMap<usize, (u32, u32)> = HashMap::new();
    for run in &history.runs {
        let run_end = run.first_line.saturating_add(run.line_count);
        let start = run.first_line.max(lines.start);
        let end = run_end.min(lines.end);
        if start >= end {
            continue;
        }
        let entry = per_commit.entry(run.commit).or_insert((0, start));
        entry.0 += end - start;
        entry.1 = entry.1.min(start);
    }
    let mut found: Vec<(usize, u32, u32)> = per_commit
        .into_iter()
        .map(|(commit, (count, first))| (commit, count, first))
        .collect();
    found.sort_by_key(|&(commit, _, first)| {
        (std::cmp::Reverse(history.commits[commit].time), first)
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::test_repo::sample;

    #[test]
    fn each_line_is_traced_to_the_commit_that_last_changed_it() {
        let Some(repo) = sample() else {
            return;
        };
        let history = file_history(repo.path(), "a.c", 1 << 20).expect("read");
        let summaries: Vec<&str> = history.commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries.len(), 2, "{summaries:?}");
        assert!(summaries.contains(&"Initial import"));
        let fix = history
            .commits
            .iter()
            .position(|c| c.summary.starts_with("Fix stack overflow"))
            .expect("fix");
        assert_eq!(history.commits[fix].strength, Some(Strength::Security));
        let line_three = history
            .runs
            .iter()
            .find(|r| (r.first_line..r.first_line + r.line_count).contains(&3))
            .expect("line 3");
        assert_eq!(line_three.commit, fix);
        // Lines 1–5 (the function): the fix (newest) first, owning line 3.
        let commits = commits_in(&history, 1..6);
        assert_eq!(commits[0], (fix, 1, 3));
        assert_eq!(commits[1].1, 4);
    }

    #[test]
    fn a_file_changed_since_the_commit_is_refused() {
        let Some(repo) = sample() else {
            return;
        };
        std::fs::write(repo.path().join("a.c"), "changed\n").expect("write");
        let error = file_history(repo.path(), "a.c", 1 << 20).expect_err("changed");
        assert!(error.contains("differs"), "{error}");
        assert!(file_history(repo.path(), "missing.c", 1 << 20).is_err());
    }
}
