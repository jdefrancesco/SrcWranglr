//! Your settings, kept in `~/.config/srcwranglr/settings.toml`: the editor
//! Open in Editor uses, more folders to find tools in, and the source color
//! scheme. Edited in File > Settings…, or by hand:
//!
//! ```toml
//! editor = "zed"                     # or "code -g {file}:{line}:{column}"
//! tool_paths = ["/opt/homebrew/bin"] # searched before PATH
//! source_colors = "solarized-dark"
//! ```

use std::path::{Path, PathBuf};

use toml_edit::{value, Array, DocumentMut, Item};

use super::dangerous_lists::{config_dir, write_user_file};
use crate::code_view::scheme::Scheme;
use crate::programs::{fits_in_path, SearchPath};

pub const SETTINGS_FILE: &str = "settings.toml";
/// A settings file larger than this isn't read.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Tool folders past this many are refused.
pub const MAX_TOOL_DIRS: usize = 64;
/// The longest editor command or folder accepted, in characters.
const MAX_TEXT_CHARS: usize = 4096;
const HEADER: &str = "# SrcWranglr settings (File > Settings…).\n";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The editor command (`zed`, `vim`) or template with `{file}` in it
    /// (`code -g {file}:{line}:{column}`); `None` leaves it to `$VISUAL`,
    /// `$EDITOR` and what is installed.
    pub editor: Option<String>,
    /// Folders searched for programs before `PATH`.
    pub tool_dirs: Vec<PathBuf>,
    pub scheme: Scheme,
}

impl Settings {
    /// Settings from TOML; a key that is missing keeps its default.
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
        let editor = match doc.get("editor") {
            None => None,
            Some(item) => {
                let editor = item.as_str().ok_or("editor must be a string")?.trim();
                check_text(editor, "editor")?;
                (!editor.is_empty()).then(|| editor.to_string())
            }
        };
        let tool_dirs = match doc.get("tool_paths") {
            None => Vec::new(),
            Some(item) => tool_dirs_of(item)?,
        };
        let scheme = match doc.get("source_colors") {
            None => Scheme::default(),
            Some(item) => {
                let id = item.as_str().ok_or("source_colors must be a string")?;
                Scheme::from_id(id).ok_or_else(|| {
                    let known: Vec<&str> = Scheme::ALL.iter().map(|s| s.id()).collect();
                    format!("unknown source_colors {id:?} (one of {})", known.join(", "))
                })?
            }
        };
        let settings = Self {
            editor,
            tool_dirs,
            scheme,
        };
        settings.check()?;
        Ok(settings)
    }

    /// What `parse` reads back as these settings.
    pub fn to_toml(&self) -> String {
        format!("{HEADER}{}", self.written_over(DocumentMut::new()))
    }

    /// `doc` (your file) with these settings in it, its comments and other
    /// keys kept.
    fn written_over(&self, mut doc: DocumentMut) -> String {
        match &self.editor {
            Some(editor) => doc["editor"] = value(editor.as_str()),
            None => {
                doc.remove("editor");
            }
        }
        let dirs: Array = self
            .tool_dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        doc["tool_paths"] = value(dirs);
        doc["source_colors"] = value(self.scheme.id());
        doc.to_string()
    }

    /// Whether these can be saved (and read back): a short editor command,
    /// and few, absolute folders.
    pub fn check(&self) -> Result<(), String> {
        if let Some(editor) = &self.editor {
            check_text(editor, "editor")?;
        }
        if self.tool_dirs.len() > MAX_TOOL_DIRS {
            return Err(format!("more than {MAX_TOOL_DIRS} tool folders"));
        }
        for dir in &self.tool_dirs {
            check_text(&dir.to_string_lossy(), "tool folder")?;
            if !dir.is_absolute() {
                return Err(format!(
                    "tool folder {} isn't a full path (start it at /)",
                    dir.display()
                ));
            }
            if !fits_in_path(dir) {
                return Err(format!(
                    "tool folder {} has a path separator (:) in its name, which PATH can't hold",
                    dir.display()
                ));
            }
        }
        Ok(())
    }

    /// The settings at `path`: defaults if there is no file. Your own file,
    /// so a symlink to it (a dotfile manager's) is followed.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let shown = path.display();
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{shown}: {e}")),
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{shown} isn't a settings file of at most {MAX_FILE_BYTES} bytes"
            ));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
        Self::parse(&text).map_err(|e| format!("{shown}: {e}"))
    }

    /// Writes the settings to `path` (through its symlink, if it is one),
    /// changing only their keys in the file there. A file that isn't TOML
    /// is kept as `settings.toml.bak` first.
    pub fn save_to(&self, path: &Path) -> Result<PathBuf, String> {
        self.check()?;
        let existing = std::fs::read_to_string(path).ok();
        let text = match existing.as_deref().map(str::parse::<DocumentMut>) {
            Some(Ok(doc)) => self.written_over(doc),
            Some(Err(_)) => {
                let backup = path.with_extension("toml.bak");
                std::fs::copy(path, &backup).map_err(|e| {
                    format!(
                        "Could not keep the unreadable {} as a backup: {e}",
                        path.display()
                    )
                })?;
                self.to_toml()
            }
            None => self.to_toml(),
        };
        write_user_file(path, &text).map_err(|e| format!("Could not save {}: {e}", path.display()))
    }

    /// Where programs are looked for: your folders, then `PATH`, then the
    /// usual install places.
    pub fn search_path(&self) -> SearchPath {
        SearchPath::from_process(&self.tool_dirs)
    }
}

