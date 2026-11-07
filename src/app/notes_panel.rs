//! The Notes & Bookmarks panel on the right: every note of the project,
//! newest first, filterable; click one to go there.

use super::layout::Layout;
use egui::RichText;

use super::notebook::Notebook;
use super::notes_actions::target_label;
use super::pane_bar::{self, Edge};
use crate::notes::store::{NOTES_DIR, NOTES_FILE};
use crate::notes::{Note, NoteTarget, Notes};

const DEFAULT_WIDTH: f32 = 280.0;
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotesFilter {
    #[default]
    All,
    Bookmarks,
    WithText,
}

impl NotesFilter {
    const ALL: [NotesFilter; 3] = [
        NotesFilter::All,
        NotesFilter::Bookmarks,
        NotesFilter::WithText,
    ];

    fn label(self) -> &'static str {
        match self {
            NotesFilter::All => "All",
            NotesFilter::Bookmarks => "Bookmarks",
            NotesFilter::WithText => "Notes",
        }
    }

    fn accepts(self, note: &Note) -> bool {
        match self {
            NotesFilter::All => true,
            NotesFilter::Bookmarks => note.is_starred,
            NotesFilter::WithText => !note.text.is_empty(),
        }
    }
}

/// What the user picked in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesPanelAction {
    GoTo(NoteTarget),
    Edit(NoteTarget),
}

#[derive(Default)]
pub struct NotesPanel {
    is_open: bool,
    filter: NotesFilter,
    query: String,
}

impl NotesPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Draws the panel (while open, sliding in and out) at the right of
    /// `ui`.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        notebook: &Notebook,
        layout: Layout,
    ) -> Option<NotesPanelAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("notes_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| self.contents(ui, notebook));
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(&mut self, ui: &mut egui::Ui, notebook: &Notebook) -> Option<NotesPanelAction> {
        let title = format!("Notes & Bookmarks ({})", notebook.notes().len());
        if pane_bar::heading(ui, title, Edge::Right) {
            self.is_open = false;
        }
        storage_line(ui, notebook);
        ui.horizontal(|ui| {
            for filter in NotesFilter::ALL {
                ui.selectable_value(&mut self.filter, filter, filter.label());
            }
        });
        let search = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Search notes…")
            .desired_width(f32::INFINITY);
        ui.add(search);
        ui.separator();
        let notes = listed(notebook.notes(), self.filter, &self.query);
        if notes.is_empty() {
            ui.weak(empty_hint(notebook.notes().is_empty()));
            return None;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                notes.iter().filter_map(|note| note_row(ui, note)).last()
            })
            .inner
    }
}

/// Where the notes are kept, or why they can't be saved.
fn storage_line(ui: &mut egui::Ui, notebook: &Notebook) {
    match notebook.load_error() {
        Some(error) => {
            ui.label(RichText::new(format!("Read-only: {error}")).color(ERROR_COLOR));
        }
        None if notebook.is_saved_to_disk() => {
            ui.weak(format!("Saved in {NOTES_DIR}/{NOTES_FILE}"));
        }
        None => {
            ui.weak("Demo project: notes aren't saved");
        }
    }
}

fn empty_hint(has_no_notes: bool) -> &'static str {
    if has_no_notes {
        "No notes yet. Use ☆ or Note… above the code, or right-click a line."
    } else {
        "No matching notes."
    }
}

/// One note: its target (click to go there), its text, and an Edit button.
fn note_row(ui: &mut egui::Ui, note: &Note) -> Option<NotesPanelAction> {
    let star = if note.is_starred { "★ " } else { "" };
    let mut action = None;
    ui.horizontal(|ui| {
        let target =
            ui.link(RichText::new(format!("{star}{}", target_label(&note.target))).monospace());
        if target.on_hover_text("Go there").clicked() {
            action = Some(NotesPanelAction::GoTo(note.target.clone()));
        }
        if ui.small_button("Edit").clicked() {
            action = Some(NotesPanelAction::Edit(note.target.clone()));
        }
    });
    if !note.text.is_empty() {
        ui.add(egui::Label::new(&note.text).wrap());
    }
    ui.separator();
    action
}

/// The notes `filter` and `query` (case-insensitive, on the target and
/// text) select, most recently changed first.
pub fn listed<'a>(notes: &'a Notes, filter: NotesFilter, query: &str) -> Vec<&'a Note> {
    let needle = query.trim().to_lowercase();
    let matches = |note: &Note| {
        needle.is_empty()
            || target_label(&note.target).to_lowercase().contains(&needle)
            || note.text.to_lowercase().contains(&needle)
    };
    let mut listed: Vec<&Note> = notes
        .iter()
        .filter(|n| filter.accepts(n) && matches(n))
        .collect();
    listed.sort_by_key(|n| std::cmp::Reverse(n.updated));
    listed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Notes {
        let mut notes = Notes::default();
        notes.set(NoteTarget::Symbol("a::parse".into()), "", true, None, 1);
        notes.set(
            NoteTarget::Line {
                file: "net.c".into(),
                line: 40,
            },
            "Length from the packet, unchecked",
            false,
            None,
            3,
        );
        notes.set(
            NoteTarget::Symbol("b::Close".into()),
            "double free?",
            true,
            None,
            2,
        );
        notes
    }

    fn labels(listed: &[&Note]) -> Vec<String> {
        listed.iter().map(|n| target_label(&n.target)).collect()
    }

    #[test]
    fn lists_newest_first() {
        let notes = notes();
        assert_eq!(
            labels(&listed(&notes, NotesFilter::All, "")),
            ["net.c:40", "b::Close", "a::parse"]
        );
    }

    #[test]
    fn filters_bookmarks_and_notes_with_text() {
        let notes = notes();
        assert_eq!(
            labels(&listed(&notes, NotesFilter::Bookmarks, "")),
            ["b::Close", "a::parse"]
        );
        assert_eq!(
            labels(&listed(&notes, NotesFilter::WithText, "")),
            ["net.c:40", "b::Close"]
        );
    }

    #[test]
    fn query_matches_target_or_text_ignoring_case() {
        let notes = notes();
        assert_eq!(
            labels(&listed(&notes, NotesFilter::All, "PACKET")),
            ["net.c:40"]
        );
        assert_eq!(
            labels(&listed(&notes, NotesFilter::All, " parse ")),
            ["a::parse"]
        );
        assert!(listed(&notes, NotesFilter::Bookmarks, "packet").is_empty());
    }
}
