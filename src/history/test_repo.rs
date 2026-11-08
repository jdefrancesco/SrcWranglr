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
