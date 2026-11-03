//! Finds the C/C++ source files of a project directory.
//!
//! Honors `.gitignore`/`.ignore` files (even outside a git checkout), skips
//! hidden entries and well-known dependency/output directories, and returns
//! paths sorted so indexing is deterministic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::IndexError;

pub const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "h++", "inl", "ipp", "tpp",
];

/// Directories that are almost never first-party source.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target"];

#[derive(Debug, Default)]
pub struct Discovery {
    pub files: Vec<PathBuf>,
    /// Non-fatal problems (unreadable entries, truncation) to show the user.
    pub warnings: Vec<String>,
}

pub fn discover_sources(
    root: &Path,
    max_files: usize,
    cancel: &AtomicBool,
) -> Result<Discovery, IndexError> {
    if !root.is_dir() {
        return Err(IndexError::NotADirectory(root.to_path_buf()));
    }
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .follow_links(false)
        .require_git(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && SKIPPED_DIRS.iter().any(|d| entry.file_name() == *d))
        })
        .build();

    let mut warnings = Vec::new();
    let mut files = Vec::new();
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return Err(IndexError::Cancelled);
        }
        match entry {
            Ok(e) if e.file_type().is_some_and(|t| t.is_file()) && is_source_file(e.path()) => {
                files.push(e.into_path());
            }
            Ok(_) => {}
            Err(err) => warnings.push(format!("Skipped an entry while scanning: {err}")),
        }
    }
    files.sort();

    if files.len() > max_files {
        warnings.push(format!(
            "Found {} source files; only the first {max_files} were indexed.",
            files.len()
        ));
        files.truncate(max_files);
    }
    Ok(Discovery { files, warnings })
}

pub fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            SOURCE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static NOT_CANCELLED: AtomicBool = AtomicBool::new(false);

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "a.c");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            discover_sources(dir.path(), 10, &cancelled),
            Err(IndexError::Cancelled)
        ));
    }

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, "int x;\n").expect("write");
    }

    fn rel_names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn finds_sources_sorted_and_skips_everything_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for rel in [
            "src/main.cpp",
            "include/a.hpp",
            "b.h",
            "README.md",
            "ignored/gen.cpp",
            ".hidden/x.cpp",
            "node_modules/dep/y.cc",
        ] {
            touch(root, rel);
        }
        fs::write(root.join(".gitignore"), "ignored/\n").expect("gitignore");

        let found = discover_sources(root, 100, &NOT_CANCELLED).expect("discover");
        assert_eq!(
            rel_names(root, &found.files),
            ["b.h", "include/a.hpp", "src/main.cpp"]
        );
        assert!(found.warnings.is_empty());
    }

    #[test]
    fn truncates_with_warning_past_max_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            touch(dir.path(), &format!("f{i}.c"));
        }
        let found = discover_sources(dir.path(), 3, &NOT_CANCELLED).expect("discover");
        assert_eq!(found.files.len(), 3);
        assert_eq!(found.warnings.len(), 1);
    }

    #[test]
    fn rejects_non_directory_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.cpp");
        fs::write(&file, "").expect("write");
        assert!(matches!(
            discover_sources(&file, 10, &NOT_CANCELLED),
            Err(IndexError::NotADirectory(_))
        ));
        assert!(matches!(
            discover_sources(&dir.path().join("missing"), 10, &NOT_CANCELLED),
            Err(IndexError::NotADirectory(_))
        ));
    }

    #[test]
    fn recognizes_source_extensions_case_insensitively() {
        assert!(is_source_file(Path::new("a/B.CPP")));
        assert!(is_source_file(Path::new("x.h")));
        assert!(!is_source_file(Path::new("Makefile")));
        assert!(!is_source_file(Path::new("notes.txt")));
    }
}
