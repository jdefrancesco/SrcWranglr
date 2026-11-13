//! What the app does with the project's journal: reading it when a project
//! opens, the Notes pane (whose Journal tab shows it), saving each change,
//! and going back to where an entry was written.

use super::journal_view::{JournalAction, JournalContext};
use super::notebook::now;
use super::notes_panel::NotesPanelAction;
use super::{SrcWranglrApp, Status};
use crate::notes::journal::{self, Journal, Place, JOURNAL_FILE};
use crate::notes::store::NOTES_DIR;

impl SrcWranglrApp {
    /// Reads the open project's journal; a bad file leaves it read-only.
    pub(super) fn load_journal(&mut self) {
        let loaded = self.project.root.as_deref().map(journal::load);
        (self.journal, self.journal_error) = match loaded {
            Some(Ok(journal)) => (journal, None),
            Some(Err(error)) => (Journal::default(), Some(error)),
            None => (Journal::default(), None),
        };
    }

    /// The Notes pane, with its notes and its journal.
    pub(super) fn show_notes_panel(&mut self, ui: &mut egui::Ui) {
        let storage = match (&self.journal_error, &self.project.root) {
            (Some(error), _) => Err(format!("Read-only: {error}")),
            (None, Some(_)) => Ok(format!("Saved in {NOTES_DIR}/{JOURNAL_FILE}")),
            (None, None) => Ok("Demo project: the journal isn't saved".to_string()),
        };
        let here = self.journal_here();
        let context = JournalContext {
            journal: &self.journal,
            here: here.as_ref(),
            storage: storage.as_deref().map_err(String::as_str),
            now: now(),
        };
        let picked = self
            .notes_panel
            .show(ui, &self.notebook, &context, self.layout);
        match picked {
            Some(NotesPanelAction::GoTo(target)) => self.go_to_note(&target),
            Some(NotesPanelAction::Edit(target)) => self.edit_note(target),
            Some(NotesPanelAction::Journal(action)) => self.journal_action(action),
            None => {}
        }
    }

    fn journal_action(&mut self, action: JournalAction) {
        match action {
            JournalAction::Changed(journal) => {
                self.set_journal(journal);
            }
            JournalAction::Added(journal) => {
                if self.set_journal(journal) {
                    self.notes_panel.journal_view().clear_draft();
                }
            }
            JournalAction::GoTo { file, line, symbol } => {
                self.go_to_journal_place(&file, line, symbol.as_deref());
            }
        }
    }

    /// Keeps `journal`, saved with the project (if it has a folder and its
    /// file could be read). Whether it was kept.
    pub(super) fn set_journal(&mut self, journal: Journal) -> bool {
        if let Some(error) = &self.journal_error {
            self.status = Status::Error(format!("The journal is read-only: {error}"));
            return false;
        }
        if let Some(root) = &self.project.root {
            if let Err(error) = journal::save(root, &journal) {
                self.status = Status::Error(error);
                return false;
            }
        }
        self.journal = journal;
        true
    }

    /// Where you are: the line the source shows, in the selected symbol.
    pub(super) fn journal_here(&self) -> Option<Place> {
        let selected = self.graph_view.selected();
        let at = self.code_panel.position(&self.project.graph, selected)?;
        let symbol = selected
            .and_then(|id| self.project.graph.symbols.get(&id))
            .map(|s| s.qualified_name.clone());
        Some(Place {
            file: at.file,
            line: at.line,
            symbol,
        })
    }

    /// Shows the line an entry was written at, in its symbol if that is
    /// still in the project.
    fn go_to_journal_place(&mut self, file: &str, line: u32, symbol: Option<&str>) {
        let graph = &self.project.graph;
        if !graph.file_contents.contains_key(file) {
            self.status = Status::Info(format!("{file} isn't in this project any more."));
            return;
        }
        let id = symbol.and_then(|name| {
            graph
                .symbols
                .values()
                .filter(|s| s.qualified_name == name && s.file == file)
                .min_by_key(|s| s.line.abs_diff(line))
                .map(|s| s.id)
        });
        self.go_to_place(id, file, line);
    }

    /// Opens the Journal, ready to write in.
    pub(super) fn write_in_journal(&mut self) {
        self.notes_panel.open_journal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::notes::journal::Entry;

    fn app_in(dir: &std::path::Path) -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let graph = crate::index::graph_of_files(&[("a.c", "int f(void) {\n  return 0;\n}\n")]);
        app.project = Project::new("t", Some(dir.to_path_buf()), graph);
        app.load_journal();
        app
    }

    #[test]
    fn a_change_is_saved_with_the_project_and_read_back() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_in(dir.path());
        let journal = Journal::default()
            .with_added(Entry::new("started on the parser", None, 1))
            .expect("valid");
        assert!(app.set_journal(journal.clone()));
        app.journal = Journal::default();
        app.load_journal();
        assert_eq!(app.journal, journal);
        app.journal_error = Some("bad file".into());
        assert!(!app.set_journal(Journal::default()), "read-only");
        assert_eq!(app.journal, journal);
    }

    #[test]
    fn the_journal_command_opens_the_notes_pane() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_in(dir.path());
        app.notes_panel.set_open(false);
        app.run_command(
            crate::app::palette::Command::Journal,
            &egui::Context::default(),
        );
        assert!(app.notes_panel.is_open());
    }

    #[test]
    fn a_bad_file_leaves_the_journal_read_only() {
        let dir = tempfile::tempdir().expect("dir");
        let folder = dir.path().join(NOTES_DIR);
        std::fs::create_dir(&folder).expect("mkdir");
        std::fs::write(folder.join(JOURNAL_FILE), "not json").expect("write");
        let app = app_in(dir.path());
        assert!(app.journal_error.is_some());
        assert!(app.journal.entries().is_empty());
    }

    #[test]
    fn here_is_the_selected_symbol_and_going_back_selects_it() {
        let dir = tempfile::tempdir().expect("dir");
        let mut app = app_in(dir.path());
        let f = app
            .project
            .graph
            .symbols
            .values()
            .find(|s| s.qualified_name == "f")
            .map(|s| s.id)
            .expect("f");
        app.graph_view.select(f);
        let here = app.journal_here().expect("a place");
        assert_eq!(
            (here.file.as_str(), here.symbol.as_deref()),
            ("a.c", Some("f"))
        );
        app.graph_view = Default::default();
        app.go_to_journal_place("a.c", 2, Some("f"));
        assert_eq!(app.graph_view.selected(), Some(f));
        app.go_to_journal_place("gone.c", 1, None);
        assert!(matches!(&app.status, Status::Info(m) if m.contains("gone.c")));
    }
}
