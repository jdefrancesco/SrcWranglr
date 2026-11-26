//! Your settings, kept in `~/.config/srcwranglr/settings.toml`: the editor
//! Open in Editor uses, more folders to find tools in, the source color
//! scheme, and the symbol list's font. Edited in File > Settings…, or by
//! hand:
//!
//! ```toml
//! editor = "zed"                     # or "code -g {file}:{line}:{column}"
//! tool_paths = ["/opt/homebrew/bin"] # searched before PATH
//! source_colors = "solarized-dark"
//! symbol_font = "iosevka"            # or "monospace", "source-code-pro"
//! bar_font = "ubuntu-medium"          # the top bar: or "standard", "iosevka", …
//! ```

use std::path::{Path, PathBuf};

use toml_edit::{value, Array, DocumentMut, Item};

use super::dangerous_lists::{config_dir, write_user_file};
use super::fonts::{BarFont, SymbolFont};
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
    /// The font the symbol list is drawn in.
    pub symbol_font: SymbolFont,
    /// The font the top bar is drawn in.
    pub bar_font: BarFont,
    /// Where the in-app agent's model is, and the key for it.
    pub agent: AgentSettings,
}

/// The model the in-app agent uses: an OpenAI-compatible endpoint (z.ai,
/// OpenRouter, OpenAI, a local server), the model id, and the key. The key
/// is kept in the settings file, so only enter one on a machine you trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSettings {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

/// z.ai's GLM coding-plan endpoint; change it for another provider.
pub const DEFAULT_AGENT_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_AGENT_BASE_URL.to_string(),
            model: String::new(),
            api_key: String::new(),
        }
    }
}

impl AgentSettings {
    /// The provider to talk to, if it's filled in enough to try.
    // Consumed by the agent pane (next); keep until it lands.
    #[allow(dead_code)]
    pub fn provider(&self) -> Option<crate::agent::openai::Provider> {
        let provider = crate::agent::openai::Provider {
            base_url: self.base_url.trim().to_string(),
            model: self.model.trim().to_string(),
            api_key: self.api_key.trim().to_string(),
        };
        provider.is_ready().then_some(provider)
    }
}

impl Settings {
    /// Settings from TOML; a key that is missing keeps its default. A key
    /// with a bad value fails the whole parse. Loading uses
    /// [`parse_recovering`](Self::parse_recovering) instead, so this strict
    /// form is kept as the round-trip contract the tests pin down.
    #[cfg(test)]
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
        let settings = Self {
            editor: editor_of(&doc)?,
            tool_dirs: tool_dirs_in(&doc)?,
            scheme: scheme_of(&doc)?,
            symbol_font: symbol_font_in(&doc)?,
            bar_font: bar_font_of(&doc)?,
            agent: agent_of(doc.get("agent"))?,
        };
        settings.check()?;
        Ok(settings)
    }

    /// Like `parse`, but one bad value falls back to that field's default
    /// (with a warning) instead of discarding every setting. Used when
    /// loading, so a single stale or renamed key can't silently reset
    /// everything — colors included. A file that isn't valid TOML at all
    /// still can't be recovered.
    pub fn parse_recovering(text: &str) -> Result<(Self, Vec<String>), String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
        let mut warnings = Vec::new();
        let mut settings = Self {
            editor: recover("editor", editor_of(&doc), &mut warnings),
            tool_dirs: recover("tool_paths", tool_dirs_in(&doc), &mut warnings),
            scheme: recover("source_colors", scheme_of(&doc), &mut warnings),
            symbol_font: recover("symbol_font", symbol_font_in(&doc), &mut warnings),
            bar_font: recover("bar_font", bar_font_of(&doc), &mut warnings),
            agent: recover("agent", agent_of(doc.get("agent")), &mut warnings),
        };
        // The recovered settings must still be saveable (and read back);
        // the only check-level risk is the tool folders, so drop them if so.
        if let Err(e) = settings.check() {
            warnings.push(format!("tool_paths: {e}; dropping them"));
            settings.tool_dirs.clear();
        }
        Ok((settings, warnings))
    }

    /// The settings at `path`, recovering what it can: defaults if there is
    /// no file, and any bad key degraded to its default (with warnings
    /// naming them) rather than reverting the whole file. A missing home,
    /// an oversized file or unreadable TOML is still a hard error.
    pub fn load_recovering(path: &Path) -> Result<(Self, Vec<String>), String> {
        let shown = path.display();
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Self::default(), Vec::new()))
            }
            Err(e) => return Err(format!("{shown}: {e}")),
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{shown} isn't a settings file of at most {MAX_FILE_BYTES} bytes"
            ));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
        Self::parse_recovering(&text).map_err(|e| format!("{shown}: {e}"))
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
        doc["symbol_font"] = value(self.symbol_font.id());
        doc["bar_font"] = value(self.bar_font.id());
        // The [agent] table is only written once configured, so a file
        // from someone who never uses the agent stays clean.
        if self.agent != AgentSettings::default()
            && (!self.agent.model.trim().is_empty() || !self.agent.api_key.trim().is_empty())
        {
            let mut agent = toml_edit::Table::new();
            agent["base_url"] = value(self.agent.base_url.as_str());
            agent["model"] = value(self.agent.model.as_str());
            if !self.agent.api_key.trim().is_empty() {
                agent["api_key"] = value(self.agent.api_key.as_str());
            }
            doc["agent"] = Item::Table(agent);
        } else {
            doc.remove("agent");
        }
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

    /// The settings at `path`, parsed strictly: defaults if there is no
    /// file. The app loads with [`load_recovering`](Self::load_recovering);
    /// this strict form backs the round-trip tests.
    #[cfg(test)]
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

