//! Audit bundles: one file with a project's audit — where you were, your
//! notes and bookmarks, TODOs and finding verdicts — to hand to someone
//! auditing their own checkout of the same code. It names the project by
//! its folder's name only, never a path on your machine.
//!
//! A bundle can come from anyone, so reading one checks every part as the
//! project's own files are checked. Importing merges and never overwrites:
//! a note on a spot you already noted is added under yours, and a verdict
//! on a finding you already judged keeps yours.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::session::Session;
use crate::notes::store::{self, NoteEntry, MAX_NOTES, MAX_TEXT_BYTES};
use crate::notes::todo::{self, Todo, Todos, MAX_TODOS};
use crate::notes::triage::{self, Triage, TriageEntry, TriageStatus};
use crate::notes::{Note, NoteTarget, Notes};

/// Extension of bundle files.
pub const BUNDLE_EXTENSION: &str = "srcwranglr-audit";
const FORMAT_VERSION: u32 = 1;
/// A bundle file larger than this isn't read.
const MAX_FILE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_PROJECT_NAME_BYTES: usize = 255;

/// A project's audit, as written to a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BundleFile {
    srcwranglr_audit: u32,
    /// The project folder's name, to tell the reader which code it is for.
    project: String,
    /// Unix seconds.
    #[serde(default)]
    exported: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Session>,
    #[serde(default)]
    notes: Vec<NoteEntry>,
    #[serde(default)]
    todos: Vec<Todo>,
    #[serde(default)]
    verdicts: Vec<TriageEntry>,
}

/// A bundle's contents, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub project: String,
    pub exported: u64,
    pub session: Option<Session>,
    pub notes: Vec<Note>,
    pub todos: Vec<Todo>,
    pub verdicts: Vec<TriageEntry>,
}

impl Bundle {
    /// The bundle's text, ready to save.
    pub fn to_text(&self) -> Result<String, String> {
        let file = BundleFile {
            srcwranglr_audit: FORMAT_VERSION,
            project: self.project.clone(),
            exported: self.exported,
            session: self.session.clone(),
            notes: self.notes.iter().map(store::to_entry).collect(),
            todos: self.todos.clone(),
            verdicts: self.verdicts.clone(),
        };
        serde_json::to_string_pretty(&file)
            .map(|json| format!("{json}\n"))
            .map_err(|e| format!("Could not write the bundle: {e}"))
    }

    /// The bundle in `text`, every part checked.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let file: BundleFile = serde_json::from_str(text)
            .map_err(|e| format!("not a SrcWranglr audit bundle: {e}"))?;
        if file.srcwranglr_audit != FORMAT_VERSION {
            return Err(format!(
                "bundle version {} isn't supported",
                file.srcwranglr_audit
            ));
        }
        check_project_name(&file.project)?;
        if let Some(session) = &file.session {
            session.check().map_err(|e| format!("its session: {e}"))?;
        }
        let notes = checked_notes(file.notes)?;
        if file.todos.len() > MAX_TODOS {
            return Err(format!("more than {MAX_TODOS} TODOs"));
        }
        for (i, todo) in file.todos.iter().enumerate() {
            todo::check_todo(todo).map_err(|e| format!("TODO {}: {e}", i + 1))?;
        }
        if file.verdicts.len() > triage::MAX_ENTRIES {
            return Err(format!("more than {} verdicts", triage::MAX_ENTRIES));
        }
        for (i, verdict) in file.verdicts.iter().enumerate() {
            triage::check_entry(verdict).map_err(|e| format!("verdict {}: {e}", i + 1))?;
        }
        Ok(Self {
            project: file.project,
            exported: file.exported,
            session: file.session,
            notes,
            todos: file.todos,
            verdicts: file.verdicts,
        })
    }
}

fn check_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_PROJECT_NAME_BYTES {
        return Err(format!(
            "its project name must be 1 to {MAX_PROJECT_NAME_BYTES} bytes"
        ));
    }
    Ok(())
}

