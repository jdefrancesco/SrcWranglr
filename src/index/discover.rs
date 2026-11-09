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

/// Directories that are almost never first-party source. Matched by name at
/// any depth, so only list names no C/C++ project uses for its own code.
const SKIPPED_DIRS: &[&str] = &["node_modules"];

/// Cargo's build output, recognized by the `Cargo.toml` beside it.
///
/// `target` used to sit in `SKIPPED_DIRS`, so any folder with that name was
/// skipped. C projects use the name for real source: on the Linux kernel that
/// silently dropped `drivers/target` (the SCSI target subsystem),
/// `drivers/nvme/target` and `include/target`, about 80 files.
fn is_cargo_target_dir(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "target")
        && path
            .parent()
            .is_some_and(|parent| parent.join("Cargo.toml").is_file())
}

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
            !(is_dir
                && (SKIPPED_DIRS.iter().any(|d| entry.file_name() == *d)
                    || is_cargo_target_dir(entry.path())))
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

    // Truncating after the sort keeps an alphabetical prefix, so a limit that
    // is too small drops whole directories from the end of the alphabet
    // rather than a scattering of files. That is how a 20,000 limit lost most
    // of Linux: everything after `drivers/gpu/drm/nouveau/...`, including
    // `fs/`, `kernel/`, `mm/` and `net/`. See `IndexOptions::default`.
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
    fn skips_cargo_target_but_keeps_source_dirs_named_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // Linux keeps real sources in `drivers/target` and `include/target`.
        for rel in [
            "Cargo.toml",
            "target/debug/build/gen.c",
            "drivers/target/target_core.c",
            "drivers/nvme/target/core.c",
            "include/target/target_core_base.h",
        ] {
            touch(root, rel);
        }

        let found = discover_sources(root, 100, &NOT_CANCELLED).expect("discover");
        assert_eq!(
            rel_names(root, &found.files),
            [
                "drivers/nvme/target/core.c",
                "drivers/target/target_core.c",
                "include/target/target_core_base.h",
            ]
        );
    }

    #[test]
    fn keeps_top_level_target_dir_outside_a_cargo_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "target/main.c");
        let found = discover_sources(dir.path(), 100, &NOT_CANCELLED).expect("discover");
        assert_eq!(rel_names(dir.path(), &found.files), ["target/main.c"]);
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
