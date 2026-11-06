//! Finding the programs SrcWranglr runs (semgrep, cppcheck, flawfinder,
//! your editor): the folders you add in Settings first, then `PATH`, then
//! the usual install places. An app opened from the Dock or Finder gets a
//! short `PATH` without Homebrew's or pip's folders, so a tool that works
//! in a terminal wouldn't be found otherwise.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where package managers put programs, searched after `PATH`.
const USUAL_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/opt/local/bin",
    "/home/linuxbrew/.linuxbrew/bin",
    "/snap/bin",
    "/usr/bin",
    "/bin",
];
/// The same, under your home folder (pipx, pip --user, cargo).
const USUAL_HOME_DIRS: &[&str] = &[".local/bin", ".cargo/bin", "bin"];
/// pip's per-version `--user` folders on macOS: `~/Library/Python/3.12/bin`.
const MAC_PYTHON_DIR: &str = "Library/Python";

/// The folders searched for programs, in order, each once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPath {
    dirs: Vec<PathBuf>,
}

impl SearchPath {
    /// `extra` folders, then `path_var`'s, then the usual install places
    /// (some under `home`). Relative folders are left out: they would
    /// depend on the folder the app happens to run in.
    pub fn new(extra: &[PathBuf], path_var: Option<OsString>, home: Option<&Path>) -> Self {
        let from_path = path_var
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default();
        let usual = USUAL_DIRS.iter().map(PathBuf::from);
        let home_dirs = home
            .map(|home| {
                let python = python_user_dirs(&home.join(MAC_PYTHON_DIR));
                USUAL_HOME_DIRS
                    .iter()
                    .map(|d| home.join(d))
                    .chain(python)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let candidates = extra
            .iter()
            .cloned()
            .chain(from_path)
            .chain(usual)
            .chain(home_dirs);
        for dir in candidates.filter(|d| d.is_absolute() && fits_in_path(d)) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        Self { dirs }
    }

    /// The search for this process: `extra`, then its `PATH`, then the
    /// usual places.
    pub fn from_process(extra: &[PathBuf]) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        Self::new(extra, std::env::var_os("PATH"), home.as_deref())
    }

    /// Where `program` is: the first folder holding it as an executable
    /// file. A `program` with a folder in it is taken as is, if it is one.
    pub fn find(&self, program: &str) -> Option<PathBuf> {
        if program.is_empty() {
            return None;
        }
        let as_written = Path::new(program);
        if as_written.components().count() > 1 {
            return as_written
                .is_absolute()
                .then(|| as_written.to_path_buf())
                .filter(|p| is_executable(p));
        }
        self.dirs
            .iter()
            .flat_map(|dir| names_of(program).into_iter().map(move |n| dir.join(n)))
            .find(|candidate| is_executable(candidate))
    }

    /// The folders, joined as a `PATH` value for the programs started, so
    /// they find what they run in turn (a script's interpreter, say).
    pub fn joined(&self) -> OsString {
        std::env::join_paths(&self.dirs).unwrap_or_default()
    }
}

/// Whether `dir` can be part of a `PATH` value: it has no `:` (`;` on
/// Windows) in it, which would split it in two.
pub fn fits_in_path(dir: &Path) -> bool {
    std::env::join_paths([dir]).is_ok()
}

/// `~/Library/Python/*/bin`, newest version first.
fn python_user_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path().join("bin"))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs
}

/// The file names `program` may have: on Windows, also with the usual
/// extensions (`python3.12` is `python3.12.exe`, not a `.12` file).
fn names_of(program: &str) -> Vec<String> {
    let with_extensions = ["exe", "cmd", "bat", "com"]
        .iter()
        .filter(|_| cfg!(windows))
        .map(|ext| format!("{program}.{ext}"));
    std::iter::once(program.to_string())
        .chain(with_extensions)
        .collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn program(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    #[test]
    fn finds_a_program_in_your_folders_before_path() {
        let (mine, on_path) = (
            tempfile::tempdir().expect("dir"),
            tempfile::tempdir().expect("dir"),
        );
        let ours = program(mine.path(), "semgrep", 0o755);
        program(on_path.path(), "semgrep", 0o755);
        let path_var = std::env::join_paths([on_path.path()]).ok();
        let search = SearchPath::new(&[mine.path().to_path_buf()], path_var, None);
        assert_eq!(search.find("semgrep"), Some(ours));
    }

    #[test]
    fn finds_it_in_the_usual_places_under_home_when_path_is_short() {
        let home = tempfile::tempdir().expect("dir");
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        let tool = program(&bin, "srcwranglr-test-tool", 0o755);
        let search = SearchPath::new(&[], Some("/usr/bin:/bin".into()), Some(home.path()));
        assert_eq!(search.find("srcwranglr-test-tool"), Some(tool));
    }

    #[test]
    fn skips_files_that_cannot_run_and_relative_folders() {
        let dir = tempfile::tempdir().expect("dir");
        program(dir.path(), "cppcheck-not-executable", 0o644);
        let search = SearchPath::new(&[dir.path().to_path_buf(), "relative".into()], None, None);
        assert_eq!(search.find("cppcheck-not-executable"), None);
        assert!(!search.dirs.contains(&PathBuf::from("relative")));
        assert_eq!(search.find(""), None);
    }

    #[test]
    fn a_path_with_folders_is_taken_as_is() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = program(dir.path(), "tool", 0o755);
        let search = SearchPath::new(&[], None, None);
        assert_eq!(search.find(tool.to_str().expect("utf-8")), Some(tool));
        assert_eq!(search.find("bin/tool"), None);
    }

    #[test]
    fn a_folder_that_would_split_path_is_left_out() {
        let dir = tempfile::tempdir().expect("dir");
        let search = SearchPath::new(
            &["/a:b".into()],
            std::env::join_paths([dir.path()]).ok(),
            None,
        );
        assert!(!search.dirs.contains(&PathBuf::from("/a:b")));
        let joined = search.joined();
        assert!(std::env::split_paths(&joined).any(|d| d == dir.path()));
    }

    #[test]
    fn each_folder_is_searched_once_and_joined_for_children() {
        let dir = tempfile::tempdir().expect("dir");
        let path_var = std::env::join_paths([dir.path(), dir.path()]).ok();
        let search = SearchPath::new(&[dir.path().to_path_buf()], path_var, None);
        let joined = search.joined();
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs.iter().filter(|d| *d == dir.path()).count(), 1);
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
    }
}
