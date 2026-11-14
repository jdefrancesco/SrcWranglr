//! Small git repositories for tests, made with the `git` program (with
//! none of your own config).

use std::path::Path;
use std::process::Command;

/// Runs `git` in `dir` with nothing from your own config, at a fixed time.
pub fn git(dir: &Path, args: &[&str], time: u32) -> bool {
    let date = format!("{} +0000", 1_700_000_000 + time * 60);
    Command::new("git")
        .args(["-c", "user.name=Tess", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Writes `files` and commits them as `message`.
pub fn commit(dir: &Path, files: &[(&str, &str)], message: &str, time: u32) {
    for (name, text) in files {
        std::fs::write(dir.join(name), text).expect("write");
    }
    assert!(git(dir, &["add", "-A"], time), "git add");
    assert!(
        git(dir, &["commit", "-q", "-m", message], time),
        "git commit"
    );
}

/// A repository with an overflow fix in a.c and a typo fix in b.c; `None`
/// where git isn't installed.
pub fn sample() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().expect("dir");
    if !git(dir.path(), &["init", "-q"], 0) {
        return None;
    }
    let a = "int parse(char *b) {\n  char t[8];\n  strcpy(t, b);\n  return 0;\n}\n";
    commit(
        dir.path(),
        &[("a.c", a), ("b.c", "int x;\n")],
        "Initial import",
        1,
    );
    let fixed = a.replace("strcpy(t, b);", "strncpy(t, b, 7);");
    commit(
        dir.path(),
        &[("a.c", &fixed)],
        "Fix stack overflow in parse (CVE-2023-12345)",
        2,
    );
    commit(dir.path(), &[("b.c", "int x;\nint y;\n")], "Add y", 3);
    commit(dir.path(), &[("b.c", "int z;\nint y;\n")], "fix typo", 4);
    Some(dir)
}

const B: &str = "int b(void) {\n  int x = 1;\n  int y = 2;\n  return x + y;\n}\n";

/// A repository tagged `v1` (and `v1a`, annotated; branch `old`) at its
/// first commit, then: a.c changed, b.c moved to d.c and changed, c.h
/// deleted, e.c added, a line of f.c removed. `None` without git.
pub fn ranged() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path();
    if !git(root, &["init", "-q"], 0) {
        return None;
    }
    let a = "int a(void) {\n  return 1;\n}\n";
    let f = "one\ntwo\nthree\n";
    commit(
        root,
        &[("a.c", a), ("b.c", B), ("c.h", "int c;\n"), ("f.c", f)],
        "Initial",
        1,
    );
    assert!(git(root, &["tag", "v1"], 1));
    assert!(git(root, &["tag", "-a", "v1a", "-m", "release"], 1));
    assert!(git(root, &["branch", "old"], 1));
    commit(
        root,
        &[("a.c", "int a(void) {\n  int z = 2;\n  return z;\n}\n")],
        "Change a",
        2,
    );
    assert!(git(root, &["mv", "b.c", "d.c"], 3));
    commit(root, &[("d.c", &B.replace("y = 2", "y = 3"))], "Move b", 3);
    assert!(git(root, &["rm", "-q", "c.h"], 4));
    assert!(git(root, &["commit", "-q", "-m", "Drop c"], 4));
    commit(root, &[("e.c", "int e;\nint f;\n")], "Add e", 5);
    commit(root, &[("f.c", "one\nthree\n")], "Shrink f", 6);
    Some(dir)
}
