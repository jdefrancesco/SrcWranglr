//! Notes on disk: `<project>/.srcwranglr/notes.json`.
//!
//! The file lives inside a source tree that may be untrusted, so loading
//! validates everything, and neither loading nor saving follows symlinks.

use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Note, NoteTarget, Notes};
use crate::safe_fs::{is_real_dir, write_new_file};

pub const NOTES_DIR: &str = ".srcwranglr";
pub const NOTES_FILE: &str = "notes.json";
const TEMP_FILE: &str = "notes.json.tmp";
const FORMAT_VERSION: u32 = 1;
/// Longest note text (and line anchor) accepted, in bytes.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Longest symbol name or file path accepted, in bytes.
const MAX_NAME_BYTES: usize = 4096;
pub const MAX_NOTES: usize = 20_000;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum NotesError {
    Io { path: PathBuf, message: String },
    Invalid { path: PathBuf, message: String },
}

impl fmt::Display for NotesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotesError::Io { path, message } => write!(f, "{}: {message}", path.display()),
            NotesError::Invalid { path, message } => {
                write!(f, "{} is not a valid notes file: {message}", path.display())
            }
        }
    }
}

/// The file as stored: a version and a flat list of entries.
#[derive(Debug, Serialize, Deserialize)]
struct NotesFile {
    version: u32,
    notes: Vec<NoteEntry>,
}

/// One note: `symbol`, or `file` + `line`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    updated: u64,
}

/// The project's notes; none if it has no notes file.
pub fn load(root: &Path) -> Result<Notes, NotesError> {
    let dir = root.join(NOTES_DIR);
    let path = dir.join(NOTES_FILE);
    let invalid = |message: String| NotesError::Invalid {
        path: path.clone(),
        message,
    };
    if !is_real_dir(&dir).map_err(|e| io_error(&dir, &e))? {
        return Ok(Notes::default());
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Notes::default()),
        Err(e) => return Err(io_error(&path, &e)),
    };
    if !metadata.file_type().is_file() {
        return Err(invalid("not a regular file".into()));
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(invalid(format!("larger than {MAX_FILE_BYTES} bytes")));
    }
    let json = fs::read_to_string(&path).map_err(|e| io_error(&path, &e))?;
    let file: NotesFile = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    if file.version != FORMAT_VERSION {
        return Err(invalid(format!("unsupported version {}", file.version)));
    }
    if file.notes.len() > MAX_NOTES {
        return Err(invalid(format!("more than {MAX_NOTES} notes")));
    }
    let notes = file
        .notes
        .into_iter()
        .enumerate()
        .map(|(i, entry)| to_note(entry).map_err(|m| invalid(format!("note {}: {m}", i + 1))))
        .collect::<Result<Vec<Note>, NotesError>>()?;
    Notes::try_from_notes(notes).map_err(|duplicate| {
        invalid(format!(
            "more than one note on {duplicate:?} (merge them by hand)"
        ))
    })
}

/// Writes the notes atomically. With no notes and no notes folder yet,
/// writes nothing, so browsing a project never adds files to it.
pub fn save(root: &Path, notes: &Notes) -> Result<(), NotesError> {
    let dir = root.join(NOTES_DIR);
    let path = dir.join(NOTES_FILE);
    if !is_real_dir(&dir).map_err(|e| io_error(&dir, &e))? {
        if notes.is_empty() {
            return Ok(());
        }
        fs::create_dir(&dir).map_err(|e| io_error(&dir, &e))?;
    }
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(NotesError::Invalid {
            path,
            message: "not a regular file".into(),
        });
    }
    let file = NotesFile {
        version: FORMAT_VERSION,
        notes: notes.iter().map(to_entry).collect(),
    };
    // Never write what `load` would reject: that would lock the notes.
    if notes.len() > MAX_NOTES {
        return Err(NotesError::Invalid {
            path,
            message: format!("more than {MAX_NOTES} notes; delete some first"),
        });
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| NotesError::Invalid {
        path: path.clone(),
        message: e.to_string(),
    })?;
    if json.len() + 1 > MAX_FILE_BYTES {
        return Err(NotesError::Invalid {
            path,
            message: format!("notes would exceed {MAX_FILE_BYTES} bytes; shorten some first"),
        });
    }
    let temp = dir.join(TEMP_FILE);
    write_new_file(&temp, &format!("{json}\n")).map_err(|e| io_error(&temp, &e))?;
    fs::rename(&temp, &path).map_err(|e| io_error(&path, &e))
}

