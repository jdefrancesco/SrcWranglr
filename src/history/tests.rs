use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::test_repo::{self, git, sample};
use super::*;

fn project_files() -> HashSet<String> {
    ["a.c", "b.c"].iter().map(|f| f.to_string()).collect()
}

fn read_sample(dir: &Path, limits: Limits) -> History {
    read(dir, &project_files(), limits, &AtomicBool::new(false)).expect("read")
}

#[test]
fn fix_commits_and_the_lines_they_still_own_are_found() {
    let Some(dir) = sample() else {
        return;
    };
    let history = read_sample(dir.path(), Limits::default());
    assert_eq!(history.commits_read, 4);
    assert!(!history.is_walk_cut_short);
    let summaries: Vec<(&str, Strength)> = history
        .fix_commits
        .iter()
        .map(|c| (c.summary.as_str(), c.strength))
        .collect();
    assert_eq!(
        summaries,
        [
            ("fix typo", Strength::Fix),
            (
                "Fix stack overflow in parse (CVE-2023-12345)",
                Strength::Security
            ),
        ]
    );
    assert_eq!(
        history.fix_commits[1].keywords,
        ["fix", "overflow", "cve-2023-12345"]
    );
    assert_eq!(history.fix_commits[1].author, "Tess");
    assert_eq!(history.fix_commits[0].short.len(), 8);
    let owned: Vec<(&str, u32, u32, usize)> = history
        .owned
        .iter()
        .map(|o| (o.file.as_str(), o.first_line, o.line_count, o.commit))
        .collect();
    assert_eq!(owned, [("a.c", 3, 1, 1), ("b.c", 1, 1, 0)]);
    assert_eq!(history.files_traced, 2);
    assert!(history.files_changed.is_empty());
}

#[test]
fn a_file_changed_since_the_commit_is_left_out() {
    let Some(dir) = sample() else {
        return;
    };
    std::fs::write(dir.path().join("b.c"), "int changed;\nint z;\nint y;\n").expect("write");
    let history = read_sample(dir.path(), Limits::default());
    assert_eq!(history.files_changed, ["b.c"]);
    assert!(history.owned.iter().all(|o| o.file == "a.c"));
}

#[test]
fn limits_cut_the_walk_and_the_tracing_short() {
    let Some(dir) = sample() else {
        return;
    };
    let limits = Limits {
        max_commits: 2,
        max_files: 0,
        ..Limits::default()
    };
    let history = read_sample(dir.path(), limits);
    assert_eq!(history.commits_read, 2);
    assert!(history.is_walk_cut_short);
    assert_eq!(history.fix_commits.len(), 1);
    assert!(history.owned.is_empty());
    assert_eq!(history.files_not_traced, 1);
}

#[test]
fn cancelling_stops_the_reading() {
    let Some(dir) = sample() else {
        return;
    };
    let history = read(
        dir.path(),
        &project_files(),
        Limits::default(),
        &AtomicBool::new(true),
    )
    .expect("read");
    assert_eq!(history.commits_read, 0);
    assert!(history.is_walk_cut_short);
}

#[test]
fn only_a_real_git_folder_at_the_top_is_read() {
    let dir = tempfile::tempdir().expect("dir");
    let error = read(
        dir.path(),
        &HashSet::new(),
        Limits::default(),
        &AtomicBool::new(false),
    )
    .expect_err("not a repository");
    assert_eq!(error, read::NOT_A_REPOSITORY);
    #[cfg(unix)]
    {
        let Some(other) = sample() else {
            return;
        };
        std::os::unix::fs::symlink(other.path().join(".git"), dir.path().join(".git"))
            .expect("symlink");
        let error = read(
            dir.path(),
            &HashSet::new(),
            Limits::default(),
            &AtomicBool::new(false),
        )
        .expect_err("a symlinked .git");
        assert_eq!(error, read::NOT_A_REPOSITORY);
    }
}

