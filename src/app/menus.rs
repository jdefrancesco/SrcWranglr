//! The menu bar: File, Go, View, Analyze, Triage, and Help.

use egui::RichText;

use super::layout::SourcePlacement;
use super::palette::{self, Command, OPEN_COMMAND_SHORTCUT, OPEN_SYMBOL_SHORTCUT};
use super::pane_bar::Pane;
use super::survey_view::SurveyTab;
use super::{
    SrcWranglrApp, BOOKMARK_SHORTCUT, EDITOR_SHORTCUT, ENTRY_POINT_SHORTCUT, FINDINGS_SHORTCUT,
    LAYOUT_SHORTCUT, MAX_ENTRY_MENU_ITEMS, NOTES_PANEL_SHORTCUT, NOTE_SHORTCUT, OPEN_SHORTCUT,
    SETTINGS_SHORTCUT, TRIAGE_SHORTCUT,
};

impl SrcWranglrApp {
    pub(super) fn show_menu_bar(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        egui::Panel::top("menu_bar")
            .show(ui, |ui| {
                egui::MenuBar::new()
                    .ui(ui, |ui| {
                        let file = ui.menu_button("File", |ui| self.file_menu(ui));
                        let go = ui.menu_button("Go", |ui| self.go_menu(ui));
                        let view = ui.menu_button("View", |ui| self.view_menu(ui));
                        let analyze = ui.menu_button("Analyze", |ui| self.analyze_menu(ui));
                        let triage = ui.menu_button("Triage", triage_menu);
                        let help = ui.menu_button("Help", help_menu);
                        let action = file
                            .inner
                            .flatten()
                            .or(go.inner.flatten())
                            .or(view.inner.flatten())
                            .or(analyze.inner.flatten())
                            .or(triage.inner.flatten())
                            .or(help.inner.flatten());
                        ui.separator();
                        ui.label(RichText::new(&self.project.name).strong());
                        let toggled = self.show_pane_toggles(ui);
                        action.or(toggled)
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
        let recent = ui
            .add_enabled_ui(!self.recent.paths().is_empty(), |ui| {
                ui.menu_button("Open Recent", |ui| self.recent_menu(ui))
                    .inner
                    .flatten()
            })
            .inner;
        if recent.is_some() {
            return recent;
        }
        if ui.button("Open Demo Project").clicked() {
            return Some(Command::OpenDemo);
        }
        ui.separator();
        if ui.button(Command::OpenSession.label()).clicked() {
            return Some(Command::OpenSession);
        }
        let can_save_session = self.project.root.is_some() && self.is_project_open;
        let save_session = egui::Button::new(Command::SaveSession.label());
        if ui.add_enabled(can_save_session, save_session).clicked() {
            return Some(Command::SaveSession);
        }
        ui.separator();
        let can_reindex = self.project.root.is_some() && self.job.is_none();
        if ui
            .add_enabled(can_reindex, egui::Button::new("Re-index"))
            .clicked()
        {
            return Some(Command::Reindex);
        }
        ui.separator();
        let settings = egui::Button::new(Command::Settings.label())
            .shortcut_text(ui.ctx().format_shortcut(&SETTINGS_SHORTCUT));
        if ui.add(settings).clicked() {
            return Some(Command::Settings);
        }
        ui.separator();
        ui.button("Quit").clicked().then_some(Command::Quit)
    }

    /// Recent projects, most recent first.
    fn recent_menu(&self, ui: &mut egui::Ui) -> Option<Command> {
        self.recent
            .paths()
            .iter()
            .position(|path| ui.button(path.display().to_string()).clicked())
            .map(Command::OpenRecent)
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
        let notes = Command::TogglePane(Pane::Notes);
        let panel = egui::Button::new(notes.label())
            .shortcut_text(ui.ctx().format_shortcut(&NOTES_PANEL_SHORTCUT));
        if ui.add(panel).clicked() {
            return Some(notes);
        }
        let todos = Command::TogglePane(Pane::Todos);
        if ui.button(todos.label()).clicked() {
            return Some(todos);
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

    /// The View menu: where the source pane sits, and resetting pane sizes.
    pub(super) fn view_menu(&self, ui: &mut egui::Ui) -> Option<Command> {
        let keys = ui.ctx().format_shortcut(&LAYOUT_SHORTCUT);
        let placements = [
            (SourcePlacement::Below, "Source Below the Graph"),
            (SourcePlacement::Beside, "Source Beside the Graph"),
        ];
        let is_other_placement_picked = placements.into_iter().any(|(placement, label)| {
            let is_current = self.layout.source == placement;
            let keys = if is_current { "" } else { keys.as_str() };
            let button = egui::Button::new(label)
                .selected(is_current)
                .shortcut_text(keys);
            ui.add(button).clicked() && !is_current
        });
        if is_other_placement_picked {
            return Some(Command::SwapSourcePane);
        }
        ui.separator();
        ui.button("Reset Layout")
            .clicked()
            .then_some(Command::ResetLayout)
    }

    pub(super) fn analyze_menu(&self, ui: &mut egui::Ui) -> Option<Command> {
        let command = Command::TogglePane(Pane::Findings);
        let findings = egui::Button::new(command.label())
            .shortcut_text(ui.ctx().format_shortcut(&FINDINGS_SHORTCUT));
        if ui.add(findings).clicked() {
            return Some(command);
        }
        let strings = Command::TogglePane(Pane::Strings);
        if ui.button(strings.label()).clicked() {
            return Some(strings);
        }
        ui.separator();
        [
            Command::MarkDangerous,
            Command::TrustProject,
            Command::RunSemgrep,
            Command::RunCppcheck,
            Command::RunFlawfinder,
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

fn help_menu(ui: &mut egui::Ui) -> Option<Command> {
    ui.button(Command::About.label())
        .clicked()
        .then_some(Command::About)
}
