//! The semgrep rules a run can use, for the Semgrep Rules window to list:
//! SrcWranglr's own, yours (in `~/.config/srcwranglr/semgrep/`), and the
//! project's (in its `.srcwranglr/semgrep/`). A project's files are the
//! project's own, maybe hostile: every file semgrep could load from its
//! folder is listed (in subfolders too, and whatever its name), read
//! without following symlinks, capped in size and number, and only shown
//! escaped until you trust them.

use std::path::{Path, PathBuf};

use super::dangerous_lists::semgrep_rules_dir;
use super::rule_files::{is_rule_file_name, project_rules, RuleHome};
use crate::analysis::tools::BUNDLED_RULES;
use crate::safe_fs::read_regular_file;

/// Rule files larger than this aren't read.
const MAX_RULE_FILE_BYTES: usize = 1024 * 1024;
/// Rule files listed per folder, at most.
const MAX_RULE_FILES: usize = 500;
/// Folder entries looked at per rules folder (subfolders included), at
/// most.
const MAX_ENTRIES: usize = 4 * MAX_RULE_FILES;
/// Bytes of rule files read per rules folder, at most: they are read as
/// the window opens.
const MAX_FOLDER_BYTES: usize = 8 * 1024 * 1024;
/// How deep subfolders are listed.
const MAX_DEPTH: usize = 4;
/// Rule ids listed per file, at most, and how long each may be.
const MAX_IDS: usize = 200;
const MAX_ID_CHARS: usize = 80;

/// A new rule to start from.
pub const TEMPLATE: &str = "\
rules:
  - id: my-rule
    languages: [c, cpp]
    severity: WARNING   # ERROR, WARNING or INFO
    message: \"What this finds, and why it matters\"
    patterns:
      - pattern: memcpy($DST, $SRC, $LEN)
      # Leave out the fixed form, and copies after a check of the size:
      - pattern-not: memcpy($DST, $SRC, sizeof($DST))
      - pattern-not-inside: |
          if (<... $LEN > sizeof($DST) ...>) ...
          ...
";

/// Where rules come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    BuiltIn,
    Yours,
    Project,
}

impl RuleSource {
    /// Where its files are saved; `None` for SrcWranglr's own.
    pub fn home(self) -> Option<RuleHome> {
        match self {
            RuleSource::BuiltIn => None,
            RuleSource::Yours => Some(RuleHome::Yours),
            RuleSource::Project => Some(RuleHome::Project),
        }
    }
}

/// A file of rules (or one of SrcWranglr's own rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFile {
    pub source: RuleSource,
    /// Where it is; `None` for SrcWranglr's own.
    pub path: Option<PathBuf>,
    pub name: String,
    /// Its text, or why it couldn't be read.
    pub text: Result<String, String>,
    /// The ids of its rules.
    pub ids: Vec<String>,
    /// In a subfolder (its name is the path from the rules folder): read
    /// only here.
    pub is_nested: bool,
}

/// Every rule a semgrep run can use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub built_in: Vec<RuleFile>,
    pub yours: Vec<RuleFile>,
    pub project: Vec<RuleFile>,
    /// Where yours are kept.
    pub yours_dir: Option<PathBuf>,
    /// A folder had more in it than is listed, or nested deeper.
    pub is_cut_short: bool,
}

/// The rules for the project at `root` (if any).
pub fn load(root: Option<&Path>) -> Library {
    load_from(semgrep_rules_dir(), root.and_then(project_rules))
}

/// The rules in `yours_dir` and `project_dir`, with SrcWranglr's own.
pub fn load_from(yours_dir: Option<PathBuf>, project_dir: Option<PathBuf>) -> Library {
    let mut library = Library {
        built_in: built_in(),
        yours_dir: yours_dir.clone(),
        ..Library::default()
    };
    if let Some(dir) = yours_dir {
        library.yours = read_folder(&dir, RuleSource::Yours, &mut library);
    }
    if let Some(dir) = project_dir {
        library.project = read_folder(&dir, RuleSource::Project, &mut library);
    }
    library
}

impl Library {
    pub fn files(&self, source: RuleSource) -> &[RuleFile] {
        match source {
            RuleSource::BuiltIn => &self.built_in,
            RuleSource::Yours => &self.yours,
            RuleSource::Project => &self.project,
        }
    }
}

