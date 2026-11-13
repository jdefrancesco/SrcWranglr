//! Notes and bookmarks the user attaches to symbols and source lines.
//!
//! A note targets a symbol (by qualified name, so it survives re-indexing)
//! or one line of a file. It has text, possibly empty, and may be starred:
//! a starred note is a bookmark. Each target has at most one note.

pub mod journal;
pub mod store;
pub mod todo;
pub mod triage;

use std::collections::{HashMap, HashSet};

/// What a note is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NoteTarget {
    /// A symbol, by qualified name.
    Symbol(String),
    /// A 1-based line of a project-relative file.
    Line { file: String, line: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub target: NoteTarget,
    pub text: String,
    /// Starred notes are bookmarks.
    pub is_starred: bool,
    /// For line notes: the line's (trimmed) text when the note was made,
    /// to find the line again after the file changes.
    pub anchor: Option<String>,
    /// Unix seconds.
    pub created: u64,
    pub updated: u64,
}

/// All notes of a project, in the order they were made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    notes: Vec<Note>,
}

impl Notes {
    /// The given notes; a later note for the same target replaces an
    /// earlier one.
    #[cfg(test)]
    pub fn from_notes(notes: Vec<Note>) -> Self {
        let mut index: HashMap<NoteTarget, usize> = HashMap::with_capacity(notes.len());
        let mut deduped: Vec<Note> = Vec::with_capacity(notes.len());
        for note in notes {
            match index.get(&note.target) {
                Some(&i) => deduped[i] = note,
                None => {
                    index.insert(note.target.clone(), deduped.len());
                    deduped.push(note);
                }
            }
        }
        Self { notes: deduped }
    }

    /// The given notes, or the first target that has more than one.
    pub fn try_from_notes(notes: Vec<Note>) -> Result<Self, NoteTarget> {
        let mut seen: HashSet<&NoteTarget> = HashSet::with_capacity(notes.len());
        if let Some(duplicate) = notes.iter().find(|n| !seen.insert(&n.target)) {
            return Err(duplicate.target.clone());
        }
        Ok(Self { notes })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter()
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn get(&self, target: &NoteTarget) -> Option<&Note> {
        self.notes.iter().find(|n| &n.target == target)
    }

    /// Line notes of `file`, by line.
    pub fn lines_in(&self, file: &str) -> Vec<(u32, &Note)> {
        let mut lines: Vec<(u32, &Note)> = self
            .notes
            .iter()
            .filter_map(|n| match &n.target {
                NoteTarget::Line { file: f, line } if f == file => Some((*line, n)),
                _ => None,
            })
            .collect();
        lines.sort_by_key(|(line, _)| *line);
        lines
    }

    /// Sets `target`'s text and star, creating the note if needed. A note
    /// left with no text and no star is deleted. Trailing whitespace is
    /// dropped. `anchor` replaces the stored one when given.
    pub fn set(
        &mut self,
        target: NoteTarget,
        text: &str,
        is_starred: bool,
        anchor: Option<String>,
        now: u64,
    ) {
        let text = text.trim_end();
        if text.trim().is_empty() && !is_starred {
            self.remove(&target);
            return;
        }
        match self.notes.iter_mut().find(|n| n.target == target) {
            Some(note) => {
                note.text = text.to_string();
                note.is_starred = is_starred;
                note.anchor = anchor.or(note.anchor.take());
                note.updated = now;
            }
            None => self.notes.push(Note {
                target,
                text: text.to_string(),
                is_starred,
                anchor,
                created: now,
                updated: now,
            }),
        }
    }

    /// Stars or unstars `target`. Returns whether it is starred now.
    pub fn toggle_star(&mut self, target: NoteTarget, anchor: Option<String>, now: u64) -> bool {
        let (text, was_starred) = self
            .get(&target)
            .map_or((String::new(), false), |n| (n.text.clone(), n.is_starred));
        self.set(target, &text, !was_starred, anchor, now);
        !was_starred
    }

    pub fn remove(&mut self, target: &NoteTarget) {
        self.notes.retain(|n| &n.target != target);
    }

    /// These notes with each line note moved to where its anchored text is
    /// now, if its own line no longer has it. Returns how many moved.
    ///
    /// Moves are conservative: only to the one line in the file with that
    /// exact text (an anchor matching several lines, or no letters or
    /// digits, is ambiguous), and never onto a line another note keeps.
    /// A note that can't move stays where it was; nothing is ever lost.
    pub fn relocated(&self, file_contents: &HashMap<String, String>) -> (Self, usize) {
        let mut line_index = LineIndex::new(file_contents);
        let wanted: Vec<Option<NoteTarget>> = self
            .notes
            .iter()
            .map(|note| line_index.relocated(note))
            .collect();
        let mut finals: Vec<&NoteTarget> = self
            .notes
            .iter()
            .zip(&wanted)
            .map(|(note, to)| to.as_ref().unwrap_or(&note.target))
            .collect();
        // Undo moves that collide until every target is unique. Each round
        // undoes at least one move and none is redone, so this ends; the
        // original targets were unique, so the result is.
        loop {
            let mut counts: HashMap<&NoteTarget, usize> = HashMap::new();
            for target in &finals {
                *counts.entry(*target).or_insert(0) += 1;
            }
            let mut undone = false;
            for (note, target) in self.notes.iter().zip(finals.iter_mut()) {
                if counts[*target] > 1 && **target != note.target {
                    *target = &note.target;
                    undone = true;
                }
            }
            if !undone {
                break;
            }
        }
        let moved = self
            .notes
            .iter()
            .zip(&finals)
            .filter(|(note, target)| note.target != ***target)
            .count();
        let notes = self
            .notes
            .iter()
            .zip(finals)
            .map(|(note, target)| Note {
                target: target.clone(),
                ..note.clone()
            })
            .collect();
        (Self { notes }, moved)
    }
}

/// Per file, which line each trimmed text is on, if it is on exactly one.
/// Built lazily, once per file.
struct LineIndex<'a> {
    contents: &'a HashMap<String, String>,
    files: HashMap<&'a str, HashMap<&'a str, Option<usize>>>,
}

