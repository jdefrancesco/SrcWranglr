//! The two walks over the project folder: one over the source the project
//! tracks (honoring `.gitignore`, like the indexer) for languages and line
//! counts, and one over everything on disk for sizes and binaries, since
//! build outputs are usually git-ignored. Symlinks are never followed.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::binary::{self, BinaryReport, Inspection, MAGIC_BYTES};
use super::languages::{self, Comments, LineCounts};
use super::{platforms, project_files, LanguageStats};

/// Entries visited per walk before giving up (huge trees, loops).
const MAX_ENTRIES: usize = 400_000;
/// Larger text files are counted as files but not read for lines.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
/// Larger binaries are listed as too large to inspect.
const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARIES: usize = 500;
/// Version-control metadata, never counted.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn"];
/// Folders whose binaries are tool scratch (CMake's compiler probes).
const SCRATCH_DIRS: &[&str] = &["CMakeFiles"];

pub(super) const CANCELLED: &str = "Cancelled";

/// What the tracked source says.
#[derive(Debug, Default)]
pub(super) struct SourceStats {
    pub languages: Vec<LanguageStats>,
    pub platforms: Vec<(&'static str, usize)>,
    pub build_systems: Vec<(&'static str, usize)>,
    pub warnings: Vec<String>,
}

/// What is on disk.
#[derive(Debug, Default)]
pub(super) struct DiskStats {
    pub file_count: usize,
    pub total_bytes: u64,
    pub binaries: Vec<BinaryReport>,
    pub unreadable_binaries: Vec<(String, String)>,
    pub object_files: usize,
    pub warnings: Vec<String>,
}

pub(super) fn source_stats(root: &Path, cancel: &AtomicBool) -> Result<SourceStats, String> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .follow_links(false)
        .require_git(false)
        .build();
    let mut by_language: BTreeMap<&'static str, LanguageStats> = BTreeMap::new();
    let mut platform_files: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut build_files: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut warnings = Vec::new();
    for (visited, entry) in walker.enumerate() {
        check(cancel)?;
        if visited >= MAX_ENTRIES {
            warnings.push(format!(
                "Stopped counting source after {MAX_ENTRIES} entries."
            ));
            break;
        }
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy();
        if let Some(system) = project_files::build_system_of(&name) {
            *build_files.entry(system).or_default() += 1;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(language) = languages::language_of(entry.path()) else {
            continue;
        };
        let text = read_text(entry.path());
        let lines = text.as_deref().map_or_else(LineCounts::default, |t| {
            languages::count_lines(t, language.comments)
        });
        if let Some(text) = text.as_deref().filter(|_| is_c_family(language.comments)) {
            for platform in platforms::platforms_in(text) {
                *platform_files.entry(platform).or_default() += 1;
            }
        }
        let stats = by_language.entry(language.name).or_insert(LanguageStats {
            name: language.name,
            kind: language.kind,
            files: 0,
            lines: LineCounts::default(),
        });
        stats.files += 1;
        stats.lines = stats.lines.plus(lines);
    }
    Ok(SourceStats {
        languages: sorted_languages(by_language.into_values().collect()),
        platforms: sorted_platforms(platform_files),
        build_systems: by_count(build_files),
        warnings,
    })
}

fn is_c_family(comments: Comments) -> bool {
    matches!(comments, Comments::CStyle | Comments::Assembly)
}

/// Code first, by code lines; then build files, docs and data.
fn sorted_languages(mut languages: Vec<LanguageStats>) -> Vec<LanguageStats> {
    languages.sort_by(|a, b| {
        (a.kind, std::cmp::Reverse(a.lines.code), a.name).cmp(&(
            b.kind,
            std::cmp::Reverse(b.lines.code),
            b.name,
        ))
    });
    languages
}

fn sorted_platforms(counts: BTreeMap<&'static str, usize>) -> Vec<(&'static str, usize)> {
    let mut platforms: Vec<_> = counts.into_iter().collect();
    platforms.sort_by_key(|(name, _)| platforms::order_of(name));
    platforms
}

fn by_count(counts: BTreeMap<&'static str, usize>) -> Vec<(&'static str, usize)> {
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    list
}

pub(super) fn disk_stats(root: &Path, cancel: &AtomicBool) -> Result<DiskStats, String> {
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && VCS_DIRS.iter().any(|d| entry.file_name() == *d))
        })
        .build();
    let mut stats = DiskStats::default();
    for (visited, entry) in walker.enumerate() {
        check(cancel)?;
        if visited >= MAX_ENTRIES {
            stats.warnings.push(format!(
                "Stopped scanning the folder after {MAX_ENTRIES} entries."
            ));
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let size = entry.metadata().map_or(0, |m| m.len());
        stats.file_count += 1;
        stats.total_bytes += size;
        let path = relative(root, entry.path());
        let is_scratch = path.split('/').any(|part| SCRATCH_DIRS.contains(&part));
        let is_binary = || {
            read_prefix(entry.path(), MAGIC_BYTES as u64)
                .is_ok_and(|m| binary::looks_like_binary(&m))
        };
        if !is_scratch && is_binary() {
            add_binary(&mut stats, &path, entry.path(), size);
        }
    }
    stats.binaries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(stats)
}