/// Where your settings live.
pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(SETTINGS_FILE))
}

fn tool_dirs_of(item: &Item) -> Result<Vec<PathBuf>, String> {
    let list = item
        .as_array()
        .ok_or("tool_paths must be a list of folders")?;
    if list.len() > MAX_TOOL_DIRS {
        return Err(format!("more than {MAX_TOOL_DIRS} tool_paths"));
    }
    list.iter()
        .map(|v| {
            v.as_str()
                .map(|d| PathBuf::from(d.trim()))
                .ok_or_else(|| "tool_paths must be a list of folders".to_string())
        })
        .collect()
}

fn check_text(text: &str, what: &str) -> Result<(), String> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("{what} is longer than {MAX_TEXT_CHARS} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{what} has control characters in it"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            editor: Some("code -g {file}:{line}:{column}".into()),
            tool_dirs: vec!["/opt/homebrew/bin".into(), "/Users/me/.local/bin".into()],
            scheme: Scheme::SolarizedDark,
        }
    }

    #[test]
    fn saves_and_loads_round_trip() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("srcwranglr").join(SETTINGS_FILE);
        sample().save_to(&path).expect("saved");
        assert_eq!(Settings::load_from(&path), Ok(sample()));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.starts_with("# SrcWranglr settings"), "{text}");
        assert!(
            text.contains("source_colors = \"solarized-dark\""),
            "{text}"
        );
    }

    #[test]
    fn no_file_means_the_defaults_and_missing_keys_keep_theirs() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join(SETTINGS_FILE);
        assert_eq!(Settings::load_from(&missing), Ok(Settings::default()));
        let only_colors = Settings::parse("source_colors = \"monokai\"\n").expect("parsed");
        assert_eq!(only_colors.scheme, Scheme::Monokai);
        assert_eq!(only_colors.editor, None);
        assert!(only_colors.tool_dirs.is_empty());
    }

    #[test]
    fn bad_settings_say_what_is_wrong() {
        for (text, says) in [
            ("editor = 3", "editor must be a string"),
            ("tool_paths = \"/bin\"", "list of folders"),
            ("tool_paths = [\"relative/bin\"]", "full path"),
            ("tool_paths = [\"/my:tools\"]", "path separator"),
            ("source_colors = \"neon\"", "unknown source_colors"),
            ("editor = \"vim\\u0007\"", "control characters"),
            ("= broken", "not valid TOML"),
        ] {
            let error = Settings::parse(text).expect_err(text);
            assert!(error.contains(says), "{text}: {error}");
        }
    }

    #[test]
    fn saving_keeps_your_comments_and_other_keys() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join(SETTINGS_FILE);
        let mine = "editor = \"vim\"\n# colors I like\nsource_colors = \"monokai\"\n\
                    # for a later version\nfuture_option = true\n";
        std::fs::write(&path, mine).expect("write");
        let automatic = Settings {
            scheme: Scheme::GruvboxDark,
            ..Settings::default()
        };
        automatic.save_to(&path).expect("saved");
        let text = std::fs::read_to_string(&path).expect("read");
        for kept in [
            "# colors I like",
            "# for a later version",
            "future_option = true",
        ] {
            assert!(text.contains(kept), "{kept}: {text}");
        }
        assert!(
            !text.contains("vim"),
            "Automatic drops the editor key: {text}"
        );
        assert_eq!(Settings::load_from(&path), Ok(automatic));
    }

    #[test]
    fn saving_over_a_broken_file_keeps_a_backup() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join(SETTINGS_FILE);
        std::fs::write(&path, "editor = [unclosed").expect("write");
        Settings::default().save_to(&path).expect("saved");
        let backup = std::fs::read_to_string(dir.path().join("settings.toml.bak"));
        assert_eq!(backup.ok().as_deref(), Some("editor = [unclosed"));
        assert_eq!(Settings::load_from(&path), Ok(Settings::default()));
    }

    #[test]
    fn refuses_to_save_what_it_would_not_read_back() {
        let relative = Settings {
            tool_dirs: vec!["bin".into()],
            ..Settings::default()
        };
        let dir = tempfile::tempdir().expect("dir");
        assert!(relative.save_to(&dir.path().join(SETTINGS_FILE)).is_err());
        assert!(!dir.path().join(SETTINGS_FILE).exists());
    }
}
