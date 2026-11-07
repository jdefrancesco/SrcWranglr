//! What the app does with notes and bookmarks: toggling stars, editing a
//! note in the editor, and jumping to a note's symbol or line.

use super::note_editor::{EditorOutcome, NoteEditor};
use super::notebook::symbol_target;
use super::{SrcWranglrApp, Status};
use crate::model::SymbolId;
use crate::notes::NoteTarget;

/// Note text shown in a palette row (long notes would make search slow).
const MAX_LABEL_CHARS: usize = 80;

impl SrcWranglrApp {
    /// The note target of the selected symbol, or says why there's none.
    pub(super) fn selected_target(&mut self) -> Option<NoteTarget> {
        let target = self
            .graph_view
            .selected()
            .and_then(|id| symbol_target(&self.project, id));
        if target.is_none() {
            self.status = Status::Info("Select a symbol first.".into());
        }
        target
    }

    pub(super) fn toggle_bookmark(&mut self, target: NoteTarget) {
        let label = target_label(&target);
        self.status = match self.notebook.toggle_star(&self.project, target) {
            Ok(true) => Status::Info(format!("Bookmarked {label}.{}", self.unsaved_hint())),
            Ok(false) => Status::Info(format!("Removed the bookmark on {label}.")),
            Err(error) => Status::Error(error),
        };
    }

    pub(super) fn edit_note(&mut self, target: NoteTarget) {
        if let Some(error) = self.notebook.load_error() {
            self.status = Status::Error(format!("Notes are read-only: {error}"));
            return;
        }
        let existing = self.notebook.notes().get(&target);
        let title = target_label(&target);
        self.note_editor = Some(NoteEditor::new(target, title, existing));
    }

    /// Draws the note editor if open, and applies what the user decided.
    pub(super) fn show_note_editor(&mut self, ctx: &egui::Context) {
        if let Some(outcome) = self.note_editor.as_mut().and_then(|e| e.show(ctx)) {
            self.apply_editor_outcome(outcome);
        }
    }

    /// Saves or deletes as the user decided. The editor closes unless that
    /// fails, in which case it stays open showing why.
    fn apply_editor_outcome(&mut self, outcome: EditorOutcome) {
        let (result, done) = match outcome {
            EditorOutcome::Save {
                target,
                text,
                is_starred,
            } => (
                self.notebook.set(&self.project, target, &text, is_starred),
                "Note saved.",
            ),
            EditorOutcome::Delete(target) => (
                self.notebook.set(&self.project, target, "", false),
                "Note deleted.",
            ),
            EditorOutcome::Cancel => {
                self.note_editor = None;
                return;
            }
        };
        match result {
            Ok(()) => {
                self.note_editor = None;
                self.status = Status::Info(format!("{done}{}", self.unsaved_hint()));
            }
            // Keep the editor (and what was typed) open with the error.
            Err(error) => {
                if let Some(editor) = self.note_editor.as_mut() {
                    editor.show_error(error);
                }
            }
        }
    }

    /// Shows a note's symbol, or the line it's on.
    pub(super) fn go_to_note(&mut self, target: &NoteTarget) {
        match target {
            NoteTarget::Symbol(name) => match self.project.ids_named(name).first() {
                Some(id) => self.graph_view.select(*id),
                None => {
                    self.status = Status::Info(format!("{name} is not in this index any more."))
                }
            },
            NoteTarget::Line { file, line } => self.go_to_place(None, file, *line),
        }
    }

    /// Shows `file:line` with `symbol` selected (if known; else as
    /// `go_to_line` picks one), bringing back the source pane if it was
    /// hidden: you asked to see the line.
    pub(super) fn go_to_place(&mut self, symbol: Option<SymbolId>, file: &str, line: u32) {
        self.layout = self.layout.with_source_shown(true);
        self.place_source_at(symbol, file, line);
    }

    /// Like `go_to_place`, but leaves the panes as they are (to restore
    /// where you were).
    pub(super) fn place_source_at(&mut self, symbol: Option<SymbolId>, file: &str, line: u32) {
        match symbol.filter(|id| self.project.graph.symbols.contains_key(id)) {
            Some(id) => {
                self.graph_view.select(id);
                self.code_panel.reveal(id, file.to_string(), line);
            }
            None => self.go_to_line(file, line),
        }
    }

    /// Selects the symbol named on `line` (else the last one declared above
    /// it in `file`) and shows the line.
    fn go_to_line(&mut self, file: &str, line: u32) {
        let graph = &self.project.graph;
        let on_line = graph
            .occurrences_on_line(file, line)
            .first()
            .map(|o| o.symbol);
        let above = || {
            graph
                .symbols
                .values()
                .filter(|s| s.file == file && s.line <= line)
                .max_by_key(|s| (s.line, s.id))
                .map(|s| s.id)
        };
        let Some(id) = on_line.or_else(above).or_else(|| first_in(graph, file)) else {
            self.status = Status::Info(format!("{file} has no indexed symbols to show it with."));
            return;
        };
        self.graph_view.select(id);
        self.code_panel.reveal(id, file.to_string(), line);
    }

