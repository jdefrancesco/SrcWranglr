//! The note editor: a modal for one note's text and bookmark star.
//! ⌘Enter / Ctrl+Enter saves, Esc or clicking outside cancels.

use egui::{Id, Key, KeyboardShortcut, Modifiers};

use crate::notes::store::MAX_TEXT_BYTES;
use crate::notes::{Note, NoteTarget};

const SAVE_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
const TEXT_WIDTH: f32 = 480.0;
const TEXT_ROWS: usize = 8;
/// Characters the text box takes: at most 4 bytes each, so it always fits
/// the notes file's per-note limit.
const MAX_CHARS: usize = MAX_TEXT_BYTES / 4;
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// What the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorOutcome {
    Save {
        target: NoteTarget,
        text: String,
        is_starred: bool,
    },
    Delete(NoteTarget),
    Cancel,
}

pub struct NoteEditor {
    target: NoteTarget,
    /// What the note is on, e.g. `leveldb::DBImpl::Write`.
    title: String,
    text: String,
    is_starred: bool,
    is_existing: bool,
    /// Text and star when opened, to tell whether there are changes to lose.
    original: (String, bool),
    /// Why the last save failed; the editor stays open so nothing is lost.
    error: Option<String>,
}

impl NoteEditor {
    pub fn new(target: NoteTarget, title: impl Into<String>, existing: Option<&Note>) -> Self {
        let text = existing.map(|n| n.text.clone()).unwrap_or_default();
        let is_starred = existing.is_some_and(|n| n.is_starred);
        Self {
            target,
            title: title.into(),
            original: (text.clone(), is_starred),
            text,
            is_starred,
            is_existing: existing.is_some(),
            error: None,
        }
    }

    /// Keeps the editor open, showing why saving failed.
    pub fn show_error(&mut self, error: String) {
        self.error = Some(error);
    }

    fn has_changes(&self) -> bool {
        (self.text.as_str(), self.is_starred) != (self.original.0.as_str(), self.original.1)
    }

    /// Draws the editor. Returns the outcome once the user is done.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<EditorOutcome> {
        let (save_keys, escape) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&SAVE_SHORTCUT),
                i.key_pressed(Key::Escape),
            )
        });
        let response = egui::Modal::new(Id::unique("note_editor")).show(ctx, |ui| {
            ui.set_width(TEXT_WIDTH);
            ui.heading(format!("Note on {}", self.title));
            let text = egui::TextEdit::multiline(&mut self.text)
                .hint_text("Write a note…")
                .char_limit(MAX_CHARS)
                .desired_rows(TEXT_ROWS)
                .desired_width(f32::INFINITY);
            let text = ui.add(text);
            if !text.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
                text.request_focus();
            }
            ui.checkbox(&mut self.is_starred, "Bookmark");
            if let Some(error) = &self.error {
                ui.colored_label(ERROR_COLOR, error);
            }
            ui.separator();
            ui.horizontal(|ui| self.buttons(ui, save_keys)).inner
        });
        // A stray click outside mustn't throw away changes; Esc still does.
        let should_close = response.should_close() && (escape || !self.has_changes());
        response
            .inner
            .or_else(|| should_close.then_some(EditorOutcome::Cancel))
    }

    fn buttons(&self, ui: &mut egui::Ui, save_keys: bool) -> Option<EditorOutcome> {
        let save_hint = ui.ctx().format_shortcut(&SAVE_SHORTCUT);
        let save = ui.add(egui::Button::new("Save").shortcut_text(save_hint));
        if save.clicked() || save_keys {
            return Some(EditorOutcome::Save {
                target: self.target.clone(),
                text: self.text.clone(),
                is_starred: self.is_starred,
            });
        }
        if ui.button("Cancel").clicked() {
            return Some(EditorOutcome::Cancel);
        }
        let delete = ui.add_enabled(self.is_existing, egui::Button::new("Delete"));
        delete
            .clicked()
            .then(|| EditorOutcome::Delete(self.target.clone()))
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;

    struct State {
        editor: NoteEditor,
        outcome: Option<EditorOutcome>,
    }

    fn target() -> NoteTarget {
        NoteTarget::Symbol("f".into())
    }

    fn harness(existing: Option<&Note>) -> Harness<'static, State> {
        let state = State {
            editor: NoteEditor::new(target(), "f", existing),
            outcome: None,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(outcome) = state.editor.show(&ctx) {
                    state.outcome = Some(outcome);
                }
            },
            state,
        );
        harness.run();
        harness
    }

    fn note(text: &str, is_starred: bool) -> Note {
        Note {
            target: target(),
            text: text.into(),
            is_starred,
            anchor: None,
            created: 1,
            updated: 1,
        }
    }

    #[test]
    fn typing_and_saving_returns_the_text_and_star() {
        let mut harness = harness(None);
        harness
            .get_by_role(egui::Role::MultilineTextInput)
            .type_text("bounds unchecked");
        harness.get_by_label("Bookmark").click();
        harness.run();
        harness.get_by_label_contains("Save").click();
        harness.run();
        assert_eq!(
            harness.state().outcome,
            Some(EditorOutcome::Save {
                target: target(),
                text: "bounds unchecked".into(),
                is_starred: true
            })
        );
    }

    #[test]
    fn command_enter_saves_an_existing_note_unchanged() {
        let existing = note("old", true);
        let mut harness = harness(Some(&existing));
        harness.key_press_modifiers(SAVE_SHORTCUT.modifiers, SAVE_SHORTCUT.logical_key);
        harness.run();
        assert_eq!(
            harness.state().outcome,
            Some(EditorOutcome::Save {
                target: target(),
                text: "old".into(),
                is_starred: true
            })
        );
    }

    #[test]
    fn delete_is_only_offered_for_existing_notes() {
        let mut harness = harness(None);
        harness.get_by_label("Delete").click();
        harness.run();
        assert_eq!(harness.state().outcome, None);

        let existing = note("x", false);
        let mut harness = self::harness(Some(&existing));
        harness.get_by_label("Delete").click();
        harness.run();
        assert_eq!(
            harness.state().outcome,
            Some(EditorOutcome::Delete(target()))
        );
    }

    fn click_outside(harness: &mut Harness<State>) {
        let corner = egui::pos2(2.0, 2.0);
        harness.hover_at(corner);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: corner,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run();
    }

    #[test]
    fn a_stray_click_outside_keeps_changes_but_closes_an_unchanged_editor() {
        let mut harness = harness(None);
        harness
            .get_by_role(egui::Role::MultilineTextInput)
            .type_text("half-written thought");
        harness.run();
        click_outside(&mut harness);
        assert_eq!(harness.state().outcome, None);

        let mut harness = self::harness(None);
        click_outside(&mut harness);
        assert_eq!(harness.state().outcome, Some(EditorOutcome::Cancel));
    }

    #[test]
    fn save_errors_are_shown_in_the_editor() {
        let mut harness = harness(None);
        harness
            .state_mut()
            .editor
            .show_error("Could not save notes: disk full".into());
        harness.run();
        harness.get_by_label("Could not save notes: disk full");
    }

    #[test]
    fn escape_cancels() {
        let mut harness = harness(None);
        harness.key_press(Key::Escape);
        harness.run();
        assert_eq!(harness.state().outcome, Some(EditorOutcome::Cancel));
    }
}
