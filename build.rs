//! Records which commit SrcWranglr is built from (`git describe`) in
//! `SRCWRANGLR_BUILD`, for `--version` and About. Outside a git checkout of
//! this project (a source archive), it is left empty.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What git may say back: a tag, commit or path, never anything that could
/// end a cargo directive line.
const MAX_OUTPUT: usize = 4096;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let is_own_checkout = git(&manifest_dir, &["rev-parse", "--show-toplevel"])
        .is_some_and(|top| same_dir(Path::new(&top), &manifest_dir));
    let build = is_own_checkout
        .then(|| git(&manifest_dir, &["describe", "--tags", "--always"]))
        .flatten()
        .unwrap_or_default();
    println!("cargo:rustc-env=SRCWRANGLR_BUILD={build}");
    println!("cargo:rerun-if-changed=build.rs");
    if is_own_checkout {
        watch_git(&manifest_dir);
    }
}

/// Builds again when a commit, checkout or tag changes what `git describe`
/// says. Only paths that exist: a missing one makes cargo rerun every build.
fn watch_git(manifest_dir: &Path) {
    let dirs = [
        git(manifest_dir, &["rev-parse", "--git-dir"]),
        git(manifest_dir, &["rev-parse", "--git-common-dir"]),
    ];
    for dir in dirs.into_iter().flatten() {
        let dir = manifest_dir.join(dir);
        for name in ["HEAD", "refs", "packed-refs"] {
            let path = dir.join(name);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

/// git's one-line answer in `dir`, if it gave a plain one.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    let is_plain =
        !text.is_empty() && text.len() <= MAX_OUTPUT && !text.chars().any(char::is_control);
    is_plain.then(|| text.to_string())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