/// A recovered value, or that field's default with a warning recorded.
fn recover<T: Default>(field: &str, result: Result<T, String>, warnings: &mut Vec<String>) -> T {
    result.unwrap_or_else(|e| {
        warnings.push(format!("{field}: {e}; using the default"));
        T::default()
    })
}

/// The `editor` key: a command, or `None` when missing or blank.
fn editor_of(doc: &DocumentMut) -> Result<Option<String>, String> {
    let Some(item) = doc.get("editor") else {
        return Ok(None);
    };
    let editor = item.as_str().ok_or("editor must be a string")?.trim();
    check_text(editor, "editor")?;
    Ok((!editor.is_empty()).then(|| editor.to_string()))
}

/// The `tool_paths` key, or an empty list when missing.
fn tool_dirs_in(doc: &DocumentMut) -> Result<Vec<PathBuf>, String> {
    match doc.get("tool_paths") {
        None => Ok(Vec::new()),
        Some(item) => tool_dirs_of(item),
    }
}

/// The `source_colors` key, or the default scheme when missing.
fn scheme_of(doc: &DocumentMut) -> Result<Scheme, String> {
    let Some(item) = doc.get("source_colors") else {
        return Ok(Scheme::default());
    };
    let id = item.as_str().ok_or("source_colors must be a string")?;
    Scheme::from_id(id).ok_or_else(|| {
        let known: Vec<&str> = Scheme::ALL.iter().map(|s| s.id()).collect();
        format!("unknown source_colors {id:?} (one of {})", known.join(", "))
    })
}

/// The `symbol_font` key, or the default font when missing.
fn symbol_font_in(doc: &DocumentMut) -> Result<SymbolFont, String> {
    match doc.get("symbol_font") {
        None => Ok(SymbolFont::default()),
        Some(item) => symbol_font_of(item),
    }
}

/// The `bar_font` key, or the default font when missing.
fn bar_font_of(doc: &DocumentMut) -> Result<BarFont, String> {
    let Some(item) = doc.get("bar_font") else {
        return Ok(BarFont::default());
    };
    let id = item.as_str().ok_or("bar_font must be a string")?;
    BarFont::from_id(id).ok_or_else(|| {
        let known: Vec<&str> = BarFont::ALL.iter().map(|f| f.id()).collect();
        format!("unknown bar_font {id:?} (one of {})", known.join(", "))
    })
}

