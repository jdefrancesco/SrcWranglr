//! What changed between a commit you pick (the base: a tag, a branch or a
//! commit id) and the checked-out one: the commits in between, and for
//! each of the project's files that changed, which lines of the checked-out
//! code are new or changed and where lines were removed. The newer end is
//! always the checkout, so the lines are numbered as the project was
//! indexed. A file that differs from the checkout on disk is left out.
//!
//! Like the rest of the history, it is read with gitoxide, never by running
//! `git`. The base is resolved by hand (a reference, else a commit id
//! prefix), not with `git rev-parse`'s full syntax, some of which (`:/text`)
//! searches every commit.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use gix::bstr::ByteSlice;
use gix::diff::blob::{diff_with_slider_heuristics, Algorithm, InternedInput};
use gix::hash::ObjectId;
use gix::object::tree::diff::ChangeDetached;
use gix::revision::walk::Sorting;
use serde::{Deserialize, Serialize};

use super::read::{
    cut_short, open, short, MAX_AUTHOR_CHARS, MAX_SUMMARY_CHARS, OBJECT_CACHE_BYTES,
};
use super::Limits;
use crate::safe_fs;

/// A base longer than this isn't looked for.
const MAX_BASE_CHARS: usize = 256;
/// Hex digits in a full commit id, and the fewest taken as a prefix.
const FULL_ID_CHARS: usize = 40;
const MIN_PREFIX_CHARS: usize = 4;
/// Commits listed, newest first; more are only counted.
const MAX_LISTED_COMMITS: usize = 1_000;
/// Commits counted at most.
const MAX_RANGE_COMMITS: usize = 200_000;
/// Commits of the base's own counted at most.
const MAX_BASE_AHEAD: usize = 10_000;
/// Changed files diffed at most.
const MAX_CHANGED_FILES: usize = 20_000;
/// Hunks kept per file.
const MAX_HUNKS_PER_FILE: usize = 5_000;
/// Deleted files listed at most.
const MAX_DELETED: usize = 2_000;
/// Tags and branches offered at most, and read at most.
const MAX_REFS: usize = 200;
const MAX_REFS_READ: usize = 20_000;
/// A blob with a NUL byte in its first this many bytes is binary.
const BINARY_SNIFF_BYTES: usize = 8_000;
/// File name endings of C and C++ source, for listing deleted files.
const SOURCE_ENDINGS: [&str; 10] = [
    ".c", ".cc", ".cpp", ".cxx", ".c++", ".h", ".hh", ".hpp", ".hxx", ".inl",
];

/// New or changed lines of a file as checked out, or where lines were
/// removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// 1-based. Where lines were only removed: the line now after them.
    pub first_line: u32,
    /// New or changed lines; 0 where lines were only removed.
    pub line_count: u32,
    /// Lines of the base's version this replaced or removed.
    pub removed: u32,
}

/// One of the project's files that changed in the range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Project-relative path, as checked out.
    pub file: String,
    /// Where it was at the base, if it moved.
    pub renamed_from: Option<String>,
    /// Whether it wasn't there at the base.
    pub is_new: bool,
    /// In line order.
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
}

/// A commit in the range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeCommit {
    pub short: String,
    pub summary: String,
    pub author: String,
    /// Seconds since 1970, UTC.
    pub time: i64,
}

/// What changed from a base to the checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    /// The base as asked for.
    pub base: String,
    pub base_short: String,
    pub head: String,
    /// Commits in the checkout's history but not the base's.
    pub commit_count: usize,
    /// Newest first, the first `MAX_LISTED_COMMITS`.
    pub commits: Vec<RangeCommit>,
    pub is_walk_cut_short: bool,
    /// Commits in the base's history but not the checkout's, up to
    /// `MAX_BASE_AHEAD`: when the checkout has none of its own, the base
    /// is newer than it, and the changes run backwards.
    pub base_ahead: usize,
    /// By path.
    pub files: Vec<ChangedFile>,
    /// C and C++ files there at the base, gone now.
    pub deleted: Vec<String>,
    /// Files that differ from the checkout on disk: left out.
    pub files_changed_on_disk: Vec<String>,
    /// Changed files not diffed (too large, binary, unreadable).
    pub files_not_read: usize,
    /// Whether more files changed than were diffed (limits).
    pub is_cut_short: bool,
}

/// Bases to offer: tags (newest commit first) and local branches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refs {
    pub tags: Vec<String>,
    pub branches: Vec<String>,
}

