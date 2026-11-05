//! The open project's notes and bookmarks: loaded from and saved to
//! `.slurcetrail/notes.json` (the demo's stay in memory), with each
//! symbol's mark resolved for the graph, symbol list, and code view.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use super::project::Project;
use crate::model::SymbolId;
use crate::notes::store;
use crate::notes::{Note, NoteTarget, Notes};

/// How a symbol or line is marked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoteMark {
    pub is_starred: bool,
    pub has_text: bool,
}

impl NoteMark {
    fn of(note: &Note) -> Self {
        Self {
            is_starred: note.is_starred,
            has_text: !note.text.is_empty(),
        }
    }
}

pub struct Notebook {
    /// Where notes are saved; `None` keeps them in memory (the demo).
    root: Option<PathBuf>,
    notes: Notes,
    /// Why the notes file couldn't be read. Saving is refused meanwhile,
    /// so a file we couldn't parse is never overwritten.
    load_error: Option<String>,
    /// Line notes that followed their line to a new position on open.
    moved_on_open: usize,
    /// Why saving those new positions failed, if it did.
    open_save_error: Option<String>,
    symbol_marks: HashMap<SymbolId, NoteMark>,
}

impl Notebook {
    /// Loads `project`'s notes. Line notes whose line moved (the file
    /// changed since) follow it, and the new positions are saved.
    pub fn open(project: &Project) -> Self {
        let root = project.root.clone();
        let loaded = root
            .as_deref()
            .map(store::load)
            .unwrap_or(Ok(Notes::default()));
        let (notes, load_error) = match loaded {
            Ok(notes) => (notes, None),
            Err(error) => (Notes::default(), Some(error.to_string())),
        };
        let (notes, moved_on_open) = notes.relocated(&project.graph.file_contents);
        let mut book = Self {
            root,
            notes,
            load_error,
            moved_on_open,
            open_save_error: None,
            symbol_marks: HashMap::new(),
        };
        book.refresh_marks(project);
        if moved_on_open > 0 {
            book.open_save_error = book.save().err();
        }
        book
    }

    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn open_save_error(&self) -> Option<&str> {
        self.open_save_error.as_deref()
    }

    pub fn moved_on_open(&self) -> usize {
        self.moved_on_open
    }

    pub fn is_saved_to_disk(&self) -> bool {
        self.root.is_some()
    }

    pub fn mark(&self, id: SymbolId) -> Option<NoteMark> {
        self.symbol_marks.get(&id).copied()
    }

    /// Marks of `file`'s line notes, with their text, by line.
    pub fn line_marks(&self, file: &str) -> BTreeMap<u32, (NoteMark, &str)> {
        self.notes
            .lines_in(file)
            .into_iter()
            .map(|(line, note)| (line, (NoteMark::of(note), note.text.as_str())))
            .collect()
    }

    /// Sets a note's text and star (an empty, unstarred note is deleted)
    /// and saves. Line notes remember their line's text.
    pub fn set(
        &mut self,
        project: &Project,
        target: NoteTarget,
        text: &str,
        is_starred: bool,
    ) -> Result<(), String> {
        self.check_writable()?;
        store::check_target(&target)?;
        store::check_text(text, "The note")?;
        let anchor = anchor_for(project, &target);
        self.notes.set(target, text, is_starred, anchor, now());
        self.refresh_marks(project);
        self.save()
    }

    /// Stars or unstars `target` and saves. Returns whether it's starred.
    pub fn toggle_star(&mut self, project: &Project, target: NoteTarget) -> Result<bool, String> {
        self.check_writable()?;
        store::check_target(&target)?;
        let anchor = anchor_for(project, &target);
        let is_starred = self.notes.toggle_star(target, anchor, now());
        self.refresh_marks(project);
        self.save().map(|()| is_starred)
    }

    fn check_writable(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(format!(
                "Notes are read-only until the notes file is fixed or moved away \
                 (then re-index): {error}"
            )),
            None => Ok(()),
        }
    }

    /// Writes the notes (the demo's aren't written anywhere).
    fn save(&self) -> Result<(), String> {
        match &self.root {
            Some(root) => {
                store::save(root, &self.notes).map_err(|e| format!("Could not save notes: {e}"))
            }
            None => Ok(()),
        }
    }

    fn refresh_marks(&mut self, project: &Project) {
        self.symbol_marks = self
            .notes
            .iter()
            .filter_map(|note| match &note.target {
                NoteTarget::Symbol(name) => Some((name, NoteMark::of(note))),
                NoteTarget::Line { .. } => None,
            })
            .flat_map(|(name, mark)| project.ids_named(name).iter().map(move |id| (*id, mark)))
            .collect();
    }
}

/// A line note's anchor: the line's trimmed text (none for blank lines,
/// or lines too long to store, e.g. minified code).
fn anchor_for(project: &Project, target: &NoteTarget) -> Option<String> {
    let NoteTarget::Line { file, line } = target else {
        return None;
    };
    let contents = project.graph.file_contents.get(file)?;
    let index = usize::try_from(line.checked_sub(1)?).ok()?;
    let text = contents.lines().nth(index)?.trim();
    let is_storable = !text.is_empty() && store::check_text(text, "anchor").is_ok();
    is_storable.then(|| text.to_string())
}