#[test]
fn a_repository_config_that_names_programs_runs_none() {
    let Some(dir) = sample() else {
        return;
    };
    // Each of these would run `touch` if git (or a careless reader) obeyed them.
    let marker = dir.path().join("ran");
    let command = format!("touch {}", marker.display());
    for key in [
        "core.fsmonitor",
        "core.pager",
        "diff.external",
        "diff.c.textconv",
    ] {
        assert!(git(dir.path(), &["config", key, &command], 5), "{key}");
    }
    std::fs::write(dir.path().join(".gitattributes"), "*.c diff=c\n").expect("write");
    read_sample(dir.path(), Limits::default());
    assert!(
        !marker.exists(),
        "a program from the repository's config ran"
    );
}

#[test]
fn dates_are_written_as_days() {
    assert_eq!(date(0), "1970-01-01");
    assert_eq!(date(1_700_000_000), "2023-11-14");
    assert_eq!(date(951_782_400), "2000-02-29");
    assert_eq!(date(-86_400), "1969-12-31");
}

/// Times reading a real repository: `SRCWRANGLR_HISTORY_REPO=<checkout>
/// cargo test --release timing -- --ignored --nocapture`.
#[test]
#[ignore = "needs a repository named in SRCWRANGLR_HISTORY_REPO"]
fn timing_on_a_real_repository() {
    let Some(root) = std::env::var_os("SRCWRANGLR_HISTORY_REPO") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let files: HashSet<String> = ignore::Walk::new(&root)
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e
                .path()
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .into_owned();
            let is_c = [".c", ".h", ".cc", ".cpp", ".hpp"]
                .iter()
                .any(|x| path.ends_with(x));
            is_c.then_some(path)
        })
        .collect();
    let max_files = std::env::var("SRCWRANGLR_HISTORY_FILES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(Limits::default().max_files);
    let limits = Limits {
        max_files,
        ..Limits::default()
    };
    let started = std::time::Instant::now();
    let history = read(&root, &files, limits, &AtomicBool::new(false)).expect("read");
    println!(
        "{:?}: {} commits read, {} fix commits, {} files traced ({} not, {} changed), {} hunks",
        started.elapsed(),
        history.commits_read,
        history.fix_commits.len(),
        history.files_traced,
        history.files_not_traced,
        history.files_changed.len(),
        history.owned.len()
    );
}

#[cfg(unix)]
#[test]
fn a_git_folder_with_symlinked_parts_is_refused() {
    let Some(dir) = sample() else {
        return;
    };
    let git = dir.path().join(".git");
    std::fs::rename(git.join("HEAD"), git.join("HEAD.real")).expect("move");
    std::os::unix::fs::symlink("/dev/zero", git.join("HEAD")).expect("symlink");
    let error = read(
        dir.path(),
        &project_files(),
        Limits::default(),
        &AtomicBool::new(false),
    )
    .expect_err("refused");
    assert!(error.contains(".git/HEAD"), "{error}");
}

#[test]
fn a_repository_borrowing_another_ones_objects_is_refused() {
    let Some(dir) = sample() else {
        return;
    };
    let alternates = dir.path().join(".git/objects/info/alternates");
    std::fs::create_dir_all(alternates.parent().expect("parent")).expect("mkdir");
    std::fs::write(&alternates, "/somewhere/else/objects\n").expect("write");
    let error = read(
        dir.path(),
        &project_files(),
        Limits::default(),
        &AtomicBool::new(false),
    )
    .expect_err("refused");
    assert!(error.contains("another one's folder"), "{error}");
}

#[test]
fn a_merge_commit_is_kept_but_not_diffed() {
    let Some(dir) = sample() else {
        return;
    };
    assert!(git(dir.path(), &["checkout", "-q", "-b", "side"], 6));
    test_repo::commit(dir.path(), &[("c.c", "int c;\n")], "Add c", 7);
    assert!(git(dir.path(), &["checkout", "-q", "-"], 8));
    assert!(git(
        dir.path(),
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge fix for the side branch",
            "side"
        ],
        9
    ));
    let files: HashSet<String> = ["a.c", "b.c", "c.c"]
        .iter()
        .map(|f| f.to_string())
        .collect();
    let history = read(
        dir.path(),
        &files,
        Limits::default(),
        &AtomicBool::new(false),
    )
    .expect("read");
    assert_eq!(
        history.fix_commits[0].summary,
        "Merge fix for the side branch"
    );
    // c.c came in through the merge only, so it isn't traced.
    assert!(history.owned.iter().all(|o| o.file != "c.c"));
    assert_eq!(history.files_traced, 2);
}