fn read_folder(dir: &Path, source: RuleSource, library: &mut Library) -> Vec<RuleFile> {
    let mut walk = Walk {
        source,
        files: Vec::new(),
        entries: 0,
        bytes: 0,
        is_cut_short: false,
    };
    walk.folder(dir, Path::new(""), 0);
    let mut files = walk.files;
    files.sort_by(|a, b| (a.is_nested, &a.name).cmp(&(b.is_nested, &b.name)));
    if files.len() > MAX_RULE_FILES {
        files.truncate(MAX_RULE_FILES);
        walk.is_cut_short = true;
    }
    library.is_cut_short |= walk.is_cut_short;
    files
}

/// Listing one rules folder.
struct Walk {
    source: RuleSource,
    files: Vec<RuleFile>,
    entries: usize,
    bytes: usize,
    is_cut_short: bool,
}

impl Walk {
    /// Lists `dir`, `depth` folders down at `prefix` in the rules folder.
    fn folder(&mut self, dir: &Path, prefix: &Path, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if self.entries >= MAX_ENTRIES {
                self.is_cut_short = true;
                return;
            }
            self.entries += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            let shown = prefix.join(&name).to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let text = match (self.source, kind.is_symlink(), kind.is_dir()) {
                // Whatever it points to: semgrep may read it once trusted.
                (RuleSource::Project, true, _) => Err("A symlink: not read.".to_string()),
                (_, false, true) if depth + 1 < MAX_DEPTH => {
                    self.folder(&path, Path::new(&shown), depth + 1);
                    continue;
                }
                (_, false, true) => {
                    self.is_cut_short = true;
                    continue;
                }
                _ if !self.is_listed(&name) => continue,
                _ if entry.file_name().to_str().is_none() => {
                    Err("A name that isn't text: not read.".to_string())
                }
                _ if self.bytes > MAX_FOLDER_BYTES => {
                    self.is_cut_short = true;
                    Err("Not read: there is too much here to read it all.".to_string())
                }
                (RuleSource::Yours, true, _) => read_followed(&path),
                _ => read(&path),
            };
            self.bytes += text.as_ref().map_or(0, String::len);
            let ids = text.as_deref().map(rule_ids).unwrap_or_default();
            self.files.push(RuleFile {
                source: self.source,
                path: Some(path),
                name: shown,
                text,
                ids,
                is_nested: depth > 0,
            });
        }
    }

    /// Whether a file named `name` is listed: in a project's folder, any
    /// file semgrep might load (SrcWranglr's own name, hidden ones and
    /// odd cases included).
    fn is_listed(&self, name: &str) -> bool {
        match self.source {
            RuleSource::Project => is_rule_file_name(RuleHome::Project, name),
            _ => is_rule_file_name(RuleHome::Yours, name),
        }
    }
}

fn read(path: &Path) -> Result<String, String> {
    match read_regular_file(path, MAX_RULE_FILE_BYTES) {
        Ok(Some(text)) => Ok(text),
        Ok(None) => Err("Gone.".into()),
        Err(e) => Err(format!("Not read: {e}.")),
    }
}

/// Yours may be symlinks a dotfile manager keeps.
fn read_followed(path: &Path) -> Result<String, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("Not read: {e}."))?;
    read(&real)
}

/// SrcWranglr's own rules, one by one.
fn built_in() -> Vec<RuleFile> {
    let mut rules: Vec<(String, String)> = Vec::new();
    for line in BUNDLED_RULES.lines() {
        if let Some(id) = line.strip_prefix("  - id:") {
            rules.push((id.trim().to_string(), String::from("rules:\n")));
        }
        if let Some((_, text)) = rules.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    rules
        .into_iter()
        .map(|(id, text)| RuleFile {
            source: RuleSource::BuiltIn,
            path: None,
            name: id.clone(),
            text: Ok(text),
            ids: vec![id],
            is_nested: false,
        })
        .collect()
}

/// The ids of the rules in `text`: each `- id:` of the rule list.
pub fn rule_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim_start().strip_prefix("- id:"))
        .map(|id| {
            id.trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .chars()
                .take(MAX_ID_CHARS)
                .collect::<String>()
        })
        .filter(|id| !id.is_empty())
        .take(MAX_IDS)
        .collect()
}