/// Unix seconds now (0 if the clock is before 1970).
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The note target for a symbol: its qualified name.
pub fn symbol_target(project: &Project, id: SymbolId) -> Option<NoteTarget> {
    let symbol = project.graph.symbols.get(&id)?;
    Some(NoteTarget::Symbol(symbol.qualified_name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_codebase;

    fn project_in(root: &std::path::Path) -> Project {
        Project::new(
            "demo",
            Some(root.to_path_buf()),
            demo_codebase().expect("demo"),
        )
    }

    fn target(project: &Project, name: &str) -> NoteTarget {
        symbol_target(project, project.find(name).expect(name)).expect("target")
    }

    #[test]
    fn demo_notes_stay_in_memory_and_mark_their_symbol() {
        let project = Project::demo().expect("demo");
        let mut book = Notebook::open(&project);
        assert!(!book.is_saved_to_disk());
        let circle = project.find("Circle").expect("Circle");
        book.set(&project, target(&project, "Circle"), "round", false)
            .expect("set");
        assert_eq!(
            book.mark(circle),
            Some(NoteMark {
                is_starred: false,
                has_text: true
            })
        );
        assert!(book
            .toggle_star(&project, target(&project, "Circle"))
            .expect("star"));
        assert!(book
            .mark(circle)
            .is_some_and(|m| m.is_starred && m.has_text));
    }

    #[test]
    fn notes_are_saved_and_reloaded_with_the_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = project_in(dir.path());
        let mut book = Notebook::open(&project);
        book.toggle_star(&project, target(&project, "total_area"))
            .expect("star");
        let reopened = Notebook::open(&project_in(dir.path()));
        let id = project.find("total_area").expect("id");
        assert!(reopened.mark(id).is_some_and(|m| m.is_starred));
        assert_eq!(reopened.notes().len(), 1);
    }

    #[test]
    fn line_notes_remember_their_line_text_and_are_listed_per_file() {
        let project = Project::demo().expect("demo");
        let mut book = Notebook::open(&project);
        let line = NoteTarget::Line {
            file: "main.cpp".into(),
            line: 4,
        };
        book.set(&project, line.clone(), "entry", true)
            .expect("set");
        let note = book.notes().get(&line).expect("note");
        assert_eq!(
            note.anchor.as_deref(),
            Some("double total_area(const Circle& c, const Square& s) {")
        );
        let marks = book.line_marks("main.cpp");
        assert_eq!(marks.get(&4).map(|(_, text)| *text), Some("entry"));
        assert!(book.line_marks("circle.h").is_empty());
    }

    #[test]
    fn line_notes_follow_moved_lines_on_open_and_are_saved_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = project_in(dir.path());
        let mut book = Notebook::open(&project);
        let at = |line| NoteTarget::Line {
            file: "main.cpp".into(),
            line,
        };
        book.set(&project, at(4), "entry", false).expect("set");
        // Pretend the file gained two lines on top since the note was made.
        let mut graph = project.graph.clone();
        let source = graph.file_contents["main.cpp"].clone();
        graph
            .file_contents
            .insert("main.cpp".into(), format!("// a\n// b\n{source}"));
        let shifted = Project::new("demo", Some(dir.path().to_path_buf()), graph);
        let reopened = Notebook::open(&shifted);
        assert_eq!(reopened.moved_on_open(), 1);
        assert!(reopened.notes().get(&at(6)).is_some());
        let saved = store::load(dir.path()).expect("load");
        assert!(saved.get(&at(6)).is_some());
    }

    #[test]
    fn whatever_is_saved_loads_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut graph = demo_codebase().expect("demo");
        let huge_line = "x".repeat(store::MAX_TEXT_BYTES + 10);
        graph.file_contents.insert("min.js".into(), huge_line);
        let project = Project::new("p", Some(dir.path().to_path_buf()), graph);
        let mut book = Notebook::open(&project);
        let line = NoteTarget::Line {
            file: "min.js".into(),
            line: 1,
        };
        book.set(&project, line.clone(), "minified", false)
            .expect("set");
        assert_eq!(book.notes().get(&line).and_then(|n| n.anchor.clone()), None);
        let too_long = "y".repeat(store::MAX_TEXT_BYTES + 1);
        assert!(book.set(&project, line.clone(), &too_long, false).is_err());
        let escaping = NoteTarget::Line {
            file: "../outside.c".into(),
            line: 1,
        };
        assert!(book.toggle_star(&project, escaping).is_err());
        assert_eq!(store::load(dir.path()).expect("load").len(), 1);
    }

    #[test]
    fn a_damaged_notes_file_is_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let notes_dir = dir.path().join(store::NOTES_DIR);
        std::fs::create_dir(&notes_dir).expect("mkdir");
        let path = notes_dir.join(store::NOTES_FILE);
        std::fs::write(&path, "{ broken").expect("write");
        let project = project_in(dir.path());
        let mut book = Notebook::open(&project);
        assert!(book.load_error().is_some());
        let result = book.set(&project, target(&project, "main"), "x", false);
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ broken");
    }
}
