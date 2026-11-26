//! The Notes pane on the right. Its Notes & Bookmarks tab lists every
//! note of the project, newest first, filterable (click one to go there);
//! its Journal tab is the project's free-form audit log.

use super::layout::Layout;
use egui::RichText;

use super::journal_view::{JournalAction, JournalContext, JournalView};
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
    Journal(JournalAction),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum NotesTab {
    #[default]
    Notes,
    Journal,
}

#[derive(Default)]
pub struct NotesPanel {
    is_open: bool,
    tab: NotesTab,
    filter: NotesFilter,
    query: String,
    journal: JournalView,
}

impl NotesPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Opens the pane on the Journal tab, ready to write in.
    pub fn open_journal(&mut self) {
        self.is_open = true;
        self.tab = NotesTab::Journal;
        self.journal.focus_draft();
    }

    pub fn journal_view(&mut self) -> &mut JournalView {
        &mut self.journal
    }

    /// Forgets what was being written for the last project.
    pub fn reset_journal(&mut self) {
        self.journal = JournalView::default();
    }

    /// Draws the panel (while open, sliding in and out) at the right of
    /// `ui`.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        notebook: &Notebook,
        journal: &JournalContext,
        layout: Layout,
    ) -> Option<NotesPanelAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("notes_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| self.contents(ui, notebook, journal));
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        notebook: &Notebook,
        journal: &JournalContext,
    ) -> Option<NotesPanelAction> {
        // Claim the pane's full width so a drag-resize sticks: egui stores
        // the panel at its content's width on release, and a short tab or a
        // wide font can otherwise leave the content narrower than the drag,
        // snapping the pane back.
        ui.set_min_width(ui.available_width());
        let title = match self.tab {
            NotesTab::Notes => format!("Notes & Bookmarks ({})", notebook.notes().len()),
            NotesTab::Journal => format!("Journal ({})", journal.journal.entries().len()),
        };
        if pane_bar::heading(ui, title, Edge::Right) {
            self.is_open = false;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, NotesTab::Notes, "Notes & Bookmarks");
            ui.selectable_value(&mut self.tab, NotesTab::Journal, "Journal")
                .on_hover_text("A log of your audit: what you did, theories, what to try next");
        });
        ui.separator();
        let action = match self.tab {
            NotesTab::Notes => self.notes(ui, notebook),
            NotesTab::Journal => self
                .journal
                .show(ui, journal)
                .map(NotesPanelAction::Journal),
        };
        // Claim the pane's full width unconditionally. egui stores a
        // resizable panel at its content's width when the drag is released
        // (and on any later re-layout frame), so a tab whose content wraps
        // to just under the width — the Journal's markdown entries, which
        // the agent also rewrites live — would otherwise let the pane snap
        // back. A full-width strip pins the stored width to the pane.
        ui.allocate_space(egui::vec2(ui.available_width(), 0.0));
        action
    }

    fn notes(&mut self, ui: &mut egui::Ui, notebook: &Notebook) -> Option<NotesPanelAction> {
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

#[cfg(test)]
mod resize_tests {
    use egui::{pos2, vec2};

    use super::*;
    use crate::app::project::Project;
    use crate::notes::journal::Journal;

    struct S {
        panel: NotesPanel,
        notebook: Notebook,
        journal: Journal,
        central_avail: f32,
    }

    /// Drags the Notes pane's left edge inward to widen it, and reports the
    /// central space left before and after (less space = a wider pane).
    fn widen_notes() -> (f32, f32) {
        let project = Project::demo().expect("demo");
        let mut panel = NotesPanel::default();
        panel.set_open(true);
        let mut harness = crate::app::fonts::test_harness()
            .with_size(vec2(800.0, 600.0))
            .with_accessibility_check(false)
            .build_ui_state(
                |ui, s: &mut S| {
                    let context = JournalContext {
                        journal: &s.journal,
                        here: None,
                        storage: Ok("in memory"),
                        now: 0,
                    };
                    s.panel.show(ui, &s.notebook, &context, Layout::default());
                    s.central_avail = ui.available_width();
                },
                S {
                    panel,
                    notebook: Notebook::open(&project),
                    journal: Journal::default(),
                    central_avail: 0.0,
                },
            );
        let settle = |h: &mut egui_kittest::Harness<S>| (0..40).for_each(|_| h.step());
        settle(&mut harness);
        let before = harness.state().central_avail;
        // The 280-wide pane sits at the right; its handle is near x=520.
        harness.drag_at(pos2(520.0, 300.0));
        harness.step();
        harness.hover_at(pos2(470.0, 300.0));
        harness.step();
        harness.hover_at(pos2(420.0, 300.0));
        harness.step();
        harness.drop_at(pos2(420.0, 300.0));
        settle(&mut harness);
        (before, harness.state().central_avail)
    }

    #[test]
    fn dragging_the_edge_widens_the_pane_and_it_sticks() {
        let (before, after) = widen_notes();
        assert!(
            after < before - 40.0,
            "the pane should stay wider after release: central space {before} -> {after}"
        );
    }

    fn widen_journal(with_entry: bool) -> (f32, f32) {
        let project = Project::demo().expect("demo");
        let mut panel = NotesPanel::default();
        panel.set_open(true);
        panel.open_journal();
        let mut journal = Journal::default();
        if with_entry {
            journal = journal
                .with_added(crate::notes::journal::Entry::new(
                    "looked at parse_header",
                    None,
                    0,
                ))
                .expect("added");
        }
        let mut harness = crate::app::fonts::test_harness()
            .with_size(vec2(800.0, 600.0))
            .with_accessibility_check(false)
            .build_ui_state(
                |ui, s: &mut S| {
                    let context = JournalContext {
                        journal: &s.journal,
                        here: None,
                        storage: Ok("in memory"),
                        now: 0,
                    };
                    s.panel.show(ui, &s.notebook, &context, Layout::default());
                    s.central_avail = ui.available_width();
                },
                S {
                    panel,
                    notebook: Notebook::open(&project),
                    journal,
                    central_avail: 0.0,
                },
            );
        let settle = |h: &mut egui_kittest::Harness<S>| (0..40).for_each(|_| h.step());
        settle(&mut harness);
        let before = harness.state().central_avail;
        harness.drag_at(pos2(520.0, 300.0));
        harness.step();
        harness.hover_at(pos2(470.0, 300.0));
        harness.step();
        harness.hover_at(pos2(420.0, 300.0));
        harness.step();
        harness.drop_at(pos2(420.0, 300.0));
        settle(&mut harness);
        (before, harness.state().central_avail)
    }

    /// Long entries full of unbreakable path/crash-id tokens must not widen
    /// the content-sized pane. Before the Markdown wrapped such tokens, one
    /// over-wide line made the pane snap to the content and balloon across the
    /// window; the pane must instead hold its width and stay put.
    #[test]
    fn the_journal_tab_does_not_balloon_the_pane() {
        let project = Project::demo().expect("demo");
        let mut panel = NotesPanel::default();
        panel.set_open(true);
        panel.open_journal();
        let longs = [
            "Confirmed the SDP out-of-bounds read in `rtp.c:313` (stream peek, no NUL sentinel). Artifacts: `.aavr/crashes/sdp-oob-196/`, `.aavr/crashes/docker-easy/{crash-4b8b8c1e,sdp2-oob-196}`.",
            "AV1 sequence header 45, parseAttributes 45, ParseFlacPicture 40, config_StringUnescape 40, json_unescape 36 with a reachability blocker. Verified real callers for each.",
        ];
        let journal = (0..10).fold(Journal::default(), |j, i| {
            j.with_added(crate::notes::journal::Entry::new(
                longs[i % longs.len()],
                None,
                i as u64,
            ))
            .expect("added")
        });
        let mut harness = crate::app::fonts::test_harness()
            .with_size(vec2(900.0, 600.0))
            .with_accessibility_check(false)
            .build_ui_state(
                |ui, s: &mut S| {
                    let context = JournalContext {
                        journal: &s.journal,
                        here: None,
                        storage: Ok("in memory"),
                        now: 0,
                    };
                    s.panel.show(ui, &s.notebook, &context, Layout::default());
                    s.central_avail = ui.available_width();
                },
                S {
                    panel,
                    notebook: Notebook::open(&project),
                    journal,
                    central_avail: 0.0,
                },
            );
        (0..30).for_each(|_| harness.step());
        let central = harness.state().central_avail;
        // The pane holds near its ~280px default, leaving most of the 900px
        // window as central space. A ballooned pane would shrink this toward 0.
        assert!(
            central > 500.0,
            "the Journal tab ballooned the pane: only {central} central of 900"
        );
    }

    #[test]
    fn the_journal_tab_resizes_and_sticks() {
        let (before, after) = widen_journal(true);
        assert!(
            after < before - 40.0,
            "the Journal tab should stay wider after release: central space {before} -> {after}"
        );
    }
}
