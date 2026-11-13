//! The Journal tab of the Notes pane: free-form entries you write as you
//! audit (what you did, theories, what to try next), newest first, in
//! Markdown, each with when it was written and the place it was written
//! at (click it to go back there).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use egui::{KeyboardShortcut, Modifiers, RichText};

use super::readme::{shown_markdown, Block};
use super::readme_view::markdown;
use super::survey_view::ERROR_COLOR;
use super::visible::visible;
use crate::notes::journal::{date_time, Entry, Journal, Place};

/// Adds the entry being written.
pub const ADD_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Enter);
const DRAFT_ROWS: usize = 4;
/// Laid-out entries kept beyond the journal's own (old edits), before the
/// cache is emptied.
const SPARE_RENDERED: usize = 64;
/// An entry not drawn yet is guessed at this many rows at most.
const MAX_GUESSED_ROWS: usize = 40;

/// What the tab shows and where it is.
pub struct JournalContext<'a> {
    pub journal: &'a Journal,
    /// The place you are at, which a new entry is linked to.
    pub here: Option<&'a Place>,
    /// Where the journal is kept, or why it can't be saved (then it is
    /// read-only).
    pub storage: Result<&'a str, &'a str>,
    /// Unix seconds.
    pub now: u64,
}

/// What the user did in the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalAction {
    /// The journal as it should be now (an entry edited or deleted).
    Changed(Journal),
    /// An entry was added: the draft can be cleared once it is kept.
    Added(Journal),
    /// Show this place (and this symbol, by qualified name, if known).
    GoTo {
        file: String,
        line: u32,
        symbol: Option<String>,
    },
}

#[derive(Default)]
pub struct JournalView {
    draft: String,
    /// Whether a new entry is left unlinked from the place you are at.
    is_unlinked: bool,
    /// The entry being edited (by index) and its text so far.
    editing: Option<(usize, String)>,
    /// The entry whose deletion waits for a second click.
    confirming_delete: Option<usize>,
    query: String,
    /// What went wrong with the last change.
    error: Option<String>,
    /// Each entry's text, laid out once, by a hash of the text.
    rendered: HashMap<u64, Vec<Block>>,
    /// How tall each entry was drawn (or is guessed to be, until it is),
    /// so only those in view are drawn.
    heights: HashMap<EntryKey, f32>,
    /// The entries the query last matched.
    search: Option<Search>,
    /// Put the keyboard in the draft on the next frame.
    wants_focus: bool,
}

impl JournalView {
    pub fn clear_draft(&mut self) {
        self.draft.clear();
        self.error = None;
    }

    /// Puts the keyboard in the draft.
    pub fn focus_draft(&mut self) {
        self.wants_focus = true;
    }

