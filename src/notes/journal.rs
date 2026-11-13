//! The project's journal, in `<project>/.srcwranglr/journal.json` next to
//! its notes: free-form entries you write as you go (what you looked at,
//! what you think, what to try next), each with when it was written and,
//! if you like, the place you were at.
//!
//! The file sits in a source tree that may be untrusted, so loading checks
//! everything, entries are only ever shown as text (with hidden characters
//! marked), and neither loading nor saving follows symlinks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{check_target, NOTES_DIR};
use super::todo::is_disallowed;
use super::NoteTarget;
use crate::safe_fs::{read_regular_file, write_atomically};

pub const JOURNAL_FILE: &str = "journal.json";
const FORMAT_VERSION: u32 = 1;
/// The longest entry accepted, in bytes.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ENTRIES: usize = 5_000;
/// Years of daily entries; more would slow every save.
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// A place's symbol name is at most this many characters.
const MAX_SYMBOL_CHARS: usize = 512;

/// One entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Markdown.
    pub text: String,
    /// Unix seconds.
    pub created: u64,
    pub updated: u64,
    /// The line it was written at, if linked to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The symbol being looked at then, by qualified name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Where an entry was written: a line, and the symbol being looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub file: String,
    pub line: u32,
    pub symbol: Option<String>,
}

impl Entry {
    pub fn new(text: &str, place: Option<Place>, now: u64) -> Self {
        let (file, line, symbol) = match place {
            Some(place) => (Some(place.file), Some(place.line), place.symbol),
            None => (None, None, None),
        };
        Self {
            text: tidied(text),
            created: now,
            updated: now,
            file,
            line,
            symbol,
        }
    }

    /// The line it was written at.
    pub fn place(&self) -> Option<(&str, u32)> {
        self.file.as_deref().zip(self.line)
    }
}

/// `text` without trailing spaces or blank lines at either end.
fn tidied(text: &str) -> String {
    text.trim_matches('\n').trim_end().to_string()
}

/// The entries, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    entries: Vec<Entry>,
}

impl Journal {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn with_added(&self, entry: Entry) -> Result<Self, String> {
        check_entry(&entry)?;
        if self.entries.len() >= MAX_ENTRIES {
            return Err(format!("The journal holds at most {MAX_ENTRIES} entries"));
        }
        let entries = self.entries.iter().cloned().chain([entry]).collect();
        Ok(Self { entries })
    }

    /// Entry `index` with `text` instead, as of `now`.
    pub fn with_text(&self, index: usize, text: &str, now: u64) -> Result<Self, String> {
        let entry = self.entries.get(index).ok_or("No such entry")?;
        let edited = Entry {
            text: tidied(text),
            updated: now,
            ..entry.clone()
        };
        check_entry(&edited)?;
        let entries = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if i == index {
                    edited.clone()
                } else {
                    e.clone()
                }
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn without(&self, index: usize) -> Self {
        let entries = self
            .entries
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, e)| e.clone())
            .collect();
        Self { entries }
    }
}

/// What the file holds.
#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    entries: Vec<Entry>,
}

/// The project's journal (empty without a file).
pub fn load(root: &Path) -> Result<Journal, String> {
    let path = root.join(NOTES_DIR).join(JOURNAL_FILE);
    let invalid = |message: String| format!("{}: {message}", path.display());
    let Some(json) =
        read_regular_file(&path, MAX_FILE_BYTES).map_err(|e| invalid(e.to_string()))?
    else {
        return Ok(Journal::default());
    };
    let file: JournalFile = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    if file.version != FORMAT_VERSION {
        return Err(invalid(format!("unsupported version {}", file.version)));
    }
    if file.entries.len() > MAX_ENTRIES {
        return Err(invalid(format!("more than {MAX_ENTRIES} entries")));
    }
    for (i, entry) in file.entries.iter().enumerate() {
        check_entry(entry).map_err(|e| invalid(format!("entry {}: {e}", i + 1)))?;
    }
    Ok(Journal {
        entries: file.entries,
    })
}

