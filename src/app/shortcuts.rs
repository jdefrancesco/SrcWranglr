//! The keyboard shortcuts that work anywhere in the window.

use super::palette::Command;
use super::pane_bar::Pane;
use super::survey_view::SurveyTab;
use super::{
    SrcWranglrApp, BOOKMARK_SHORTCUT, EDITOR_SHORTCUT, ENTRY_POINT_SHORTCUT, FINDINGS_SHORTCUT,
    JOURNAL_SHORTCUT, LAYOUT_SHORTCUT, NOTES_PANEL_SHORTCUT, NOTE_SHORTCUT, OPEN_SHORTCUT,
    SAVE_SESSION_SHORTCUT, SETTINGS_SHORTCUT, TRIAGE_SHORTCUT,
};

impl SrcWranglrApp {
    pub(super) fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        // Off macOS, ⌘ is Ctrl, and Ctrl+letter edits text in text fields.
        let can_use = cfg!(target_os = "macos") || !ctx.egui_wants_keyboard_input();
        // ⌘⇧E before ⌘E: egui matches ⌘E even with Shift held.
        let (open, editor, entry, bookmark, note, panel) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&OPEN_SHORTCUT),
                can_use && i.consume_shortcut(&EDITOR_SHORTCUT),
                can_use && i.consume_shortcut(&ENTRY_POINT_SHORTCUT),
                can_use && i.consume_shortcut(&BOOKMARK_SHORTCUT),
                can_use && i.consume_shortcut(&NOTE_SHORTCUT),
                can_use && i.consume_shortcut(&NOTES_PANEL_SHORTCUT),
            )
        });
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&FINDINGS_SHORTCUT)) {
            self.run_command(Command::TogglePane(Pane::Findings), ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&TRIAGE_SHORTCUT)) {
            self.run_command(Command::Triage(SurveyTab::Overview), ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&LAYOUT_SHORTCUT)) {
            self.run_command(Command::SwapSourcePane, ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&SAVE_SESSION_SHORTCUT)) {
            self.run_command(Command::SaveSession, ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&JOURNAL_SHORTCUT)) {
            self.run_command(Command::Journal, ctx);
        }
        if can_use && ctx.input_mut(|i| i.consume_shortcut(&SETTINGS_SHORTCUT)) {
            self.open_settings();
        }
        if panel {
            self.run_command(Command::TogglePane(Pane::Notes), ctx);
        }
        if bookmark {
            self.run_command(Command::ToggleBookmark, ctx);
        }
        if note {
            self.run_command(Command::EditNote, ctx);
        }
        if open {
            self.open_project_dialog(ctx);
        }
        if editor {
            self.open_in_editor();
        }
        if entry {
            self.go_to_entry_point();
        }
    }
}