    /// Draws the tab.
    pub fn show(&mut self, ui: &mut egui::Ui, context: &JournalContext) -> Option<JournalAction> {
        match context.storage {
            Ok(place) => ui.weak(place),
            Err(error) => ui.label(RichText::new(visible(error)).color(ERROR_COLOR)),
        };
        let is_editable = context.storage.is_ok();
        let mut action = ui
            .add_enabled_ui(is_editable, |ui| self.new_entry(ui, context))
            .inner;
        if let Some(error) = &self.error {
            ui.label(RichText::new(visible(error)).color(ERROR_COLOR));
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Search the journal…")
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        let entries = context.journal.entries();
        if entries.is_empty() {
            ui.weak(
                "Nothing written yet. Keep a log here as you go: what you looked at, what you \
                 think, what to try next.",
            );
            return action;
        }
        if self.rendered.len() > entries.len() + SPARE_RENDERED {
            self.rendered.clear();
        }
        if self.heights.len() > entries.len() + SPARE_RENDERED {
            self.heights.clear();
        }
        let shown = self.matching(entries);
        egui::ScrollArea::vertical()
            .id_salt("journal_entries")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in shown {
                    let picked = self.entry_in_view(ui, context, index, is_editable);
                    action = picked.or(action.take());
                }
            });
        action
    }

    /// The indices of the entries the query matches, searched again only
    /// when the query or the journal changes.
    fn matching(&mut self, entries: &[Entry]) -> Vec<usize> {
        let fingerprint = fingerprint(entries);
        let is_current = self
            .search
            .as_ref()
            .is_some_and(|s| s.query == self.query && s.fingerprint == fingerprint);
        if !is_current {
            self.search = Some(Search {
                query: self.query.clone(),
                fingerprint,
                shown: shown_entries(entries, &self.query),
            });
        }
        self.search
            .as_ref()
            .map_or_else(Vec::new, |s| s.shown.clone())
    }

    /// Entry `index` if it is in view; else just the room it takes, so a
    /// long journal costs only what shows.
    fn entry_in_view(
        &mut self,
        ui: &mut egui::Ui,
        context: &JournalContext,
        index: usize,
        is_editable: bool,
    ) -> Option<JournalAction> {
        let entry = &context.journal.entries()[index];
        let key = (entry.created, entry.updated, entry.text.len());
        let height = *self
            .heights
            .entry(key)
            .or_insert_with(|| guessed_height(ui, entry));
        let room = egui::vec2(ui.available_width(), height);
        let is_editing = self.editing.as_ref().is_some_and(|(i, _)| *i == index);
        if !is_editing && !ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, room)) {
            ui.allocate_space(room);
            return None;
        }
        let top = ui.cursor().top();
        let action = self.entry(ui, context, index, is_editable);
        // `allocate_space` adds the spacing after it again.
        let drawn = ui.cursor().top() - top - ui.spacing().item_spacing.y;
        self.heights.insert(key, drawn.max(0.0));
        action
    }

    /// The box to write a new entry in, and Add.
    fn new_entry(&mut self, ui: &mut egui::Ui, context: &JournalContext) -> Option<JournalAction> {
        let draft_id = ui.make_persistent_id("journal_draft");
        // Taken before the box sees it, so it doesn't add a line.
        let is_shortcut = ui.memory(|m| m.has_focus(draft_id))
            && ui.input_mut(|i| i.consume_shortcut(&ADD_SHORTCUT));
        let draft = ui.add(
            egui::TextEdit::multiline(&mut self.draft)
                .id(draft_id)
                .hint_text("Write an entry: what you did, what you think… (Markdown)")
                .desired_rows(DRAFT_ROWS)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.wants_focus) {
            draft.request_focus();
        }
        let has_text = !self.draft.trim().is_empty();
        let mut is_linked = !self.is_unlinked;
        let is_added = ui
            .horizontal_wrapped(|ui| {
                let keys = ui.ctx().format_shortcut(&ADD_SHORTCUT);
                let add = ui
                    .add_enabled(has_text, egui::Button::new("Add Entry"))
                    .on_hover_text(format!("Add it to the journal ({keys})"));
                if let Some(here) = context.here {
                    let place = place_label(&here.file, here.line, here.symbol.as_deref());
                    ui.checkbox(&mut is_linked, format!("Link to {}", visible(&place)))
                        .on_hover_text("Click the entry later to come back here");
                }
                add.clicked() || (is_shortcut && has_text)
            })
            .inner;
        self.is_unlinked = !is_linked;
        if !is_added {
            return None;
        }
        let place = context.here.filter(|_| is_linked).cloned();
        let entry = Entry::new(&self.draft, place, context.now);
        self.kept(context.journal.with_added(entry), JournalAction::Added)
    }

    /// One entry: when, where (click to go there), its text, and Edit /
    /// Delete.
    fn entry(
        &mut self,
        ui: &mut egui::Ui,
        context: &JournalContext,
        index: usize,
        is_editable: bool,
    ) -> Option<JournalAction> {
        let entry = &context.journal.entries()[index];
        let picked = entry_heading(ui, entry);
        if self.editing.as_ref().is_some_and(|(i, _)| *i == index) {
            return self.editor(ui, context).or(picked);
        }
        let blocks = self
            .rendered
            .entry(text_hash(&entry.text))
            .or_insert_with(|| shown_markdown(&entry.text));
        ui.indent(("journal_entry", index), |ui| markdown(ui, blocks));
        let changed = match is_editable {
            true => self.entry_buttons(ui, context.journal, index),
            false => None,
        };
        ui.separator();
        changed.or(picked)
    }

    fn entry_buttons(
        &mut self,
        ui: &mut egui::Ui,
        journal: &Journal,
        index: usize,
    ) -> Option<JournalAction> {
        let is_confirming = self.confirming_delete == Some(index);
        let (is_edit, is_delete, is_keep, is_ask) = ui
            .horizontal(|ui| {
                let is_edit = ui.small_button("Edit").clicked();
                if !is_confirming {
                    let is_ask = ui.small_button("Delete…").clicked();
                    return (is_edit, false, false, is_ask);
                }
                let is_delete = ui.small_button("Delete It").clicked();
                let is_keep = ui.small_button("Keep It").clicked();
                (is_edit, is_delete, is_keep, false)
            })
            .inner;
        if is_edit {
            let text = journal.entries()[index].text.clone();
            self.editing = Some((index, text));
            self.confirming_delete = None;
            return None;
        }
        if is_ask {
            self.confirming_delete = Some(index);
        }
        if is_keep {
            self.confirming_delete = None;
        }
        if !is_delete {
            return None;
        }
        // Indices after this one shift down.
        self.confirming_delete = None;
        self.editing = None;
        Some(JournalAction::Changed(journal.without(index)))
    }

    /// The entry being edited, with Save and Cancel.
    fn editor(&mut self, ui: &mut egui::Ui, context: &JournalContext) -> Option<JournalAction> {
        let (index, text) = self.editing.as_mut()?;
        let index = *index;
        ui.add(
            egui::TextEdit::multiline(text)
                .hint_text("The entry's text (Markdown)")
                .desired_rows(DRAFT_ROWS)
                .desired_width(f32::INFINITY),
        );
        let (is_saved, is_cancelled) = ui
            .horizontal(|ui| (ui.button("Save").clicked(), ui.button("Cancel").clicked()))
            .inner;
        ui.separator();
        if is_cancelled {
            self.editing = None;
            return None;
        }
        if !is_saved {
            return None;
        }
        let text = self.editing.take().map(|(_, t)| t).unwrap_or_default();
        let edited = context.journal.with_text(index, &text, context.now);
        let action = self.kept(edited, JournalAction::Changed);
        if action.is_none() {
            // Keep editing what was refused.
            self.editing = Some((index, text));
        }
        action
    }

    /// `changed` as an action, or its error shown.
    fn kept(
        &mut self,
        changed: Result<Journal, String>,
        action: fn(Journal) -> JournalAction,
    ) -> Option<JournalAction> {
        match changed {
            Ok(journal) => {
                self.error = None;
                Some(action(journal))
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

/// When an entry was written, and where (a link back there).
fn entry_heading(ui: &mut egui::Ui, entry: &Entry) -> Option<JournalAction> {
    ui.horizontal_wrapped(|ui| {
        let edited = if entry.updated == entry.created {
            ""
        } else {
            " (edited)"
        };
        ui.weak(format!("{} UTC{edited}", date_time(entry.created)));
        let (file, line) = entry.place()?;
        let label = place_label(file, line, entry.symbol.as_deref());
        let link = ui
            .link(visible(&label).into_owned())
            .on_hover_text("Go there");
        link.clicked().then(|| JournalAction::GoTo {
            file: file.to_string(),
            line,
            symbol: entry.symbol.clone(),
        })
    })
    .inner
}

/// The indices of the entries `query` matches (case-insensitively, in
/// their text, file or symbol), newest first.
pub fn shown_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let contains = |text: Option<&str>| text.is_some_and(|t| t.to_lowercase().contains(&needle));
    let matches = |entry: &Entry| {
        needle.is_empty()
            || contains(Some(&entry.text))
            || contains(entry.file.as_deref())
            || contains(entry.symbol.as_deref())
    };
    (0..entries.len())
        .rev()
        .filter(|&i| matches(&entries[i]))
        .collect()
}

/// An entry's identity for its drawn height: (created, updated, length).
type EntryKey = (u64, u64, usize);

/// What the query matched, and in which journal.
struct Search {
    query: String,
    fingerprint: u64,
    shown: Vec<usize>,
}

/// Changes when an entry is added, edited or deleted.
fn fingerprint(entries: &[Entry]) -> u64 {
    let mut hasher = DefaultHasher::new();
    entries.len().hash(&mut hasher);
    for entry in entries {
        (entry.created, entry.updated, entry.text.len()).hash(&mut hasher);
    }
    hasher.finish()
}

/// How tall an entry not drawn yet probably is: its lines, and its
/// heading and buttons.
fn guessed_height(ui: &egui::Ui, entry: &Entry) -> f32 {
    let rows = entry.text.lines().take(MAX_GUESSED_ROWS).count() + 3;
    rows as f32 * ui.text_style_height(&egui::TextStyle::Body)
}

/// `net/parse.c:42 · parse_header`.
fn place_label(file: &str, line: u32, symbol: Option<&str>) -> String {
    match symbol {
        Some(symbol) => format!("{file}:{line} · {symbol}"),
        None => format!("{file}:{line}"),
    }
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[path = "journal_view_tests.rs"]
mod tests;