    /// `(index, label)` of every note, for the palette's `#` search.
    pub(super) fn note_candidates(&self) -> Vec<(usize, String)> {
        self.notebook
            .notes()
            .iter()
            .enumerate()
            .map(|(index, note)| {
                let star = if note.is_starred { "★ " } else { "" };
                let first_line: String = note
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(MAX_LABEL_CHARS)
                    .collect();
                let label = match first_line.as_str() {
                    "" => format!("{star}{}", target_label(&note.target)),
                    text => format!("{star}{} — {text}", target_label(&note.target)),
                };
                (index, label)
            })
            .collect()
    }

    /// Goes to the note at `index` in the notebook.
    pub(super) fn go_to_note_at(&mut self, index: usize) {
        let target = self
            .notebook
            .notes()
            .iter()
            .nth(index)
            .map(|n| n.target.clone());
        if let Some(target) = target {
            self.go_to_note(&target);
        }
    }

    fn unsaved_hint(&self) -> &'static str {
        if self.notebook.is_saved_to_disk() {
            ""
        } else {
            " (Demo notes aren't saved.)"
        }
    }
}

fn first_in(graph: &crate::model::CodeGraph, file: &str) -> Option<SymbolId> {
    graph
        .symbols
        .values()
        .filter(|s| s.file == file)
        .min_by_key(|s| (s.line, s.id))
        .map(|s| s.id)
}

/// How a target reads in messages and lists: `a::f` or `file.c:12`.
pub fn target_label(target: &NoteTarget) -> String {
    match target {
        NoteTarget::Symbol(name) => name.clone(),
        NoteTarget::Line { file, line } => format!("{file}:{line}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::palette::Command;

    fn demo_app() -> (SrcWranglrApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.open_demo(&ctx);
        (app, ctx)
    }

    fn select(app: &mut SrcWranglrApp, name: &str) -> SymbolId {
        let id = app.project.find(name).expect(name);
        app.graph_view.select(id);
        id
    }

    #[test]
    fn bookmark_command_stars_the_selected_symbol_and_toggles_back() {
        let (mut app, ctx) = demo_app();
        let id = select(&mut app, "Circle::area");
        app.run_command(Command::ToggleBookmark, &ctx);
        assert!(app.notebook.mark(id).is_some_and(|m| m.is_starred));
        app.run_command(Command::ToggleBookmark, &ctx);
        assert_eq!(app.notebook.mark(id), None);
    }

    #[test]
    fn note_commands_need_a_selection() {
        let (mut app, ctx) = demo_app();
        app.run_command(Command::EditNote, &ctx);
        assert!(app.note_editor.is_none());
        assert!(matches!(&app.status, Status::Info(t) if t.contains("Select a symbol")));
        select(&mut app, "main");
        app.run_command(Command::EditNote, &ctx);
        assert!(app.note_editor.is_some());
    }

    #[test]
    fn going_to_a_line_note_selects_a_symbol_on_it_and_shows_the_line() {
        let (mut app, _) = demo_app();
        let target = NoteTarget::Line {
            file: "main.cpp".into(),
            line: 11,
        };
        app.notebook
            .set(&app.project, target.clone(), "cast truncates", false)
            .expect("set");
        app.go_to_note(&target);
        let selected = app.graph_view.selected().expect("selected");
        // Line 11 is `return static_cast<int>(total_area(c, s));`.
        assert_eq!(
            app.project.graph.symbols[&selected].qualified_name,
            "total_area"
        );
        let position = app.code_panel.position(&app.project.graph, Some(selected));
        assert_eq!(
            position.map(|p| (p.file, p.line)),
            Some(("main.cpp".into(), 11))
        );
    }

    #[test]
    fn a_failed_save_keeps_the_editor_open_with_the_text() {
        let (mut app, _) = demo_app();
        let outside = NoteTarget::Line {
            file: "../outside.c".into(),
            line: 1,
        };
        app.edit_note(outside.clone());
        app.apply_editor_outcome(EditorOutcome::Save {
            target: outside,
            text: "long careful note".into(),
            is_starred: false,
        });
        assert!(app.note_editor.is_some());
        assert!(app.notebook.notes().is_empty());

        app.apply_editor_outcome(EditorOutcome::Cancel);
        assert!(app.note_editor.is_none());
    }

    #[test]
    fn deleting_says_deleted() {
        let (mut app, _) = demo_app();
        let target = NoteTarget::Symbol("main".into());
        app.notebook
            .set(&app.project, target.clone(), "x", false)
            .expect("set");
        app.edit_note(target.clone());
        app.apply_editor_outcome(EditorOutcome::Delete(target));
        assert!(app.note_editor.is_none());
        assert!(matches!(&app.status, Status::Info(t) if t.starts_with("Note deleted.")));
    }

    #[test]
    fn palette_note_entries_go_to_their_note() {
        let (mut app, _) = demo_app();
        let square = app.project.find("Square").expect("Square");
        app.notebook
            .set(
                &app.project,
                NoteTarget::Symbol("Square".into()),
                "4 sides",
                true,
            )
            .expect("set");
        let candidates = app.note_candidates();
        assert_eq!(candidates, [(0, "★ Square — 4 sides".to_string())]);
        app.go_to_note_at(0);
        assert_eq!(app.graph_view.selected(), Some(square));
    }
}