/// Saves the journal atomically; writes nothing for an empty one when
/// there is no file yet, so browsing a project never adds files to it.
pub fn save(root: &Path, journal: &Journal) -> Result<(), String> {
    let dir = root.join(NOTES_DIR);
    if journal.entries.is_empty() && !dir.join(JOURNAL_FILE).exists() {
        return Ok(());
    }
    journal.entries.iter().try_for_each(check_entry)?;
    let file = JournalFile {
        version: FORMAT_VERSION,
        entries: journal.entries.clone(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    if json.len() >= MAX_FILE_BYTES {
        return Err("The journal is too large to save; delete some entries first".into());
    }
    write_atomically(&dir, JOURNAL_FILE, &format!("{json}\n"))
        .map_err(|e| format!("Could not save {}: {e}", dir.join(JOURNAL_FILE).display()))
}

/// What the journal accepts: text (lines and tabs, but no other control
/// or text-direction characters), and a place inside the project.
pub fn check_entry(entry: &Entry) -> Result<(), String> {
    if entry.text.trim().is_empty() {
        return Err("an entry needs some text".into());
    }
    if entry.text.len() > MAX_TEXT_BYTES {
        return Err(format!("an entry is longer than {MAX_TEXT_BYTES} bytes"));
    }
    let is_bad = |c: char| c != '\n' && c != '\t' && is_disallowed(c);
    if entry.text.chars().any(is_bad) {
        return Err("an entry has control or text-direction characters in it".into());
    }
    if let Some(symbol) = &entry.symbol {
        if symbol.chars().count() > MAX_SYMBOL_CHARS || symbol.chars().any(is_disallowed) {
            return Err("an entry's symbol name is too long or has hidden characters".into());
        }
    }
    match (&entry.file, entry.line) {
        (None, None) => Ok(()),
        (Some(file), _) if file.chars().any(is_disallowed) => {
            Err("an entry's file has control or text-direction characters in it".into())
        }
        (Some(file), Some(line)) => check_target(&NoteTarget::Line {
            file: file.clone(),
            line,
        }),
        _ => Err("an entry's place needs both a file and a line".into()),
    }
}

/// `seconds` since 1970 as `YYYY-MM-DD HH:MM` (UTC).
pub fn date_time(seconds: u64) -> String {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    let of_day = seconds.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02}",
        crate::history::date(seconds),
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Journal {
        let place = Place {
            file: "net/parse.c".into(),
            line: 42,
            symbol: Some("parse_header".into()),
        };
        Journal::default()
            .with_added(Entry::new(
                "Looked at `parse_header`.\n\n- len comes from the wire\n",
                Some(place),
                1_700_000_000,
            ))
            .and_then(|j| {
                j.with_added(Entry::new(
                    "Theory: the TLV reader trusts its length.",
                    None,
                    1_700_000_060,
                ))
            })
            .expect("valid")
    }

    #[test]
    fn edits_return_a_new_journal() {
        let journal = sample();
        let edited = journal
            .with_text(1, "  Theory confirmed.\n\n", 1_700_000_100)
            .expect("valid");
        assert_eq!(edited.entries()[1].text, "  Theory confirmed.");
        assert_eq!(edited.entries()[1].updated, 1_700_000_100);
        assert_eq!(edited.entries()[1].created, 1_700_000_060);
        assert_eq!(
            journal.entries()[1].text,
            "Theory: the TLV reader trusts its length."
        );
        assert!(journal.with_text(1, " \n ", 1).is_err());
        assert_eq!(journal.without(0).entries().len(), 1);
        assert_eq!(journal.entries()[0].place(), Some(("net/parse.c", 42)));
    }

    #[test]
    fn saves_and_loads_and_writes_nothing_for_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        save(dir.path(), &Journal::default()).expect("nothing to save");
        assert!(!dir.path().join(NOTES_DIR).exists());
        save(dir.path(), &sample()).expect("saved");
        assert_eq!(load(dir.path()), Ok(sample()));
    }

    #[test]
    fn refuses_bad_entries_and_hostile_files() {
        let add = |entry: Entry| Journal::default().with_added(entry);
        assert!(add(Entry::new("", None, 1)).is_err());
        assert!(add(Entry::new("sneaky \u{202E}text", None, 1)).is_err());
        assert!(add(Entry::new("bell \u{7}", None, 1)).is_err());
        assert!(add(Entry::new("lines\nand\ttabs are fine", None, 1)).is_ok());
        let outside = Place {
            file: "../outside.c".into(),
            line: 1,
            symbol: None,
        };
        assert!(add(Entry::new("x", Some(outside), 1)).is_err());
        let dir = tempfile::tempdir().expect("dir");
        let folder = dir.path().join(NOTES_DIR);
        std::fs::create_dir(&folder).expect("mkdir");
        for json in [
            r#"{"version": 2, "entries": []}"#,
            r#"{"version": 1, "entries": [{"text": "x", "created": 1, "updated": 1, "file": "a.c"}]}"#,
            r#"{"version": 1, "entries": [{"text": "x", "created": 1, "updated": 1, "file": "/etc/passwd", "line": 1}]}"#,
        ] {
            std::fs::write(folder.join(JOURNAL_FILE), json).expect("write");
            assert!(load(dir.path()).is_err(), "{json}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_a_symlinked_file() {
        let dir = tempfile::tempdir().expect("dir");
        let folder = dir.path().join(NOTES_DIR);
        std::fs::create_dir(&folder).expect("mkdir");
        let elsewhere = dir.path().join("elsewhere.json");
        std::fs::write(&elsewhere, r#"{"version": 1, "entries": []}"#).expect("write");
        std::os::unix::fs::symlink(&elsewhere, folder.join(JOURNAL_FILE)).expect("symlink");
        assert!(load(dir.path()).is_err());
        assert!(save(dir.path(), &sample()).is_err());
    }

    #[test]
    fn times_are_written_in_utc() {
        assert_eq!(date_time(0), "1970-01-01 00:00");
        assert_eq!(date_time(1_700_000_060), "2023-11-14 22:14");
    }
}