fn checked_notes(entries: Vec<NoteEntry>) -> Result<Vec<Note>, String> {
    if entries.len() > MAX_NOTES {
        return Err(format!("more than {MAX_NOTES} notes"));
    }
    let notes = entries
        .into_iter()
        .enumerate()
        .map(|(i, entry)| store::to_note(entry).map_err(|e| format!("note {}: {e}", i + 1)))
        .collect::<Result<Vec<Note>, String>>()?;
    Notes::try_from_notes(notes.clone())
        .map_err(|target| format!("more than one note on {target:?}"))?;
    Ok(notes)
}

/// The bundle in the file at `path`.
pub fn read(path: &Path) -> Result<Bundle, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|e| format!("{shown}: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{shown} isn't a bundle file of at most {} MB",
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    // Capped as it is read, too: the file may grow after the check.
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| format!("{shown}: {e}"))?;
    Bundle::from_text(&text).map_err(|e| format!("{shown}: {e}"))
}

/// Which parts of a bundle to merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    pub notes: bool,
    pub todos: bool,
    /// Verdicts that say a finding is a confirmed bug.
    pub bug_verdicts: bool,
    /// Verdicts that say reviewed or false positive: they stop findings
    /// being highlighted, so a bundle can hide findings with them.
    pub dismissing_verdicts: bool,
}

impl Parts {
    pub const ALL: Parts = Parts {
        notes: true,
        todos: true,
        bug_verdicts: true,
        dismissing_verdicts: true,
    };
}

/// What merging a bundle does to each part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub notes_added: usize,
    /// Notes on spots you already noted, added under yours.
    pub notes_appended: usize,
    /// Your notes that only gain the bundle's bookmark star.
    pub notes_starred: usize,
    pub notes_already: usize,
    pub todos_added: usize,
    pub todos_already: usize,
    pub bugs_added: usize,
    /// Reviewed and false-positive verdicts added.
    pub dismissing_added: usize,
    /// Findings you already judged, your verdict kept.
    pub verdicts_kept: usize,
    /// What couldn't be added, and why (the first few).
    pub skipped: Vec<String>,
}

impl Counts {
    pub fn changes_notes(&self) -> bool {
        self.notes_added + self.notes_appended + self.notes_starred > 0
    }

    pub fn changes_verdicts(&self) -> bool {
        self.bugs_added + self.dismissing_added > 0
    }
}

/// The project's audit with a bundle merged in, and what changed.
#[derive(Debug, Clone)]
pub struct Merged {
    pub notes: Notes,
    pub todos: Todos,
    pub triage: Triage,
    pub counts: Counts,
}

/// Skipped parts listed, at most.
const MAX_SKIPPED_SHOWN: usize = 5;

/// `notes`, `todos` and `triage` with the `parts` of `bundle` merged in,
/// never overwriting: see the module's doc. `heading` starts the text
/// added under a note you already had. `now` (Unix seconds) dates what
/// changes. Takes time in proportion to what it merges.
pub fn merge(
    bundle: &Bundle,
    current: (&Notes, &Todos, &Triage),
    parts: Parts,
    heading: &str,
    now: u64,
) -> Merged {
    let (notes, todos, triage) = current;
    let mut counts = Counts::default();
    let notes = match parts.notes {
        true => merged_notes(&bundle.notes, notes, heading, now, &mut counts),
        false => notes.clone(),
    };
    let todos = match parts.todos {
        true => merged_todos(&bundle.todos, todos, &mut counts),
        false => todos.clone(),
    };
    let triage = merged_verdicts(&bundle.verdicts, triage, parts, &mut counts);
    counts.skipped.truncate(MAX_SKIPPED_SHOWN);
    Merged {
        notes,
        todos,
        triage,
        counts,
    }
}

