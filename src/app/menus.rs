//! The menu bar: File, Go, Analyze, and Triage.

use egui::RichText;

use super::palette::{self, Command, OPEN_COMMAND_SHORTCUT, OPEN_SYMBOL_SHORTCUT};
use super::survey_view::SurveyTab;
use super::{
    SlurcetrailApp, BOOKMARK_SHORTCUT, EDITOR_SHORTCUT, ENTRY_POINT_SHORTCUT, FINDINGS_SHORTCUT,
    MAX_ENTRY_MENU_ITEMS, NOTES_PANEL_SHORTCUT, NOTE_SHORTCUT, OPEN_SHORTCUT, TRIAGE_SHORTCUT,
};

impl SlurcetrailApp {
    pub(super) fn show_menu_bar(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        egui::Panel::top("menu_bar")
            .show(ui, |ui| {
                egui::MenuBar::new()
                    .ui(ui, |ui| {
                        let file = ui.menu_button("File", |ui| self.file_menu(ui));
                        let go = ui.menu_button("Go", |ui| self.go_menu(ui));
                        let analyze = ui.menu_button("Analyze", |ui| self.analyze_menu(ui));
                        let triage = ui.menu_button("Triage", triage_menu);
                        let action = file
                            .inner
                            .flatten()
                            .or(go.inner.flatten())
                            .or(analyze.inner.flatten())
                            .or(triage.inner.flatten());
                        ui.separator();
                        ui.label(RichText::new(&self.project.name).strong());
                        action
                    })
                    .inner
            })
            .inner
    }

    pub(super) fn file_menu(&self, ui: &mut egui::Ui) -> Option<Command> {
        let open = egui::Button::new("Open Project…")
            .shortcut_text(ui.ctx().format_shortcut(&OPEN_SHORTCUT));
        if ui.add(open).clicked() {
            return Some(Command::OpenProject);
        }
        let can_reindex = self.project.root.is_some() && self.job.is_none();
        if ui
            .add_enabled(can_reindex, egui::Button::new("Re-index"))
            .clicked()
        {
            return Some(Command::Reindex);
        }
        ui.separator();
        ui.button("Quit").clicked().then_some(Command::Quit)
    }

    /// The Go menu: opens the palette itself, returns other commands.
    pub(super) fn go_menu(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let symbol_keys = ui.ctx().format_shortcut(&OPEN_SYMBOL_SHORTCUT);
        let command_keys = ui.ctx().format_shortcut(&OPEN_COMMAND_SHORTCUT);
        let go_to = egui::Button::new("Go to Symbol…").shortcut_text(symbol_keys);
        if ui.add(go_to).clicked() {
            self.palette.open_with(String::new());
        }
        let palette = egui::Button::new("Command Palette…").shortcut_text(command_keys);
        if ui.add(palette).clicked() {
            self.palette.open_with(palette::COMMAND_PREFIX.to_string());
        }
        if ui.button("Search Notes…").clicked() {
            self.palette.open_with(palette::NOTE_PREFIX.to_string());
        }
        ui.separator();
        let entry = egui::Button::new(Command::GoToEntryPoint.label())
            .shortcut_text(ui.ctx().format_shortcut(&ENTRY_POINT_SHORTCUT));
        if ui.add(entry).clicked() {
            return Some(Command::GoToEntryPoint);
        }
        ui.menu_button("Entry Points", |ui| self.entry_points_menu(ui));
        let editor = egui::Button::new(Command::OpenInEditor.label())
            .shortcut_text(ui.ctx().format_shortcut(&EDITOR_SHORTCUT));
        if ui.add(editor).clicked() {
            return Some(Command::OpenInEditor);
        }
        ui.separator();
        let bookmark = egui::Button::new(Command::ToggleBookmark.label())
            .shortcut_text(ui.ctx().format_shortcut(&BOOKMARK_SHORTCUT));
        if ui.add(bookmark).clicked() {
            return Some(Command::ToggleBookmark);
        }
        let note = egui::Button::new(Command::EditNote.label())
            .shortcut_text(ui.ctx().format_shortcut(&NOTE_SHORTCUT));
        if ui.add(note).clicked() {
            return Some(Command::EditNote);
        }
        let panel = egui::Button::new(Command::ShowNotes.label())
            .shortcut_text(ui.ctx().format_shortcut(&NOTES_PANEL_SHORTCUT));
        if ui.add(panel).clicked() {
            return Some(Command::ShowNotes);
        }

        ui.separator();
        [
            Command::TrailView,
            Command::CallHierarchy,
            Command::Overview,
            Command::Back,
        ]
        .into_iter()
        .find(|command| ui.button(command.label()).clicked())
    }

    pub(super) fn analyze_menu(&self, ui: &mut egui::Ui) -> Option<Command> {
        let findings = egui::Button::new(Command::ShowFindings.label())
            .shortcut_text(ui.ctx().format_shortcut(&FINDINGS_SHORTCUT));
        if ui.add(findings).clicked() {
            return Some(Command::ShowFindings);
        }
        ui.separator();
        [
            Command::MarkDangerous,
            Command::TrustProject,
            Command::RunSemgrep,
            Command::RunCppcheck,
        ]
        .into_iter()
        .find(|command| ui.button(command.label()).clicked())
    }

    /// Lists the best entry points; clicking one selects it.
    pub(super) fn entry_points_menu(&mut self, ui: &mut egui::Ui) {
        let graph = &self.project.graph;
        let entries = self.graph_view.entry_points(graph);
        if entries.is_empty() {
            ui.weak("No entry points found");
            return;
        }
        let picked = entries
            .iter()
            .take(MAX_ENTRY_MENU_ITEMS)
            .filter_map(|id| graph.symbols.get(id))
            .find(|s| {
                let label = format!("{}    {}:{}", s.qualified_name, s.file, s.line);
                ui.button(label).clicked()
            })
            .map(|s| s.id);
        let more = entries.len().saturating_sub(MAX_ENTRY_MENU_ITEMS);
        if more > 0 {
            let keys = ui.ctx().format_shortcut(&ENTRY_POINT_SHORTCUT);
            ui.weak(format!("…and {more} more ({keys} steps through all)"));
        }
        if let Some(id) = picked {
            self.graph_view.select(id);
        }
    }
}

/// Opens the Project Triage window on any of its tabs.
fn triage_menu(ui: &mut egui::Ui) -> Option<Command> {
    let overview = egui::Button::new("Project Triage…")
        .shortcut_text(ui.ctx().format_shortcut(&TRIAGE_SHORTCUT));
    if ui.add(overview).clicked() {
        return Some(Command::Triage(SurveyTab::Overview));
    }
    ui.separator();
    SurveyTab::ALL
        .into_iter()
        .skip(1)
        .find(|tab| ui.button(tab.label()).clicked())
        .map(Command::Triage)
}