/// The `[agent]` table, if present: base_url, model, api_key, each a
/// string, with the default endpoint when base_url is left out.
fn agent_of(item: Option<&Item>) -> Result<AgentSettings, String> {
    let Some(item) = item else {
        return Ok(AgentSettings::default());
    };
    let table = item.as_table_like().ok_or("agent must be a table")?;
    let string = |key: &str| -> Result<String, String> {
        match table.get(key) {
            None => Ok(String::new()),
            Some(v) => {
                let text = v.as_str().ok_or(format!("agent.{key} must be a string"))?;
                check_text(text, &format!("agent.{key}"))?;
                Ok(text.trim().to_string())
            }
        }
    };
    let base_url = string("base_url")?;
    Ok(AgentSettings {
        base_url: if base_url.is_empty() {
            DEFAULT_AGENT_BASE_URL.to_string()
        } else {
            base_url
        },
        model: string("model")?,
        api_key: string("api_key")?,
    })
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

fn symbol_font_of(item: &Item) -> Result<SymbolFont, String> {
    let id = item.as_str().ok_or("symbol_font must be a string")?;
    SymbolFont::from_id(id).ok_or_else(|| {
        let known: Vec<&str> = SymbolFont::ALL.iter().map(|f| f.id()).collect();
        format!("unknown symbol_font {id:?} (one of {})", known.join(", "))
    })
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
            symbol_font: SymbolFont::Iosevka,
            bar_font: BarFont::SourceCodePro,
            agent: AgentSettings::default(),
        }
    }

    #[test]
    fn the_agent_provider_round_trips_and_a_bare_file_has_no_agent_table() {
        // A default (unconfigured) agent isn't written.
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("srcwranglr").join(SETTINGS_FILE);
        sample().save_to(&path).expect("saved");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("[agent]"));
        assert_eq!(
            Settings::default().agent.provider(),
            None,
            "empty is not ready"
        );

        // A configured one round-trips and yields a provider.
        let mut configured = sample();
        configured.agent = AgentSettings {
            base_url: "https://api.z.ai/api/coding/paas/v4".into(),
            model: "glm-5.3".into(),
            api_key: "secret".into(),
        };
        configured.save_to(&path).expect("saved");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[agent]") && text.contains("model = \"glm-5.3\""),
            "{text}"
        );
        let loaded = Settings::load_from(&path).expect("loaded");
        assert_eq!(loaded.agent, configured.agent);
        let provider = loaded.agent.provider().expect("ready");
        assert_eq!(provider.model, "glm-5.3");
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
        assert!(text.contains("symbol_font = \"iosevka\""), "{text}");
        assert!(text.contains("bar_font = \"source-code-pro\""), "{text}");
    }

    #[test]
    fn one_bad_key_keeps_the_rest_instead_of_reverting_everything() {
        // A stale tool_paths value used to fail the whole parse, reverting
        // the color scheme (and all else) to defaults on the next launch.
        let text = "source_colors = \"monokai\"\n\
                    symbol_font = \"iosevka\"\n\
                    tool_paths = [\"relative/bin\"]\n";
        assert!(Settings::parse(text).is_err(), "strict parse still fails");
        let (recovered, warnings) = Settings::parse_recovering(text).expect("recovered");
        assert_eq!(recovered.scheme, Scheme::Monokai, "colors kept");
        assert_eq!(recovered.symbol_font, SymbolFont::Iosevka, "font kept");
        assert!(recovered.tool_dirs.is_empty(), "bad folders dropped");
        assert!(
            warnings.iter().any(|w| w.contains("tool_paths")),
            "{warnings:?}"
        );
    }

    #[test]
    fn an_unknown_color_scheme_degrades_only_itself() {
        let text = "source_colors = \"neon\"\nsymbol_font = \"iosevka\"\n";
        let (recovered, warnings) = Settings::parse_recovering(text).expect("recovered");
        assert_eq!(
            recovered.scheme,
            Scheme::default(),
            "unknown color defaulted"
        );
        assert_eq!(
            recovered.symbol_font,
            SymbolFont::Iosevka,
            "font still kept"
        );
        assert!(
            warnings.iter().any(|w| w.contains("source_colors")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_clean_file_recovers_with_no_warnings() {
        let (recovered, warnings) = Settings::parse_recovering(&sample().to_toml()).expect("ok");
        assert_eq!(recovered, sample());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn an_unknown_bar_font_is_refused() {
        let error = Settings::parse("bar_font = \"comic-sans\"\n").expect_err("unknown");
        assert!(error.contains("unknown bar_font"), "{error}");
        let standard = Settings::parse("").expect("parsed");
        assert_eq!(standard.bar_font, BarFont::Standard);
    }

    #[test]
    fn no_file_means_the_defaults_and_missing_keys_keep_theirs() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join(SETTINGS_FILE);
        assert_eq!(Settings::load_from(&missing), Ok(Settings::default()));
        let only_colors = Settings::parse("source_colors = \"monokai\"\n").expect("parsed");
        assert_eq!(only_colors.scheme, Scheme::Monokai);
        assert_eq!(only_colors.symbol_font, SymbolFont::Monospace);
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
            ("symbol_font = \"comic-sans\"", "unknown symbol_font"),
            ("symbol_font = 12", "symbol_font must be a string"),
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