fn io_error(path: &Path, error: &std::io::Error) -> NotesError {
    NotesError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// `note` as stored in a file.
pub fn to_entry(note: &Note) -> NoteEntry {
    let (symbol, file, line) = match &note.target {
        NoteTarget::Symbol(name) => (Some(name.clone()), None, None),
        NoteTarget::Line { file, line } => (None, Some(file.clone()), Some(*line)),
    };
    NoteEntry {
        symbol,
        file,
        line,
        anchor: note.anchor.clone(),
        text: note.text.clone(),
        starred: note.is_starred,
        created: note.created,
        updated: note.updated,
    }
}

/// The note a stored entry holds, if it is one the notes file accepts.
pub fn to_note(entry: NoteEntry) -> Result<Note, String> {
    let target = match (entry.symbol, entry.file, entry.line) {
        (Some(symbol), None, None) => NoteTarget::Symbol(symbol),
        (None, Some(file), Some(line)) => NoteTarget::Line { file, line },
        _ => return Err("needs either `symbol`, or `file` and `line`".into()),
    };
    check_target(&target)?;
    check_text(&entry.text, "text")?;
    if let Some(anchor) = &entry.anchor {
        check_text(anchor, "anchor")?;
    }
    Ok(Note {
        target,
        text: entry.text,
        is_starred: entry.starred,
        anchor: entry.anchor,
        created: entry.created,
        updated: entry.updated,
    })
}

/// Whether the notes file can hold a note on `target`: a load rejects
/// anything this rejects, so check before saving.
pub fn check_target(target: &NoteTarget) -> Result<(), String> {
    match target {
        NoteTarget::Symbol(name) => check_name(name, "symbol"),
        NoteTarget::Line { file, line } => {
            check_name(file, "file")?;
            check_relative(file)?;
            if *line == 0 {
                return Err("lines start at 1".into());
            }
            Ok(())
        }
    }
}

/// Whether the notes file can hold `text` as a note's text or anchor.
pub fn check_text(text: &str, what: &str) -> Result<(), String> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!("{what} longer than {MAX_TEXT_BYTES} bytes"));
    }
    Ok(())
}

fn check_name(name: &str, what: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(format!("{what} must be 1 to {MAX_NAME_BYTES} bytes"));
    }
    Ok(())
}