/// What changed in the checkout at `root` since `base`, for the project's
/// `files`, within `limits`.
pub fn changes(
    root: &Path,
    base: &str,
    files: &[String],
    limits: Limits,
) -> Result<Changes, String> {
    let started = Instant::now();
    let is_over = || started.elapsed() > limits.budget;
    let mut repo = open(root)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    let head = repo
        .head_id()
        .map_err(|_| "This repository has no commits yet.".to_string())?
        .detach();
    let base_id = resolve(&repo, base)?;
    let mut changes = Changes {
        base: base.trim().to_string(),
        base_short: short(base_id),
        head: short(head),
        ..Changes::default()
    };
    list_commits(&repo, head, base_id, &is_over, &mut changes)?;
    changes.base_ahead = count_commits(&repo, base_id, head, MAX_BASE_AHEAD, &is_over);
    let project: HashSet<&str> = files.iter().map(String::as_str).collect();
    let reading = Reading {
        repo: &repo,
        root,
        project: &project,
        max_file_bytes: limits.max_file_bytes,
    };
    let tree_changes = tree_changes(&repo, base_id, head)?;
    read_files(&reading, &tree_changes, &is_over, &mut changes);
    Ok(changes)
}

/// Diffs the project's files among `tree_changes` into `changes`, until
/// done, the limit, or `is_over`.
fn read_files(
    reading: &Reading<'_>,
    tree_changes: &[ChangeDetached],
    is_over: &dyn Fn() -> bool,
    changes: &mut Changes,
) {
    for change in tree_changes {
        if is_over() || changes.files.len() >= MAX_CHANGED_FILES {
            changes.is_cut_short = true;
            break;
        }
        match reading.one(change) {
            Seen::File(file) => changes.files.push(file),
            Seen::Deleted(path) if changes.deleted.len() < MAX_DELETED => {
                changes.deleted.push(path)
            }
            Seen::OnDisk(path) => changes.files_changed_on_disk.push(path),
            Seen::NotRead => changes.files_not_read += 1,
            Seen::Deleted(_) | Seen::Skipped => {}
        }
    }
    changes.files.sort_by(|a, b| a.file.cmp(&b.file));
    changes.deleted.sort();
    changes.files_changed_on_disk.sort();
}

/// The commit `base` names: a reference (`v6.6`, `main`, `refs/tags/x`),
/// else a commit id or a unique prefix of one; a tag is peeled to its
/// commit.
fn resolve(repo: &gix::Repository, base: &str) -> Result<ObjectId, String> {
    let name = base.trim();
    let is_plain = !name.is_empty()
        && name.chars().count() <= MAX_BASE_CHARS
        && !name.chars().any(|c| c.is_control() || c.is_whitespace());
    if !is_plain {
        return Err("Give the base as a tag, a branch or a commit id.".into());
    }
    let id = named_object(repo, name)?;
    let commit = repo
        .find_object(id)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .ok_or_else(|| format!("{name} isn't a commit."))?;
    Ok(commit.id)
}

/// The object `name` names: a reference, or a commit id or unique prefix
/// of one. A full id is taken as one, whatever a reference may be called;
/// a shorter one that is also a reference's name is refused, so a tag
/// named like a commit can't stand in for it.
fn named_object(repo: &gix::Repository, name: &str) -> Result<ObjectId, String> {
    let by_reference = match repo.try_find_reference(name) {
        Ok(Some(mut reference)) => Some(
            reference
                .peel_to_id()
                .map_err(|e| format!("Could not follow {name}: {e}"))?
                .detach(),
        ),
        _ => None,
    };
    let by_id = by_prefix(repo, name).transpose()?;
    match (by_reference, by_id) {
        (_, Some(id)) if name.len() == FULL_ID_CHARS => Ok(id),
        (Some(reference), Some(id)) if reference != id => Err(format!(
            "{name} is both a tag or branch and the start of a commit id: give more of the id, \
             or the reference in full (refs/tags/{name})."
        )),
        (Some(id), _) | (None, Some(id)) => Ok(id),
        (None, None) => Err(format!(
            "Nothing here is named {name}: give a tag, a branch or a commit id."
        )),
    }
}