impl<'a> LineIndex<'a> {
    fn new(contents: &'a HashMap<String, String>) -> Self {
        Self {
            contents,
            files: HashMap::new(),
        }
    }

    /// Where a line note's anchored text is now, if not on its own line.
    fn relocated(&mut self, note: &Note) -> Option<NoteTarget> {
        let NoteTarget::Line { file, line } = &note.target else {
            return None;
        };
        let anchor = note
            .anchor
            .as_deref()
            .filter(|a| a.chars().any(char::is_alphanumeric))?;
        let (key, text) = self.contents.get_key_value(file)?;
        let lines = self.files.entry(key.as_str()).or_insert_with(|| {
            let mut lines: HashMap<&str, Option<usize>> = HashMap::new();
            for (index, line) in text.lines().enumerate() {
                lines
                    .entry(line.trim())
                    .and_modify(|unique| *unique = None)
                    .or_insert(Some(index));
            }
            lines
        });
        let found = (*lines.get(anchor)?)?;
        let found = u32::try_from(found + 1).ok()?;
        (found != *line).then(|| NoteTarget::Line {
            file: file.clone(),
            line: found,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str) -> NoteTarget {
        NoteTarget::Symbol(name.into())
    }

    fn line(file: &str, line: u32) -> NoteTarget {
        NoteTarget::Line {
            file: file.into(),
            line,
        }
    }

    #[test]
    fn set_creates_updates_and_deletes_empty_unstarred_notes() {
        let mut notes = Notes::default();
        notes.set(symbol("a::f"), "checks bounds?  \n", false, None, 10);
        let note = notes.get(&symbol("a::f")).expect("created");
        assert_eq!((note.text.as_str(), note.created), ("checks bounds?", 10));

        notes.set(symbol("a::f"), "no it doesn't", true, None, 20);
        let note = notes.get(&symbol("a::f")).expect("updated");
        assert_eq!(
            (
                note.text.as_str(),
                note.is_starred,
                note.created,
                note.updated
            ),
            ("no it doesn't", true, 10, 20)
        );
        assert_eq!(notes.len(), 1);

        notes.set(symbol("a::f"), "   ", false, None, 30);
        assert!(notes.get(&symbol("a::f")).is_none());
        assert!(notes.is_empty());
    }

    #[test]
    fn toggling_a_star_bookmarks_and_unbookmarks() {
        let mut notes = Notes::default();
        assert!(notes.toggle_star(line("a.c", 3), Some("x = 1;".into()), 5));
        assert!(notes.get(&line("a.c", 3)).is_some_and(|n| n.is_starred));
        // Unstarring a note without text deletes it.
        assert!(!notes.toggle_star(line("a.c", 3), None, 6));
        assert!(notes.is_empty());

        notes.set(symbol("g"), "keep me", false, None, 7);
        notes.toggle_star(symbol("g"), None, 8);
        notes.toggle_star(symbol("g"), None, 9);
        assert_eq!(
            notes.get(&symbol("g")).map(|n| n.text.as_str()),
            Some("keep me")
        );
    }

    #[test]
    fn from_notes_keeps_the_last_note_per_target() {
        let note = |text: &str| Note {
            target: symbol("f"),
            text: text.into(),
            is_starred: false,
            anchor: None,
            created: 1,
            updated: 1,
        };
        let notes = Notes::from_notes(vec![note("old"), note("new")]);
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes.get(&symbol("f")).map(|n| n.text.as_str()),
            Some("new")
        );
    }

    #[test]
    fn line_notes_follow_their_line_when_the_file_changes() {
        let mut notes = Notes::default();
        notes.set(
            line("a.c", 2),
            "here",
            false,
            Some("memcpy(d, s, n);".into()),
            1,
        );
        notes.set(line("a.c", 5), "gone", false, Some("free(p);".into()), 1);
        notes.set(line("b.c", 1), "same", false, Some("int x;".into()), 1);
        let contents = HashMap::from([
            (
                "a.c".to_string(),
                "// new header\n// more\nint f() {\n    memcpy(d, s, n);\n}\n".to_string(),
            ),
            ("b.c".to_string(), "int x;\n".to_string()),
        ]);
        let (moved, count) = notes.relocated(&contents);
        assert_eq!(count, 1);
        assert!(moved.get(&line("a.c", 4)).is_some_and(|n| n.text == "here"));
        // No line matches `free(p);` any more: the note stays put.
        assert!(moved.get(&line("a.c", 5)).is_some_and(|n| n.text == "gone"));
        assert_eq!(moved.len(), 3);
        assert!(moved.get(&line("b.c", 1)).is_some());
    }

    #[test]
    fn relocation_never_moves_a_note_onto_another_note() {
        let mut notes = Notes::default();
        notes.set(line("a.c", 1), "moving", false, Some("b();".into()), 1);
        notes.set(line("a.c", 2), "stale", false, Some("gone();".into()), 1);
        let contents = HashMap::from([("a.c".to_string(), "x\nb();\n".to_string())]);
        let (moved, count) = notes.relocated(&contents);
        assert_eq!((count, moved.len()), (0, 2));
        assert!(moved
            .get(&line("a.c", 1))
            .is_some_and(|n| n.text == "moving"));
        assert!(moved
            .get(&line("a.c", 2))
            .is_some_and(|n| n.text == "stale"));
    }

    #[test]
    fn ambiguous_or_featureless_anchors_never_move_a_note() {
        let mut notes = Notes::default();
        notes.set(line("a.c", 5), "n", false, Some("i++;".into()), 1);
        notes.set(line("a.c", 6), "brace", false, Some("}".into()), 1);
        let contents = HashMap::from([(
            "a.c".to_string(),
            "i++;\nx\nx\nx\nx\nx\ni++;\n}\n".to_string(),
        )]);
        let (moved, count) = notes.relocated(&contents);
        assert_eq!(count, 0);
        assert!(moved.get(&line("a.c", 5)).is_some());
        assert!(moved.get(&line("a.c", 6)).is_some());
    }

    #[test]
    fn a_blocked_move_keeps_its_line_so_targets_stay_unique() {
        // A's text moved to line 2; B's text moved to line 3, where C is.
        // B can't move, so it keeps line 2, and A must stay on line 1.
        let mut notes = Notes::default();
        notes.set(line("a.c", 1), "A", false, Some("alpha();".into()), 1);
        notes.set(line("a.c", 2), "B", false, Some("beta();".into()), 1);
        notes.set(line("a.c", 3), "C", false, Some("gone();".into()), 1);
        let contents = HashMap::from([("a.c".to_string(), "x\nalpha();\nbeta();\n".to_string())]);
        let (moved, count) = notes.relocated(&contents);
        assert_eq!((count, moved.len()), (0, 3));
        let texts: Vec<&str> = [1, 2, 3]
            .iter()
            .filter_map(|l| moved.get(&line("a.c", *l)).map(|n| n.text.as_str()))
            .collect();
        assert_eq!(texts, ["A", "B", "C"]);
    }

    #[test]
    fn chained_moves_all_happen_when_nothing_blocks_them() {
        let mut notes = Notes::default();
        notes.set(line("a.c", 1), "A", false, Some("alpha();".into()), 1);
        notes.set(line("a.c", 2), "B", false, Some("beta();".into()), 1);
        let contents = HashMap::from([("a.c".to_string(), "x\nalpha();\nbeta();\n".to_string())]);
        let (moved, count) = notes.relocated(&contents);
        assert_eq!(count, 2);
        assert!(moved.get(&line("a.c", 2)).is_some_and(|n| n.text == "A"));
        assert!(moved.get(&line("a.c", 3)).is_some_and(|n| n.text == "B"));
    }

    #[test]
    fn from_notes_reports_duplicate_targets() {
        let note = |text: &str| Note {
            target: symbol("f"),
            text: text.into(),
            is_starred: false,
            anchor: None,
            created: 1,
            updated: 1,
        };
        assert!(Notes::try_from_notes(vec![note("a"), note("b")]).is_err());
        assert!(Notes::try_from_notes(vec![note("a")]).is_ok());
    }

    #[test]
    fn line_notes_in_a_file_are_listed_by_line() {
        let mut notes = Notes::default();
        notes.set(line("a.c", 9), "x", false, None, 1);
        notes.set(line("a.c", 2), "y", false, None, 1);
        notes.set(line("b.c", 1), "z", false, None, 1);
        notes.set(symbol("f"), "w", false, None, 1);
        let lines: Vec<u32> = notes.lines_in("a.c").iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, [2, 9]);
    }
}
