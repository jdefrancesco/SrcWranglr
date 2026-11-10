//! What the app does with what is asked for in the source pane: exploring
//! a name, notes, bookmarks, TODOs, rules like a line, and traces.

use super::code_panel::PanelAction;
use super::layout::SourcePlacement;
use super::pane_bar::Edge;
use super::SrcWranglrApp;
use crate::analysis::similar::Example;

impl SrcWranglrApp {
    pub(super) fn show_code_panel(&mut self, ui: &mut egui::Ui) {
        if !self.layout.is_source_shown {
            return;
        }
        let layout = self.layout;
        let (layout, ()) = layout.show_source_pane(ui, |ui| {
            let selected = self.graph_view.selected();
            let edge = match layout.source {
                SourcePlacement::Beside => Edge::Right,
                SourcePlacement::Below => Edge::Bottom,
            };
            self.code_panel.is_drafting_rule = self.is_drafting_rule();
            let action = self.code_panel.show(
                ui,
                &self.project.graph,
                selected,
                &self.notebook,
                &self.findings,
                edge,
            );
            if let Some(action) = action {
                let ctx = ui.ctx().clone();
                self.run_panel_action(action, &ctx);
            }
        });
        // Kept hidden if its arrow was clicked.
        self.layout = layout.with_source_shown(self.layout.is_source_shown);
    }

    pub(super) fn run_panel_action(&mut self, action: PanelAction, ctx: &egui::Context) {
        match action {
            PanelAction::Hide => self.layout = self.layout.with_source_shown(false),
            PanelAction::Explore(id) => self.graph_view.select(id),
            PanelAction::OpenInEditor => self.open_in_editor(),
            PanelAction::ToggleBookmark(target) => self.toggle_bookmark(target),
            PanelAction::EditNote(target) => self.edit_note(target),
            PanelAction::ShowFinding(id) => self.explain_finding(id),
            PanelAction::MarkDangerous(name) => self.mark_dangerous(&name),
            PanelAction::AddTodo { file, line } => self.todo_panel.add_here(file, line),
            PanelAction::FindSimilar { file, first, last } => {
                self.find_similar(Example::lines(&file, first, last));
            }
            PanelAction::AddRulePlace { file, first, last } => {
                self.add_rule_place(Example::lines(&file, first, last));
            }
            PanelAction::UseAsRuleException { file, line } => self.add_rule_exception(&file, line),
            PanelAction::Trace { file, line, column } => self.trace_variable(&file, line, column),
            PanelAction::TraceForward { file, line, column } => {
                self.trace_forward(&file, line, column);
            }
            PanelAction::SetTraceSource(source) => self.set_trace_source(Some(source)),
            PanelAction::TraceToSink { file, line, call } => self.trace_to_sink(&file, line, &call),
            PanelAction::ShowDocs(name) => self.show_docs_for(&name, ctx),
            PanelAction::FindLookAlikes { file, line } => {
                self.find_code_like_line(&file, line, ctx);
            }
            PanelAction::LookUpDocs(name) => self.look_up_docs(&name, ctx),
        }
    }
}
