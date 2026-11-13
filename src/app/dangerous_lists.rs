//! Your dangerous-functions lists on disk: a global one used for every
//! project (`$XDG_CONFIG_HOME/srcwranglr/dangerous.toml`, else
//! `~/.config/…`), and the project's own in `.srcwranglr/`, whose entries
//! win. The project's file is untrusted (no symlinks, size limited); the
//! global one is yours, so a symlinked dotfile is followed.

use std::path::{Path, PathBuf};

use crate::analysis::dangerous::{with_entry, DangerousFunction, DangerousList};
use crate::notes::store::NOTES_DIR;
use crate::safe_fs::{read_regular_file, write_atomically};

pub const LIST_FILE: &str = "dangerous.toml";
const MAX_LIST_BYTES: usize = 1024 * 1024;

/// Which list an entry goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListScope {
    Global,
    Project,
}

#[derive(Debug, Default)]
pub struct DangerousLists {
    global_path: Option<PathBuf>,
    /// The project's `.srcwranglr` folder; `None` for the demo.
    project_dir: Option<PathBuf>,
    merged: DangerousList,
    /// Why a list couldn't be read (it is then left out).
    problems: Vec<String>,
    is_project_trusted: bool,
    /// "off" entries of an untrusted project list, not applied.
    ignored_project_offs: usize,
}

impl DangerousLists {
    /// The global list and, for a project on disk, its own: applied fully
    /// if `is_project_trusted`, else only as advice (it can add findings,
    /// not silence or lower them).
    pub fn load(project_root: Option<&Path>, is_project_trusted: bool) -> Self {
        Self::load_from(
            global_list_path(),
            project_root.map(|r| r.join(NOTES_DIR)),
            is_project_trusted,
        )
    }

    fn load_from(
        global_path: Option<PathBuf>,
        project_dir: Option<PathBuf>,
        is_project_trusted: bool,
    ) -> Self {
        let mut problems = Vec::new();
        let mut read = |text: std::io::Result<Option<String>>, path: &Path| {
            let parsed = text
                .map_err(|e| e.to_string())
                .and_then(|t| DangerousList::parse(&t.unwrap_or_default()));
            parsed.unwrap_or_else(|error| {
                problems.push(format!("{}: {error}", path.display()));
                DangerousList::default()
            })
        };
        let global = match &global_path {
            Some(path) => read(read_followed(path), path),
            None => DangerousList::default(),
        };
        let project = match &project_dir {
            Some(dir) => {
                let path = dir.join(LIST_FILE);
                read(read_regular_file(&path, MAX_LIST_BYTES), &path)
            }
            None => DangerousList::default(),
        };
        let (project, ignored) = match is_project_trusted {
            true => (project, 0),
            false => project.advisory(),
        };
        Self {
            merged: DangerousList::merged(&global, &project),
            global_path,
            project_dir,
            is_project_trusted,
            ignored_project_offs: ignored,
            problems,
        }
    }

    pub fn merged(&self) -> &DangerousList {
        &self.merged
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn ignored_project_offs(&self) -> usize {
        self.ignored_project_offs
    }

    pub fn has_project_list(&self) -> bool {
        self.project_dir.is_some()
    }

    /// Adds (or updates) `entry` in the `scope` list file, keeping the rest
    /// of the file, and reloads both lists. Returns the file written.
    pub fn add(&mut self, entry: &DangerousFunction, scope: ListScope) -> Result<PathBuf, String> {
        let path = match scope {
            ListScope::Global => self
                .global_path
                .clone()
                .ok_or("No home folder for the global list")?,
            ListScope::Project => self
                .project_dir
                .as_ref()
                .map(|dir| dir.join(LIST_FILE))
                .ok_or("The demo project has no folder to keep a list in")?,
        };
        let failed = |e: String| format!("Could not update {}: {e}", path.display());
        let current = match scope {
            ListScope::Global => read_followed(&path),
            ListScope::Project => read_regular_file(&path, MAX_LIST_BYTES),
        }
        .map_err(|e| failed(e.to_string()))?;
        let updated = with_entry(&current.unwrap_or_default(), entry).map_err(failed)?;
        // Never write what the next load would reject.
        if updated.len() > MAX_LIST_BYTES {
            return Err(failed(format!(
                "the list would exceed {MAX_LIST_BYTES} bytes"
            )));
        }
        DangerousList::parse(&updated).map_err(failed)?;
        let written = match scope {
            ListScope::Global => write_user_file(&path, &updated),
            ListScope::Project => {
                let dir = self.project_dir.as_deref().unwrap_or(Path::new("."));
                write_atomically(dir, LIST_FILE, &updated).map(|()| path.clone())
            }
        }
        .map_err(|e| failed(e.to_string()))?;
        *self = Self::load_from(
            self.global_path.clone(),
            self.project_dir.clone(),
            self.is_project_trusted,
        );
        Ok(written)
    }
}

/// `$XDG_CONFIG_HOME/srcwranglr`, else `~/.config/srcwranglr`.
#[cfg(not(test))]
pub fn config_dir() -> Option<PathBuf> {
    real_config_dir()
}

/// In tests, a folder of this test run's own: tests save settings, trust
/// projects and write rules, and must never do it to yours.
#[cfg(test)]
pub fn config_dir() -> Option<PathBuf> {
    let run = format!("srcwranglr-tests-{}", std::process::id());
    Some(std::env::temp_dir().join(run).join("srcwranglr"))
}

#[cfg_attr(test, allow(dead_code))]
fn real_config_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("srcwranglr"))
}

