//! Tests for reading what changed in a range of commits.

use std::path::Path;

use super::*;
use crate::history::test_repo::{git, ranged};

/// The checked-out project's files.
fn files() -> Vec<String> {
    ["a.c", "d.c", "e.c", "f.c"]
        .iter()
        .map(|f| f.to_string())
        .collect()
}

fn read_range(root: &Path, base: &str) -> Result<Changes, String> {
    changes(root, base, &files(), Limits::default())
}

fn hunks(changes: &Changes, file: &str) -> Vec<(u32, u32, u32)> {
    changes
        .files
        .iter()
        .find(|f| f.file == file)
        .unwrap_or_else(|| panic!("{file} not changed"))
        .hunks
        .iter()
        .map(|h| (h.first_line, h.line_count, h.removed))
        .collect()
}

#[test]
fn each_changed_file_has_its_new_lines_in_the_checked_out_numbering() {
    let Some(dir) = ranged() else {
        return;
    };
    let changes = read_range(dir.path(), "v1").expect("changes");
    let names: Vec<&str> = changes.files.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(names, ["a.c", "d.c", "e.c", "f.c"]);
    // "return 1" became two lines.
    assert_eq!(hunks(&changes, "a.c"), [(2, 2, 1)]);
    // Moved, and one line changed: diffed against its old self.
    assert_eq!(hunks(&changes, "d.c"), [(3, 1, 1)]);
    assert_eq!(changes.files[1].renamed_from.as_deref(), Some("b.c"));
    // New: every line.
    assert_eq!(hunks(&changes, "e.c"), [(1, 2, 0)]);
    assert!(changes.files[2].is_new);
    // Only removed: where the lines were, none new.
    assert_eq!(hunks(&changes, "f.c"), [(2, 0, 1)]);
    assert_eq!(changes.deleted, ["c.h"]);
}

#[test]
fn the_commits_in_the_range_are_listed_newest_first() {
    let Some(dir) = ranged() else {
        return;
    };
    let changes = read_range(dir.path(), "v1").expect("changes");
    assert_eq!(changes.commit_count, 5);
    let summaries: Vec<&str> = changes.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        ["Shrink f", "Add e", "Drop c", "Move b", "Change a"]
    );
    assert_eq!(changes.commits[0].author, "Tess");
    assert_eq!(changes.base_short.len(), 8);
    assert!(!changes.is_walk_cut_short);
}

#[test]
fn the_base_can_be_a_tag_a_branch_or_a_commit_id() {
    let Some(dir) = ranged() else {
        return;
    };
    let by_tag = read_range(dir.path(), "v1").expect("tag");
    for base in ["v1a", "old", "refs/tags/v1", by_tag.base_short.as_str()] {
        let changes = read_range(dir.path(), base).unwrap_or_else(|e| panic!("{base}: {e}"));
        assert_eq!(changes.base_short, by_tag.base_short, "{base}");
        assert_eq!(changes.files.len(), 4, "{base}");
    }
}

#[test]
fn a_base_that_names_nothing_is_refused() {
    let Some(dir) = ranged() else {
        return;
    };
    for base in [
        "nope",
        "",
        "  ",
        "v1\u{0}",
        "deadbeefdeadbeef",
        &"x".repeat(300),
    ] {
        assert!(read_range(dir.path(), base).is_err(), "{base:?}");
    }
}

#[test]
fn a_file_changed_since_the_commit_is_left_out() {
    let Some(dir) = ranged() else {
        return;
    };
    std::fs::write(dir.path().join("a.c"), "edited\n").expect("write");
    let changes = read_range(dir.path(), "v1").expect("changes");
    assert_eq!(changes.files_changed_on_disk, ["a.c"]);
    assert!(changes.files.iter().all(|f| f.file != "a.c"));
}

#[test]
fn only_the_projects_files_are_diffed() {
    let Some(dir) = ranged() else {
        return;
    };
    let only_a = vec!["a.c".to_string()];
    let changes = changes(dir.path(), "v1", &only_a, Limits::default()).expect("changes");
    let names: Vec<&str> = changes.files.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(names, ["a.c"]);
}

#[test]
fn recent_tags_and_branches_are_offered_as_bases() {
    let Some(dir) = ranged() else {
        return;
    };
    let refs = refs(dir.path()).expect("refs");
    assert!(refs.tags.iter().any(|t| t == "v1"), "{refs:?}");
    assert!(refs.tags.iter().any(|t| t == "v1a"), "{refs:?}");
    assert!(refs.branches.iter().any(|b| b == "old"), "{refs:?}");
}

#[test]
fn a_base_newer_than_the_checkout_is_noticed() {
    let Some(dir) = ranged() else {
        return;
    };
    let root = dir.path();
    assert!(git(root, &["tag", "later"], 7));
    assert!(git(root, &["checkout", "-q", "v1"], 7));
    let at_v1 = ["a.c", "b.c", "c.h", "f.c"].map(String::from).to_vec();
    let changes = changes(root, "later", &at_v1, Limits::default()).expect("changes");
    assert_eq!(changes.commit_count, 0);
    assert_eq!(changes.base_ahead, 5);
    let changes = read_range(root, "v1").expect("changes");
    assert_eq!(changes.base_ahead, 0);
}

#[test]
fn a_tag_named_like_a_commit_id_doesnt_hide_it() {
    let Some(dir) = ranged() else {
        return;
    };
    let root = dir.path();
    let first = read_range(root, "v1").expect("v1");
    let full = full_id(root, &first.base_short);
    // A tag named like the first commit's short id, on another commit.
    assert!(git(root, &["tag", &first.base_short, "HEAD~1"], 8));
    let error = read_range(root, &first.base_short).expect_err("ambiguous");
    assert!(error.contains("both"), "{error}");
    let by_full_id = read_range(root, &full).expect("full id");
    assert_eq!(by_full_id.base_short, first.base_short);
}

/// The full id of the commit whose id starts with `short`.
fn full_id(root: &Path, short: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", short])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