/// `text` with its first rule's id made `id`.
pub fn with_id(text: &str, id: &str) -> String {
    let mut is_done = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let Some(at) = line.find("- id:").filter(|_| !is_done) else {
                return line.to_string();
            };
            is_done = true;
            format!("{}- id: {id}", &line[..at])
        })
        .collect();
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srcwranglrs_own_rules_are_listed_one_by_one() {
        let rules = built_in();
        assert_eq!(rules.len(), 7);
        assert_eq!(rules[0].name, "srcwranglr.use-after-free");
        let text = rules[0].text.as_deref().expect("text");
        assert!(text.starts_with("rules:\n  - id: srcwranglr.use-after-free\n"));
        assert!(!text.contains("srcwranglr.double-free"));
    }

    #[test]
    fn your_and_the_projects_files_are_listed_by_name() {
        let yours = tempfile::tempdir().expect("dir");
        let project = tempfile::tempdir().expect("dir");
        let rule = "rules:\n  - id: a\n    pattern: x\n  - id: \"b\"\n    pattern: y\n";
        std::fs::write(yours.path().join("z.yaml"), rule).expect("write");
        std::fs::write(yours.path().join("a.yml"), "rules: []\n").expect("write");
        std::fs::write(yours.path().join("notes.txt"), "no").expect("write");
        std::fs::write(yours.path().join("srcwranglr-builtin.yaml"), "x").expect("write");
        std::fs::create_dir(yours.path().join("more")).expect("mkdir");
        std::fs::write(project.path().join("p.yaml"), rule).expect("write");
        let library = load_from(
            Some(yours.path().to_path_buf()),
            Some(project.path().to_path_buf()),
        );
        let names: Vec<&str> = library.yours.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.yml", "z.yaml"]);
        assert_eq!(library.yours[1].ids, ["a", "b"]);
        assert_eq!(library.project[0].source, RuleSource::Project);
        assert!(!library.is_cut_short);
    }

    #[test]
    fn every_file_semgrep_could_load_from_a_project_is_listed() {
        let project = tempfile::tempdir().expect("dir");
        let nested = project.path().join("a/b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        for name in ["srcwranglr-builtin.yaml", ".hidden.yaml", "LOUD.YML"] {
            std::fs::write(project.path().join(name), "rules: []\n").expect("write");
        }
        std::fs::write(nested.join("deep.yaml"), "rules:\n  - id: deep\n").expect("write");
        let library = load_from(None, Some(project.path().to_path_buf()));
        let names: Vec<&str> = library.project.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                ".hidden.yaml",
                "LOUD.YML",
                "srcwranglr-builtin.yaml",
                "a/b/deep.yaml"
            ]
        );
        assert!(library.project[3].is_nested);
        assert_eq!(library.project[3].ids, ["deep"]);
    }

    #[test]
    fn long_ids_are_cut_short() {
        let text = format!("rules:\n  - id: {}\n", "a".repeat(10_000));
        assert_eq!(rule_ids(&text)[0].len(), MAX_ID_CHARS);
    }

    #[cfg(unix)]
    #[test]
    fn a_projects_symlinks_are_listed_not_read() {
        let project = tempfile::tempdir().expect("dir");
        let secret = tempfile::NamedTempFile::new().expect("file");
        std::fs::write(secret.path(), "rules:\n  - id: secret\n").expect("write");
        std::os::unix::fs::symlink(secret.path(), project.path().join("s.yaml")).expect("ln");
        let library = load_from(None, Some(project.path().to_path_buf()));
        assert_eq!(library.project.len(), 1);
        assert!(library.project[0].text.is_err());
        assert!(library.project[0].ids.is_empty());
    }

    #[test]
    fn a_copy_gets_a_new_id() {
        let copy = with_id(
            "rules:\n  - id: srcwranglr.x\n    message: \"- id: no\"\n",
            "my-x",
        );
        assert_eq!(copy, "rules:\n  - id: my-x\n    message: \"- id: no\"\n");
    }

    #[test]
    fn the_template_names_one_rule() {
        assert_eq!(rule_ids(TEMPLATE), ["my-rule"]);
    }
}