/// Project-relative and inside the project: no root, drive, or `..`.
fn check_relative(file: &str) -> Result<(), String> {
    let is_inside = Path::new(file)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if is_inside {
        Ok(())
    } else {
        Err(format!("file `{file}` is not inside the project"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Notes {
        let mut notes = Notes::default();
        notes.set(
            NoteTarget::Symbol("leveldb::DBImpl::Write".into()),
            "takes the mutex\nthen batches writers",
            true,
            None,
            100,
        );
        notes.set(
            NoteTarget::Line {
                file: "db/db_impl.cc".into(),
                line: 1206,
            },
            "",
            true,
            Some("Status DBImpl::Write(".into()),
            200,
        );
        notes
    }

    #[test]
    fn missing_file_loads_as_no_notes() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load(dir.path()).expect("load"), Notes::default());
    }

    #[test]
    fn saves_and_loads_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        save(dir.path(), &sample()).expect("save");
        assert_eq!(load(dir.path()).expect("load"), sample());
        assert!(!dir.path().join(NOTES_DIR).join("notes.json.tmp").exists());
    }

    #[test]
    fn saving_no_notes_does_not_create_the_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        save(dir.path(), &Notes::default()).expect("save");
        assert!(!dir.path().join(NOTES_DIR).exists());
    }

    #[test]
    fn saving_no_notes_over_an_existing_file_clears_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        save(dir.path(), &sample()).expect("save");
        save(dir.path(), &Notes::default()).expect("save");
        assert_eq!(load(dir.path()).expect("load"), Notes::default());
    }

    fn write_raw(root: &Path, json: &str) {
        let dir = root.join(NOTES_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(NOTES_FILE), json).expect("write");
    }

    fn load_raw(json: &str) -> Result<Notes, NotesError> {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw(dir.path(), json);
        load(dir.path())
    }

    #[test]
    fn rejects_malformed_or_hostile_files() {
        let bad = [
            "not json",
            r#"{"version": 2, "notes": []}"#,
            r#"{"version": 1, "notes": [{"text": "no target"}]}"#,
            r#"{"version": 1, "notes": [{"symbol": "f", "file": "a.c", "line": 1}]}"#,
            r#"{"version": 1, "notes": [{"symbol": ""}]}"#,
            r#"{"version": 1, "notes": [{"file": "../../etc/passwd", "line": 1}]}"#,
            r#"{"version": 1, "notes": [{"file": "/etc/passwd", "line": 1}]}"#,
            r#"{"version": 1, "notes": [{"file": "a.c", "line": 0}]}"#,
            r#"{"version": 1, "notes": [{"file": "a.c"}]}"#,
        ];
        for json in bad {
            assert!(
                matches!(load_raw(json), Err(NotesError::Invalid { .. })),
                "accepted {json}"
            );
        }
        let huge = "x".repeat(MAX_TEXT_BYTES + 1);
        let json = format!(r#"{{"version": 1, "notes": [{{"symbol": "f", "text": "{huge}"}}]}}"#);
        assert!(matches!(load_raw(&json), Err(NotesError::Invalid { .. })));
    }

    #[test]
    fn duplicate_targets_make_the_file_invalid() {
        let json = r#"{"version": 1, "notes": [{"symbol": "f", "text": "a"}, {"symbol": "f", "text": "b"}]}"#;
        assert!(matches!(load_raw(json), Err(NotesError::Invalid { .. })));
    }

    #[test]
    fn refuses_to_save_more_than_load_accepts_and_keeps_the_old_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        save(dir.path(), &sample()).expect("save");
        let too_many = Notes::from_notes(
            (0..=MAX_NOTES)
                .map(|i| Note {
                    target: NoteTarget::Symbol(format!("f{i}")),
                    text: String::new(),
                    is_starred: true,
                    anchor: None,
                    created: 1,
                    updated: 1,
                })
                .collect(),
        );
        assert!(save(dir.path(), &too_many).is_err());
        assert_eq!(load(dir.path()).expect("load"), sample());
    }

    #[test]
    fn accepts_minimal_entries_with_defaults() {
        let notes = load_raw(r#"{"version": 1, "notes": [{"symbol": "f", "starred": true}]}"#)
            .expect("load");
        let note = notes.get(&NoteTarget::Symbol("f".into())).expect("note");
        assert!(note.is_starred && note.text.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_notes_folder_and_file() {
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(elsewhere.path(), project.path().join(NOTES_DIR))
            .expect("symlink");
        assert!(save(project.path(), &sample()).is_err());
        assert!(load(project.path()).is_err());
        assert!(!elsewhere.path().join(NOTES_FILE).exists());

        let project = tempfile::tempdir().expect("tempdir");
        let dir = project.path().join(NOTES_DIR);
        std::fs::create_dir(&dir).expect("mkdir");
        let target = elsewhere.path().join("secret");
        std::fs::write(&target, "{}").expect("write");
        std::os::unix::fs::symlink(&target, dir.join(NOTES_FILE)).expect("symlink");
        assert!(load(project.path()).is_err());
    }
}