fn merged_notes(
    imported: &[Note],
    current: &Notes,
    heading: &str,
    now: u64,
    counts: &mut Counts,
) -> Notes {
    let mut notes: Vec<Note> = current.iter().cloned().collect();
    let mut at: HashMap<NoteTarget, usize> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.target.clone(), i))
        .collect();
    for note in imported {
        let text = note.text.trim_end();
        if text.trim().is_empty() && !note.is_starred {
            continue;
        }
        let Some(&index) = at.get(&note.target) else {
            if notes.len() >= MAX_NOTES {
                counts.skipped.push(format!("notes past {MAX_NOTES}"));
                continue;
            }
            at.insert(note.target.clone(), notes.len());
            notes.push(Note {
                text: text.to_string(),
                updated: now,
                ..note.clone()
            });
            counts.notes_added += 1;
            continue;
        };
        let mine = &notes[index];
        let is_starred = mine.is_starred || note.is_starred;
        if holds(&mine.text, text) {
            match is_starred == mine.is_starred {
                true => counts.notes_already += 1,
                false => {
                    notes[index] = Note {
                        is_starred,
                        updated: now,
                        ..mine.clone()
                    };
                    counts.notes_starred += 1;
                }
            }
            continue;
        }
        let merged = match mine.text.trim().is_empty() {
            true => text.to_string(),
            false => format!("{}\n\n{heading}\n{text}", mine.text.trim_end()),
        };
        if merged.len() > MAX_TEXT_BYTES {
            counts.skipped.push(format!(
                "a note too long to add to yours ({:?})",
                note.target
            ));
            continue;
        }
        notes[index] = Note {
            text: merged,
            is_starred,
            updated: now,
            ..mine.clone()
        };
        counts.notes_appended += 1;
    }
    // Targets are kept one each above, so this can't fail.
    Notes::try_from_notes(notes).unwrap_or_else(|_| current.clone())
}

/// Whether your note `mine` already has `text`: all of it, or as a block of
/// its own (as a merge adds it, after a heading line).
fn holds(mine: &str, text: &str) -> bool {
    let (mine, text) = (mine.trim(), text.trim());
    text.is_empty()
        || mine == text
        || mine.ends_with(&format!("\n{text}"))
        || mine.contains(&format!("\n{text}\n\n"))
}

fn merged_todos(imported: &[Todo], current: &Todos, counts: &mut Counts) -> Todos {
    let key = |t: &Todo| (t.text.clone(), t.file.clone(), t.line);
    let mut seen: HashSet<_> = current.items().iter().map(key).collect();
    let room = MAX_TODOS.saturating_sub(current.items().len());
    let mut added = Vec::new();
    for todo in imported {
        if !seen.insert(key(todo)) {
            counts.todos_already += 1;
        } else if added.len() == room {
            counts.skipped.push(format!("TODOs past {MAX_TODOS}"));
        } else {
            added.push(todo.clone());
        }
    }
    match current.with_all_added(added.clone()) {
        Ok(todos) => {
            counts.todos_added += added.len();
            todos
        }
        Err(error) => {
            counts.skipped.push(format!("TODOs: {error}"));
            current.clone()
        }
    }
}

fn merged_verdicts(
    imported: &[TriageEntry],
    current: &Triage,
    parts: Parts,
    counts: &mut Counts,
) -> Triage {
    let key = |v: &TriageEntry| (v.rule.clone(), v.file.clone(), v.line, v.column);
    let mut judged: HashSet<_> = current.entries().iter().map(key).collect();
    let room = triage::MAX_ENTRIES.saturating_sub(current.entries().len());
    let mut added = Vec::new();
    for verdict in imported {
        let is_bug = verdict.status == TriageStatus::ConfirmedBug;
        let is_wanted = match is_bug {
            true => parts.bug_verdicts,
            false => parts.dismissing_verdicts,
        };
        if !is_wanted {
            continue;
        }
        if !judged.insert(key(verdict)) {
            counts.verdicts_kept += 1;
        } else if added.len() == room {
            counts
                .skipped
                .push(format!("verdicts past {}", triage::MAX_ENTRIES));
        } else {
            added.push(verdict.clone());
        }
    }
    match current.with_all_added(added.clone()) {
        Ok(triage) => {
            for verdict in &added {
                match verdict.status == TriageStatus::ConfirmedBug {
                    true => counts.bugs_added += 1,
                    false => counts.dismissing_added += 1,
                }
            }
            triage
        }
        Err(error) => {
            counts.skipped.push(format!("verdicts: {error}"));
            current.clone()
        }
    }
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
