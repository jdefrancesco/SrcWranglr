//! Whether you trust a project's own `.srcwranglr` verdicts,
//! dangerous-functions list and semgrep rules. A hostile repository could
//! ship verdicts that hide its bugs, a list that silences them, or rules
//! that bury them in noise, so being there isn't
//! enough: the decision lives in your config, in `trusted-projects.txt`
//! (one project folder per line).

use std::path::{Path, PathBuf};

use super::dangerous_lists::{config_dir, write_user_file, LIST_FILE};
use super::rule_files::PROJECT_RULES_DIR;
use crate::notes::store::NOTES_DIR;
use crate::notes::triage::TRIAGE_FILE;

const TRUST_FILE: &str = "trusted-projects.txt";

/// Whether you've trusted `root`'s own files.
pub fn is_trusted(root: &Path) -> bool {
    trust_file().is_some_and(|file| is_trusted_in(&file, root))
}

/// Records that you trust `root`'s own files.
pub fn trust(root: &Path) -> Result<(), String> {
    let file = trust_file().ok_or("No home folder to remember trusted projects in")?;
    trust_in(&file, root)
}

/// Whether `root` has verdicts, a dangerous list or semgrep rules of its
/// own.
pub fn has_project_files(root: &Path) -> bool {
    let dir = root.join(NOTES_DIR);
    [TRIAGE_FILE, LIST_FILE, PROJECT_RULES_DIR]
        .iter()
        .any(|name| std::fs::symlink_metadata(dir.join(name)).is_ok())
}

fn trust_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(TRUST_FILE))
}

/// `root` as it is recorded: its canonical path.
fn key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn is_trusted_in(file: &Path, root: &Path) -> bool {
    let key = key(root);
    std::fs::read_to_string(file)
        .map(|text| text.lines().any(|line| line.trim() == key))
        .unwrap_or(false)
}

fn trust_in(file: &Path, root: &Path) -> Result<(), String> {
    if is_trusted_in(file, root) {
        return Ok(());
    }
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let updated = format!("{existing}{separator}{}\n", key(root));
    write_user_file(file, &updated)
        .map(|_| ())
        .map_err(|e| format!("Could not update {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusting_is_remembered_once_per_project() {
        let config = tempfile::tempdir().expect("tmp");
        let (a, b) = (
            tempfile::tempdir().expect("tmp"),
            tempfile::tempdir().expect("tmp"),
        );
        let file = config.path().join("srcwranglr").join(TRUST_FILE);
        assert!(!is_trusted_in(&file, a.path()));
        trust_in(&file, a.path()).expect("trust");
        trust_in(&file, a.path()).expect("again");
        assert!(is_trusted_in(&file, a.path()));
        assert!(!is_trusted_in(&file, b.path()));
        let text = std::fs::read_to_string(&file).expect("read");
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn projects_with_their_own_verdicts_or_list_are_noticed() {
        let project = tempfile::tempdir().expect("tmp");
        assert!(!has_project_files(project.path()));
        let dir = project.path().join(NOTES_DIR);
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::write(dir.join("notes.json"), "{}").expect("write");
        assert!(
            !has_project_files(project.path()),
            "notes don't hide findings"
        );
        std::fs::write(dir.join(TRIAGE_FILE), "{}").expect("write");
        assert!(has_project_files(project.path()));
    }

    #[test]
    fn a_project_with_its_own_semgrep_rules_is_noticed() {
        let project = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(project.path().join(NOTES_DIR).join(PROJECT_RULES_DIR))
            .expect("mkdir");
        assert!(has_project_files(project.path()));
    }
}
