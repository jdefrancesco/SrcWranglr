//! Where semgrep rules you write are kept: yours, used for every project,
//! in `~/.config/srcwranglr/semgrep/`; or a project's own, in its
//! `.srcwranglr/semgrep/`, used once you trust the project's files (a
//! hostile repository could ship rules too).

use std::path::{Path, PathBuf};

use super::dangerous_lists::{semgrep_rules_dir, write_user_file};
use crate::notes::store::NOTES_DIR;
use crate::safe_fs::{is_real_dir, write_atomically};

/// The project's folder of rules, inside its `.srcwranglr`.
pub const PROJECT_RULES_DIR: &str = "semgrep";
/// How many `-2`, `-3`, … names to try before giving up.
const MAX_NAME_TRIES: usize = 100;

/// Where a rule is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleHome {
    /// Yours: every project's semgrep runs use it.
    Yours,
    /// The open project's.
    Project,
}

/// `root`'s folder of rules, if it is there as a real folder (not a
/// symlink, in a `.srcwranglr` that isn't one either).
pub fn project_rules(root: &Path) -> Option<PathBuf> {
    let notes = root.join(NOTES_DIR);
    let rules = notes.join(PROJECT_RULES_DIR);
    let is_there = is_real_dir(&notes).unwrap_or(false) && is_real_dir(&rules).unwrap_or(false);
    is_there.then_some(rules)
}

/// Saves the rule `yaml` as `<id>.yaml` (or `<id>-2.yaml`, … if that is
/// taken) in `home` (`root` being the project's folder). Returns where.
pub fn save_rule(
    home: RuleHome,
    root: Option<&Path>,
    id: &str,
    yaml: &str,
) -> Result<PathBuf, String> {
    check_id(id)?;
    let saved = match home {
        RuleHome::Yours => {
            let dir = semgrep_rules_dir().ok_or("No home folder to keep your rules in")?;
            let path = free_name(&dir, id)?;
            write_user_file(&path, yaml)
        }
        RuleHome::Project => {
            let root = root.ok_or("This project isn't a folder on disk")?;
            save_in_project(root, id, yaml)
        }
    };
    saved.map_err(|e| format!("Could not save the rule: {e}"))
}

fn save_in_project(root: &Path, id: &str, yaml: &str) -> std::io::Result<PathBuf> {
    let notes = root.join(NOTES_DIR);
    if !is_real_dir(&notes)? {
        std::fs::create_dir(&notes)?;
    }
    let dir = notes.join(PROJECT_RULES_DIR);
    if !is_real_dir(&dir)? {
        std::fs::create_dir(&dir)?;
    }
    let path = free_name(&dir, id).map_err(std::io::Error::other)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::other("bad file name"))?;
    write_atomically(&dir, name, yaml)?;
    Ok(path)
}

/// Refuses an id that isn't letters, digits, `-` and `_`.
fn check_id(id: &str) -> Result<(), String> {
    let is_safe = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    match is_safe {
        true => Ok(()),
        false => Err(format!(
            "{id:?} isn't a rule id SrcWranglr can name a file after: use letters, digits and -"
        )),
    }
}

/// `dir/<id>.yaml`, or the first of `<id>-2.yaml`, … with nothing there.
fn free_name(dir: &Path, id: &str) -> Result<PathBuf, String> {
    check_id(id)?;
    (1..=MAX_NAME_TRIES)
        .map(|n| match n {
            1 => dir.join(format!("{id}.yaml")),
            n => dir.join(format!("{id}-{n}.yaml")),
        })
        .find(|path| std::fs::symlink_metadata(path).is_err())
        .ok_or_else(|| format!("Too many rules named {id} already"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_rule_is_saved_under_a_free_name() {
        let root = tempfile::tempdir().expect("dir");
        let first =
            save_rule(RuleHome::Project, Some(root.path()), "like-f-1", "a").expect("saved");
        let second =
            save_rule(RuleHome::Project, Some(root.path()), "like-f-1", "b").expect("saved");
        let dir = root.path().join(".srcwranglr/semgrep");
        assert_eq!(first, dir.join("like-f-1.yaml"));
        assert_eq!(second, dir.join("like-f-1-2.yaml"));
        assert_eq!(std::fs::read_to_string(first).expect("read"), "a");
        assert_eq!(project_rules(root.path()), Some(dir));
    }

    #[test]
    fn odd_ids_and_missing_folders_are_refused() {
        let root = tempfile::tempdir().expect("dir");
        assert!(save_rule(RuleHome::Project, Some(root.path()), "../x", "a").is_err());
        assert!(save_rule(RuleHome::Project, None, "ok", "a").is_err());
        assert_eq!(project_rules(root.path()), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_rules_folder_is_not_used_or_written_through() {
        let root = tempfile::tempdir().expect("dir");
        let elsewhere = tempfile::tempdir().expect("dir");
        std::fs::create_dir(root.path().join(".srcwranglr")).expect("mkdir");
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join(".srcwranglr/semgrep"))
            .expect("symlink");
        assert_eq!(project_rules(root.path()), None);
        assert!(save_rule(RuleHome::Project, Some(root.path()), "r", "a").is_err());
        assert_eq!(std::fs::read_dir(elsewhere.path()).expect("ls").count(), 0);
    }
}