fn add_binary(stats: &mut DiskStats, path: &str, full_path: &Path, size: u64) {
    if stats.binaries.len() >= MAX_BINARIES {
        if stats.binaries.len() == MAX_BINARIES {
            stats.warnings.push(format!(
                "Only the first {MAX_BINARIES} binaries were inspected."
            ));
        }
        return;
    }
    if size > MAX_BINARY_BYTES {
        let why = format!("{} is too large to inspect", format_bytes(size));
        stats.unreadable_binaries.push((path.to_string(), why));
        return;
    }
    let inspection = match read_prefix(full_path, MAX_BINARY_BYTES) {
        Ok(bytes) => binary::inspect(path, &bytes),
        Err(error) => Inspection::Unreadable(error.to_string()),
    };
    match inspection {
        Inspection::Binaries(reports) => stats.binaries.extend(reports),
        Inspection::Object => stats.object_files += 1,
        Inspection::Unreadable(why) => stats.unreadable_binaries.push((path.to_string(), why)),
    }
}

fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

/// Up to `max` bytes from the start of `path`, if it is a regular file
/// (a symlink is refused rather than followed).
pub(super) fn read_prefix(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(max).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A text file's contents (invalid UTF-8 replaced), unless it is too big.
fn read_text(path: &Path) -> Option<String> {
    let size = std::fs::symlink_metadata(path).ok()?.len();
    if size > MAX_TEXT_BYTES {
        return None;
    }
    let bytes = read_prefix(path, MAX_TEXT_BYTES).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Project-relative path with `/` separators.
pub(super) fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `1.4 MB`, `312 KB`, `90 bytes`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, contents: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn counts_tracked_source_by_language_and_platform() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "src/a.c",
            b"#include <windows.h>\nint a;\n// note\n\n",
        );
        write(dir.path(), "src/b.c", b"#ifdef __linux__\nint b;\n#endif\n");
        write(dir.path(), "include/a.h", b"int a;\n");
        write(dir.path(), "CMakeLists.txt", b"project(x)\n");
        write(dir.path(), ".gitignore", b"build/\n");
        write(dir.path(), "build/gen.c", b"int generated;\n");
        let stats = source_stats(dir.path(), &AtomicBool::new(false)).expect("stats");
        let summary: Vec<(&str, usize, u64)> = stats
            .languages
            .iter()
            .map(|l| (l.name, l.files, l.lines.code))
            .collect();
        assert_eq!(
            summary,
            vec![("C", 2, 5), ("C/C++ header", 1, 1), ("CMake", 1, 1)]
        );
        assert_eq!(stats.platforms, vec![("Windows", 1), ("Linux", 1)]);
        assert_eq!(stats.build_systems, vec![("CMake", 1)]);
    }

    #[test]
    fn finds_binaries_even_in_ignored_build_folders() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), ".gitignore", b"build/\n");
        let mut not_elf = b"\x7fELF\x02\x01\x01".to_vec();
        not_elf.resize(64, 0xff);
        write(dir.path(), "build/broken", &not_elf);
        write(dir.path(), "src/a.c", b"int a;\n");
        write(dir.path(), ".git/objects/x", b"\x7fELF ignored");
        write(dir.path(), "build/CMakeFiles/3.28/a.out", &not_elf);
        let stats = disk_stats(dir.path(), &AtomicBool::new(false)).expect("stats");
        assert_eq!(stats.file_count, 4);
        assert_eq!(stats.unreadable_binaries.len(), 1);
        assert_eq!(stats.unreadable_binaries[0].0, "build/broken");
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks() {
        let outside = tempfile::tempdir().expect("outside");
        write(outside.path(), "secret.c", b"int secret;\n");
        let dir = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(outside.path().join("secret.c"), dir.path().join("link.c"))
            .expect("symlink");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked_dir")).expect("symlink");
        let sources = source_stats(dir.path(), &AtomicBool::new(false)).expect("stats");
        assert!(sources.languages.is_empty());
        let disk = disk_stats(dir.path(), &AtomicBool::new(false)).expect("stats");
        assert_eq!(disk.file_count, 0);
        assert!(read_prefix(&dir.path().join("link.c"), 10).is_err());
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.c", b"int a;\n");
        let cancel = AtomicBool::new(true);
        assert_eq!(
            source_stats(dir.path(), &cancel).err().as_deref(),
            Some(CANCELLED)
        );
        assert_eq!(
            disk_stats(dir.path(), &cancel).err().as_deref(),
            Some(CANCELLED)
        );
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(90), "90 bytes");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(300 * 1024 * 1024), "300 MB");
    }
}