/// The object a hex id or unique prefix names; `None` if `name` isn't one
/// or names nothing.
fn by_prefix(repo: &gix::Repository, name: &str) -> Option<Result<ObjectId, String>> {
    let is_hex = (MIN_PREFIX_CHARS..=FULL_ID_CHARS).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_hexdigit());
    if !is_hex {
        return None;
    }
    let prefix = gix::hash::Prefix::from_hex(name).ok()?;
    match repo.objects.lookup_prefix(prefix, None) {
        Ok(Some(Ok(id))) => Some(Ok(id)),
        Ok(Some(Err(()))) => Some(Err(format!(
            "{name} could be several objects: give more of its id."
        ))),
        _ => None,
    }
}

/// Counts the commits the checkout has that `base` hasn't, listing the
/// newest.
fn list_commits(
    repo: &gix::Repository,
    head: ObjectId,
    base: ObjectId,
    is_over: &dyn Fn() -> bool,
    changes: &mut Changes,
) -> Result<(), String> {
    let walk = repo
        .rev_walk([head])
        .with_hidden([base])
        .sorting(Sorting::ByCommitTime(Default::default()))
        .all()
        .map_err(|e| format!("Could not walk the history: {e}"))?;
    for info in walk {
        if changes.commit_count >= MAX_RANGE_COMMITS || is_over() {
            changes.is_walk_cut_short = true;
            break;
        }
        // A broken object ends the walk: what was read still counts.
        let Ok(info) = info else {
            changes.is_walk_cut_short = true;
            break;
        };
        changes.commit_count += 1;
        // Only the listed ones are read whole.
        if changes.commits.len() < MAX_LISTED_COMMITS {
            if let Ok(commit) = info.object() {
                changes.commits.push(range_commit(&commit));
            }
        }
    }
    Ok(())
}

/// How many commits `tip` has that `hidden` hasn't, up to `max`.
fn count_commits(
    repo: &gix::Repository,
    tip: ObjectId,
    hidden: ObjectId,
    max: usize,
    is_over: &dyn Fn() -> bool,
) -> usize {
    let Ok(walk) = repo.rev_walk([tip]).with_hidden([hidden]).all() else {
        return 0;
    };
    walk.take_while(|info| info.is_ok() && !is_over())
        .take(max)
        .count()
}

fn range_commit(commit: &gix::Commit<'_>) -> RangeCommit {
    let message = commit.message_raw_sloppy().to_str_lossy();
    RangeCommit {
        short: short(commit.id),
        summary: cut_short(message.lines().next().unwrap_or(""), MAX_SUMMARY_CHARS),
        author: commit
            .author()
            .map(|a| cut_short(&a.name.to_str_lossy(), MAX_AUTHOR_CHARS))
            .unwrap_or_default(),
        time: commit.time().map_or(0, |t| t.seconds),
    }
}

/// The changes from `base`'s tree to `head`'s, with moved files found.
fn tree_changes(
    repo: &gix::Repository,
    base: ObjectId,
    head: ObjectId,
) -> Result<Vec<ChangeDetached>, String> {
    let tree = |id: ObjectId| -> Result<gix::Tree<'_>, String> {
        let unreadable =
            |e: &dyn std::fmt::Display| format!("Could not read a commit's files: {e}");
        let commit = repo.find_commit(id).map_err(|e| unreadable(&e))?;
        commit.tree().map_err(|e| unreadable(&e))
    };
    let (base_tree, head_tree) = (tree(base)?, tree(head)?);
    let mut options = gix::diff::Options::default();
    options
        .track_path()
        .track_rewrites(Some(gix::diff::Rewrites::default()));
    repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(options))
        .map_err(|e| format!("Could not compare the commits: {e}"))
}

/// What one tree change is to the project.
enum Seen {
    File(ChangedFile),
    Deleted(String),
    OnDisk(String),
    NotRead,
    Skipped,
}

/// What reading the changes needs.
struct Reading<'a> {
    repo: &'a gix::Repository,
    root: &'a Path,
    project: &'a HashSet<&'a str>,
    max_file_bytes: usize,
}

