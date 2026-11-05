//! Project triage: a first look at an unfamiliar codebase. Its languages
//! and size, how it is built, which platforms it targets, where it lives
//! online, and which compiled binaries sit in the folder and how they were
//! hardened. Everything is read from disk; nothing is run or fetched.
//! (Not to be confused with `notes::triage`, your verdicts on findings.)

mod binary;
mod languages;
mod links;
mod platforms;
mod project_files;
mod scan;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::safe_fs;

#[cfg(test)]
pub use binary::Check;
pub use binary::{BinaryReport, Status};
pub use languages::{Kind, LineCounts};
pub use links::{github_search_url, RepoLink};
pub use scan::format_bytes;

/// Top-level files read for repository links, and how much of each.
const MAX_LINK_FILES: usize = 30;
const MAX_LINK_FILE_BYTES: usize = 1024 * 1024;
const MAX_BRANCH_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageStats {
    pub name: &'static str,
    pub kind: Kind,
    pub files: usize,
    pub lines: LineCounts,
}

#[derive(Debug, Default)]
pub struct Survey {
    /// Every file in the folder except version-control metadata.
    pub file_count: usize,
    pub total_bytes: u64,
    /// Tracked (not git-ignored) files by language: code first.
    pub languages: Vec<LanguageStats>,
    /// Platforms the C/C++ source targets, with how many files point to each.
    pub platforms: Vec<(&'static str, usize)>,
    /// Build systems, with how many of their files there are.
    pub build_systems: Vec<(&'static str, usize)>,
    /// Top-level documents worth reading first.
    pub docs: Vec<String>,
    pub license: Option<&'static str>,
    pub ci: Vec<&'static str>,
    /// The checked-out git branch (or commit), if this is a git checkout.
    pub branch: Option<String>,
    pub links: Vec<RepoLink>,
    pub binaries: Vec<BinaryReport>,
    /// Binaries that couldn't be inspected, and why.
    pub unreadable_binaries: Vec<(String, String)>,
    /// Object files and static archives (not listed one by one).
    pub object_files: usize,
    pub warnings: Vec<String>,
}

impl Survey {
    /// Lines of code (not comments or blanks) in programming languages.
    pub fn code_lines(&self) -> LineCounts {
        self.languages
            .iter()
            .filter(|l| l.kind == Kind::Code)
            .fold(LineCounts::default(), |sum, l| sum.plus(l.lines))
    }
}

/// Surveys the project folder at `root`. Checks `cancel` between files.
pub fn survey(root: &Path, cancel: &AtomicBool) -> Result<Survey, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let sources = scan::source_stats(root, cancel)?;
    let disk = scan::disk_stats(root, cancel)?;
    let top = top_level(root);
    Ok(Survey {
        file_count: disk.file_count,
        total_bytes: disk.total_bytes,
        languages: sources.languages,
        platforms: sources.platforms,
        build_systems: sources.build_systems,
        docs: top.docs,
        license: top.license,
        ci: top.ci,
        branch: top.branch,
        links: top.links,
        binaries: disk.binaries,
        unreadable_binaries: disk.unreadable_binaries,
        object_files: disk.object_files,
        warnings: sources.warnings.into_iter().chain(disk.warnings).collect(),
    })
}

#[derive(Debug, Default)]
struct TopLevel {
    docs: Vec<String>,
    license: Option<&'static str>,
    ci: Vec<&'static str>,
    branch: Option<String>,
    links: Vec<RepoLink>,
}

/// What the project's top-level files and git metadata say.
fn top_level(root: &Path) -> TopLevel {
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(root)
        .map(|dir| {
            dir.flatten()
                .filter_map(|e| {
                    let file_type = e.file_type().ok()?;
                    let is_real = file_type.is_file() || file_type.is_dir();
                    is_real.then(|| {
                        (
                            e.file_name().to_string_lossy().into_owned(),
                            file_type.is_dir(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    let read = |name: &str| {
        safe_fs::read_regular_file(&root.join(name), MAX_LINK_FILE_BYTES)
            .ok()
            .flatten()
    };
    let docs: Vec<String> = entries
        .iter()
        .filter(|(name, is_dir)| project_files::is_top_level_doc(name, *is_dir))
        .map(|(name, is_dir)| {
            if *is_dir {
                format!("{name}/")
            } else {
                name.clone()
            }
        })
        .collect();
    let license = entries
        .iter()
        .filter(|(name, is_dir)| !is_dir && project_files::is_license_file(name))
        .find_map(|(name, _)| project_files::identify_license(&read(name)?));
    let mut ci: Vec<&'static str> = entries
        .iter()
        .filter_map(|(n, _)| project_files::ci_of(n))
        .collect();
    ci.dedup();
    TopLevel {
        docs,
        license,
        ci,
        branch: git_branch(root),
        links: repo_links(root, &entries, read),
    }
}

fn repo_links(
    root: &Path,
    entries: &[(String, bool)],
    read: impl Fn(&str) -> Option<String>,
) -> Vec<RepoLink> {
    let mut links = links::LinkCollector::default();
    if safe_fs::is_real_dir(&root.join(".git")).unwrap_or(false) {
        if let Some(config) = read(".git/config") {
            links.add_git_config(&config, false);
        }
    }
    if let Some(modules) = read(".gitmodules") {
        links.add_git_config(&modules, true);
    }
    let mentioning = entries.iter().filter(|(name, is_dir)| {
        !is_dir
            && !project_files::is_license_file(name)
            && (project_files::is_top_level_doc(name, false)
                || project_files::build_system_of(name).is_some()
                || name.ends_with(".md")
                || name.ends_with(".podspec"))
    });
    for (name, _) in mentioning.take(MAX_LINK_FILES) {
        if let Some(text) = read(name) {
            links.add_mentions(&text, name);
        }
    }
    links.finish()
}

/// The branch named in `.git/HEAD`, or the commit a detached HEAD is at.
fn git_branch(root: &Path) -> Option<String> {
    if !safe_fs::is_real_dir(&root.join(".git")).unwrap_or(false) {
        return None;
    }
    let head = safe_fs::read_regular_file(&root.join(".git/HEAD"), 4096).ok()??;
    let head = head.trim();
    let branch = match head.strip_prefix("ref:") {
        Some(reference) => {
            let reference = reference.trim();
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string()
        }
        None => format!("detached at {}", head.get(..12).unwrap_or(head)),
    };
    let is_printable = !branch.chars().any(char::is_control) && branch.len() <= MAX_BRANCH_CHARS;
    is_printable.then_some(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn surveys_a_small_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(
            root,
            "src/main.c",
            "#include <unistd.h>\nint main(void) { return 0; }\n",
        );
        write(
            root,
            "README.md",
            "Mirror of https://github.com/acme/widget\n",
        );
        write(
            root,
            "LICENSE",
            "MIT License\nPermission is hereby granted, free of charge, to any person\n",
        );
        write(root, "Makefile", "all:\n\tcc src/main.c\n");
        write(root, ".git/HEAD", "ref: refs/heads/main\n");
        write(
            root,
            ".git/config",
            "[remote \"origin\"]\n\turl = git@github.com:acme/widget.git\n",
        );
        write(root, ".github/workflows/ci.yml", "on: push\n");
        write(root, "docs/guide.md", "# Guide\n");

        let survey = survey(root, &AtomicBool::new(false)).expect("survey");
        assert_eq!(survey.code_lines().code, 2);
        assert_eq!(survey.platforms, vec![("POSIX", 1)]);
        assert_eq!(survey.build_systems, vec![("Make", 1)]);
        assert_eq!(survey.docs, vec!["LICENSE", "README.md", "docs/"]);
        assert_eq!(survey.license, Some("MIT"));
        assert_eq!(survey.ci, vec!["GitHub Actions"]);
        assert_eq!(survey.branch.as_deref(), Some("main"));
        assert_eq!(survey.links.len(), 1);
        assert_eq!(survey.links[0].url, "https://github.com/acme/widget");
        assert_eq!(
            survey.links[0].sources,
            vec!["git remote \"origin\"", "README.md"]
        );
        assert!(survey.binaries.is_empty() && survey.file_count == 6);
    }

    #[test]
    fn reports_a_detached_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            ".git/HEAD",
            "0123456789abcdef0123456789abcdef01234567\n",
        );
        assert_eq!(
            git_branch(dir.path()).as_deref(),
            Some("detached at 0123456789ab")
        );
    }

    /// Manual check against a real project:
    /// `SLURCETRAIL_SURVEY_PATH=/path cargo test survey_real_project -- --ignored --nocapture`
    #[test]
    #[ignore = "needs SLURCETRAIL_SURVEY_PATH"]
    fn survey_real_project() {
        let Some(root) = std::env::var_os("SLURCETRAIL_SURVEY_PATH") else {
            return;
        };
        let start = std::time::Instant::now();
        let survey = survey(Path::new(&root), &AtomicBool::new(false)).expect("survey");
        println!(
            "surveyed in {:?}: {} files, {}",
            start.elapsed(),
            survey.file_count,
            format_bytes(survey.total_bytes)
        );
        for l in &survey.languages {
            println!(
                "  {:<16} {:>5} files {:>8} code {:>7} comments {:>6} blank",
                l.name, l.files, l.lines.code, l.lines.comments, l.lines.blank
            );
        }
        println!(
            "platforms {:?}\nbuild {:?}\ndocs {:?} license {:?} ci {:?} branch {:?}",
            survey.platforms,
            survey.build_systems,
            survey.docs,
            survey.license,
            survey.ci,
            survey.branch
        );
        for link in &survey.links {
            println!("  link {} {:?}", link.url, link.sources);
        }
        for b in &survey.binaries {
            println!(
                "  bin {} {} {} {} {} | {}",
                b.path,
                format_bytes(b.size),
                b.format,
                b.arch,
                b.kind,
                b.symbols
            );
            for c in &b.checks {
                println!("      {:?} {}: {}", c.status, c.name, c.detail);
            }
            println!(
                "      libs {:?}\n      caps {:?}",
                b.libraries, b.capabilities
            );
        }
        println!(
            "unreadable {:?} objects {} warnings {:?}",
            survey.unreadable_binaries, survey.object_files, survey.warnings
        );
    }

    #[test]
    fn refuses_a_missing_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(survey(&dir.path().join("nope"), &AtomicBool::new(false)).is_err());
    }
}