/// The global dangerous-functions list.
pub fn global_list_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(LIST_FILE))
}

/// Where your semgrep rules live (SrcWranglr's are written there too).
pub fn semgrep_rules_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("semgrep"))
}

/// Reads the user's own file, following a symlink (dotfile managers).
fn read_followed(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::canonicalize(path) {
        Ok(real) => read_regular_file(&real, MAX_LIST_BYTES),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes the user's own file; through its symlink if it is one.
pub fn write_user_file(path: &Path, contents: &str) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = real
        .parent()
        .ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let name = real
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::other("bad file name"))?;
    write_atomically(dir, name, contents)?;
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_never_touch_the_real_configuration() {
        let home = std::env::var_os("HOME").map(PathBuf::from).expect("HOME");
        let dir = config_dir().expect("a folder");
        assert!(!dir.starts_with(home.join(".config")), "{}", dir.display());
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
    }
    use crate::analysis::Severity;

    fn entry(name: &str, reason: &str) -> DangerousFunction {
        DangerousFunction {
            name: name.into(),
            severity: Some(Severity::High),
            reason: reason.into(),
            fix: None,
            cwe: None,
            is_advisory: false,
        }
    }

    fn lists(home: &Path, project: Option<&Path>) -> DangerousLists {
        DangerousLists::load_from(
            Some(home.join(".config/srcwranglr").join(LIST_FILE)),
            project.map(|p| p.join(NOTES_DIR)),
            true,
        )
    }

    #[test]
    fn entries_added_to_either_list_are_used_and_project_wins() {
        let (home, project) = (
            tempfile::tempdir().expect("tmp"),
            tempfile::tempdir().expect("tmp"),
        );
        let mut both = lists(home.path(), Some(project.path()));
        both.add(&entry("xfree", "global reason"), ListScope::Global)
            .expect("global");
        both.add(&entry("xfree", "project reason"), ListScope::Project)
            .expect("project");
        both.add(&entry("parse", "only global"), ListScope::Global)
            .expect("global");
        let reloaded = lists(home.path(), Some(project.path()));
        let merged = reloaded.merged();
        assert_eq!(
            merged.lookup("xfree").map(|e| e.reason.as_str()),
            Some("project reason")
        );
        assert!(merged.lookup("parse").is_some());
        // Another project sees only the global list.
        let other = tempfile::tempdir().expect("tmp");
        let other = lists(home.path(), Some(other.path()));
        assert_eq!(
            other.merged().lookup("xfree").map(|e| e.reason.as_str()),
            Some("global reason")
        );
    }

    #[test]
    fn an_untrusted_project_list_can_add_but_not_silence() {
        let (home, project) = (
            tempfile::tempdir().expect("tmp"),
            tempfile::tempdir().expect("tmp"),
        );
        let dir = project.path().join(NOTES_DIR);
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::write(
            dir.join(LIST_FILE),
            "[[function]]\nname = \"strcpy\"\nseverity = \"off\"\nreason = \"trust me\"\n\
             [[function]]\nname = \"parse\"\nseverity = \"high\"\nreason = \"fragile\"\n",
        )
        .expect("write");
        let untrusted =
            DangerousLists::load_from(Some(home.path().join(LIST_FILE)), Some(dir.clone()), false);
        assert_eq!(untrusted.ignored_project_offs(), 1);
        assert!(untrusted.merged().lookup("strcpy").is_none());
        assert!(untrusted
            .merged()
            .lookup("parse")
            .is_some_and(|e| e.is_advisory));
        let trusted = DangerousLists::load_from(Some(home.path().join(LIST_FILE)), Some(dir), true);
        assert_eq!(
            trusted.merged().lookup("strcpy").map(|e| e.severity),
            Some(None)
        );
    }

    #[test]
    fn a_broken_list_is_reported_and_left_out() {
        let (home, project) = (
            tempfile::tempdir().expect("tmp"),
            tempfile::tempdir().expect("tmp"),
        );
        let dir = project.path().join(NOTES_DIR);
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::write(dir.join(LIST_FILE), "[[function]]\nname = 3\n").expect("write");
        let loaded = lists(home.path(), Some(project.path()));
        assert_eq!(loaded.problems().len(), 1);
        assert!(loaded.merged().is_empty());
    }

    #[test]
    fn the_demo_has_no_project_list() {
        let home = tempfile::tempdir().expect("tmp");
        let mut demo = lists(home.path(), None);
        assert!(!demo.has_project_list());
        assert!(demo.add(&entry("f", "r"), ListScope::Project).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_global_list_is_updated_through_the_link() {
        let (home, dotfiles) = (
            tempfile::tempdir().expect("tmp"),
            tempfile::tempdir().expect("tmp"),
        );
        let real = dotfiles.path().join(LIST_FILE);
        std::fs::write(&real, "# mine\n").expect("write");
        let config = home.path().join(".config/srcwranglr");
        std::fs::create_dir_all(&config).expect("mkdir");
        std::os::unix::fs::symlink(&real, config.join(LIST_FILE)).expect("symlink");
        let mut global = lists(home.path(), None);
        global
            .add(&entry("f", "r"), ListScope::Global)
            .expect("add");
        assert!(std::fs::symlink_metadata(config.join(LIST_FILE))
            .expect("meta")
            .file_type()
            .is_symlink());
        let text = std::fs::read_to_string(&real).expect("read");
        assert!(
            text.starts_with("# mine") && text.contains("name = \"f\""),
            "{text:?}"
        );
    }
}