impl Reading<'_> {
    fn one(&self, change: &ChangeDetached) -> Seen {
        match change {
            ChangeDetached::Addition {
                location,
                entry_mode,
                id,
                ..
            } if entry_mode.is_blob() => {
                self.file(location.to_str_lossy().as_ref(), None, None, *id)
            }
            ChangeDetached::Modification {
                location,
                entry_mode,
                previous_id,
                id,
                ..
            } if entry_mode.is_blob() => self.file(
                location.to_str_lossy().as_ref(),
                None,
                Some(*previous_id),
                *id,
            ),
            ChangeDetached::Rewrite {
                source_location,
                source_id,
                location,
                entry_mode,
                id,
                ..
            } if entry_mode.is_blob() => {
                let from = source_location.to_str_lossy().into_owned();
                self.file(
                    location.to_str_lossy().as_ref(),
                    Some(from),
                    Some(*source_id),
                    *id,
                )
            }
            ChangeDetached::Deletion { location, .. } => {
                let path = location.to_str_lossy().into_owned();
                match is_source_path(&path) {
                    true => Seen::Deleted(path),
                    false => Seen::Skipped,
                }
            }
            _ => Seen::Skipped,
        }
    }

    /// The project file `path`, now blob `id`, which was blob `before`
    /// (`None`: new) at the base, at `renamed_from` if it moved.
    fn file(
        &self,
        path: &str,
        renamed_from: Option<String>,
        before: Option<ObjectId>,
        id: ObjectId,
    ) -> Seen {
        if !self.project.contains(path) {
            return Seen::Skipped;
        }
        let Some(now) = self.blob(id) else {
            return Seen::NotRead;
        };
        match safe_fs::read_regular_file(&self.root.join(path), self.max_file_bytes) {
            Ok(Some(on_disk)) if on_disk.as_bytes() == now.as_slice() => {}
            Ok(Some(_)) => return Seen::OnDisk(path.to_string()),
            _ => return Seen::NotRead,
        }
        let then = match before {
            Some(before) => match self.blob(before) {
                Some(then) => then,
                None => return Seen::NotRead,
            },
            None => Vec::new(),
        };
        let (hunks, added, removed) = hunks(&then, &now);
        Seen::File(ChangedFile {
            file: path.to_string(),
            renamed_from,
            is_new: before.is_none(),
            hunks,
            added,
            removed,
        })
    }

    /// The text of blob `id`, if it is small enough and not binary.
    fn blob(&self, id: ObjectId) -> Option<Vec<u8>> {
        let header = self.repo.find_header(id).ok()?;
        if header.size() > self.max_file_bytes as u64 {
            return None;
        }
        let data = self.repo.find_object(id).ok()?.detach().data;
        let sniffed = &data[..data.len().min(BINARY_SNIFF_BYTES)];
        (!sniffed.contains(&0)).then_some(data)
    }
}

/// The new or changed lines of `now` against `then`, and how many lines
/// were added and removed in all.
fn hunks(then: &[u8], now: &[u8]) -> (Vec<Hunk>, u32, u32) {
    let input = InternedInput::new(then, now);
    let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);
    let hunks = diff
        .hunks()
        .take(MAX_HUNKS_PER_FILE)
        .map(|hunk| Hunk {
            first_line: hunk.after.start + 1,
            line_count: hunk.after.end - hunk.after.start,
            removed: hunk.before.end - hunk.before.start,
        })
        .collect();
    (hunks, diff.count_additions(), diff.count_removals())
}

fn is_source_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SOURCE_ENDINGS.iter().any(|ending| lower.ends_with(ending))
}

/// The tags (the newest commit first) and local branches of the checkout
/// at `root`, to offer as bases.
pub fn refs(root: &Path) -> Result<Refs, String> {
    let repo = open(root)?;
    let platform = repo
        .references()
        .map_err(|e| format!("Could not read the repository's tags: {e}"))?;
    let mut tags: Vec<(i64, String)> = platform
        .tags()
        .map_err(|e| format!("Could not read the repository's tags: {e}"))?
        .take(MAX_REFS_READ)
        .filter_map(Result::ok)
        .filter_map(|mut reference| {
            let name = reference.name().shorten().to_str_lossy().into_owned();
            let time = reference.peel_to_commit().ok()?.time().ok()?.seconds;
            Some((time, name))
        })
        .collect();
    tags.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut branches: Vec<String> = platform
        .local_branches()
        .map_err(|e| format!("Could not read the repository's branches: {e}"))?
        .take(MAX_REFS_READ)
        .filter_map(Result::ok)
        .map(|reference| reference.name().shorten().to_str_lossy().into_owned())
        .collect();
    branches.sort();
    branches.truncate(MAX_REFS);
    Ok(Refs {
        tags: tags
            .into_iter()
            .take(MAX_REFS)
            .map(|(_, name)| name)
            .collect(),
        branches,
    })
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
