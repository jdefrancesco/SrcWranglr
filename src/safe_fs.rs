//! File access for files SrcWranglr keeps inside a project, which may be
//! an untrusted source tree: symlinks are never followed, reads are size
//! limited, writes replace the file atomically.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tells apart the temporary files of writes made by this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether `dir` exists as a directory (not a symlink to one). Something
/// else at that path is an error.
pub fn is_real_dir(dir: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(std::io::Error::other(
            "exists but is not a folder (symlinks are not followed)",
        )),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Creates `path` fresh (never writing through a symlink left there).
pub fn write_new_file(path: &Path, contents: &str) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    create_file(path, contents)
}

/// Creates `path`, failing if anything (even a symlink) is there.
fn create_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// The text of `path` if it is a regular file (not a symlink, in a folder
/// that isn't one either) of at most `max_bytes`; `None` if there is
/// nothing there.
pub fn read_regular_file(path: &Path, max_bytes: usize) -> std::io::Result<Option<String>> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !is_real_dir(parent)? {
            return Ok(None);
        }
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::other(
            "not a regular file (symlinks are not followed)",
        ));
    }
    if metadata.len() > max_bytes as u64 {
        return Err(std::io::Error::other(format!(
            "larger than {max_bytes} bytes"
        )));
    }
    fs::read_to_string(path).map(Some)
}

/// Replaces `dir/name` with `contents` atomically, creating `dir` (one
/// level) if needed. Refuses a symlinked `dir` or a non-file at the target.
pub fn write_atomically(dir: &Path, name: &str, contents: &str) -> std::io::Result<()> {
    if !is_real_dir(dir)? {
        fs::create_dir(dir)?;
    }
    let path = dir.join(name);
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(std::io::Error::other(
            "not a regular file (symlinks are not followed)",
        ));
    }
    // A name of our own, so nothing already in `dir` is touched.
    let temp = dir.join(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    // Fails, touching nothing, if something is already there.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let replaced = written.and_then(|()| fs::rename(&temp, &path));
    if replaced.is_err() {
        // The temp file is ours. The write's own error is the one worth
        // reporting; if the temp can't be removed either, it is left for
        // the user to see.
        let _ = fs::remove_file(&temp);
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_regular_files_and_reports_missing_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.toml");
        assert!(read_regular_file(&path, 100).expect("read").is_none());
        std::fs::write(&path, "x = 1").expect("write");
        assert_eq!(
            read_regular_file(&path, 100).expect("read").as_deref(),
            Some("x = 1")
        );
        assert!(read_regular_file(&path, 2).is_err());
    }

    #[test]
    fn writes_atomically_creating_the_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join(".srcwranglr");
        write_atomically(&sub, "a.toml", "one").expect("write");
        write_atomically(&sub, "a.toml", "two").expect("write");
        assert_eq!(
            std::fs::read_to_string(sub.join("a.toml")).expect("read"),
            "two"
        );
        assert!(!sub.join("a.toml.tmp").exists());
    }

    #[test]
    fn atomic_writes_leave_other_files_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.toml.tmp"), "mine").expect("write");
        write_atomically(dir.path(), "a.toml", "x").expect("write");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.toml.tmp")).expect("read"),
            "mine"
        );
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.toml", "a.toml.tmp"]);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks() {
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(elsewhere.path(), &linked).expect("symlink");
        assert!(write_atomically(&linked, "a.toml", "x").is_err());
        assert!(!elsewhere.path().join("a.toml").exists());
        let target = elsewhere.path().join("secret");
        std::fs::write(&target, "s").expect("write");
        let link = dir.path().join("link.toml");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(read_regular_file(&link, 100).is_err());
        // Nor through a symlinked folder.
        std::fs::write(elsewhere.path().join("a.toml"), "x").expect("write");
        assert!(read_regular_file(&linked.join("a.toml"), 100).is_err());
    }
}
