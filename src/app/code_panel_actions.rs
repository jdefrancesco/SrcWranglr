//! What the app does with what is asked for in the source pane: exploring
//! a name, notes, bookmarks, TODOs, rules like a line, and traces.

use super::code_panel::PanelAction;
use super::layout::SourcePlacement;
use super::pane_bar::Edge;
use super::{SrcWranglrApp, Status};
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
            self.code_panel.range = self.history_panel.shared_range();
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
            PanelAction::ToggleComment { file, first, last } => {
                self.toggle_comment(&file, first, last);
            }
            PanelAction::AddTodo { file, line } => self.todo_panel.add_here(file, line),
            PanelAction::FindSimilar { file, first, last } => {
                self.find_similar(Example::lines(&file, first, last));
            }
            PanelAction::AddRulePlace { file, first, last } => {
                self.add_rule_place(Example::lines(&file, first, last));
            }
            PanelAction::UseAsRuleException { file, line } => self.add_rule_exception(&file, line),
            PanelAction::CrossReferences(target) => self.show_cross_references(&target),
            PanelAction::OpenFile { from, name } => self.open_named_file(&from, &name),
            PanelAction::CrossReferencesNamed { file, line, name } => {
                let target =
                    super::xrefs::referenced_by_name(&self.project.graph, &file, line, &name);
                self.show_cross_references(&target);
            }
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

    /// Toggles a `//` comment on lines `first..=last` of `file`, writing the
    /// change to the file on disk and updating the source shown. Analysis
    /// (findings, cross references) refreshes on the next re-index.
    fn toggle_comment(&mut self, file: &str, first: u32, last: u32) {
        let Some(root) = self.project.root.clone() else {
            self.status = Status::Info(
                "The demo's sources aren't on disk: open a project (⌘O) to edit.".into(),
            );
            return;
        };
        let Some(text) = self.project.graph.file_contents.get(file) else {
            self.status = Status::Error(format!("{file} isn't in this index."));
            return;
        };
        let updated = super::line_comment::toggle(text, first, last);
        if updated == *text {
            return;
        }
        let path = root.join(file);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            self.status = Status::Error(format!("Couldn't work out the path for {file}."));
            return;
        };
        let parent = path.parent().unwrap_or(root.as_path());
        match crate::safe_fs::write_atomically(parent, name, &updated) {
            Ok(()) => {
                // Copy-on-write: free unless an agent run shares the graph.
                std::sync::Arc::make_mut(&mut self.project.graph)
                    .file_contents
                    .insert(file.to_string(), updated);
                let what = if first == last {
                    format!("line {first}")
                } else {
                    format!("lines {first}\u{2013}{last}")
                };
                self.status = Status::Info(format!("Toggled // comment on {what} of {file}."));
            }
            Err(error) => self.status = Status::Error(format!("Couldn't write {file}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::launch::LaunchOptions;
    use super::super::SrcWranglrApp;
    use super::PanelAction;

    /// An app with `a.c` of `source` indexed from a temp folder.
    fn indexed(source: &str) -> (SrcWranglrApp, egui::Context, tempfile::TempDir) {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.c"), source).expect("write");
        app.start_indexing(dir.path().to_path_buf(), &ctx);
        for _ in 0..2000 {
            app.poll_job(&ctx);
            if app.job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        (app, ctx, dir)
    }

    fn toggle(app: &mut SrcWranglrApp, ctx: &egui::Context, first: u32, last: u32) {
        app.run_panel_action(
            PanelAction::ToggleComment {
                file: "a.c".into(),
                first,
                last,
            },
            ctx,
        );
    }

    #[test]
    fn toggling_comments_the_file_on_disk_and_in_the_shown_source() {
        let (mut app, ctx, dir) = indexed("int main(void) {\n    return 0;\n}\n");
        toggle(&mut app, &ctx, 2, 2);
        let commented = "int main(void) {\n    // return 0;\n}\n";
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.c")).expect("read"),
            commented
        );
        assert_eq!(app.project.graph.file_contents["a.c"], commented);
    }

    #[test]
    fn toggling_again_uncomments_it() {
        let (mut app, ctx, dir) = indexed("int main(void) {\n    return 0;\n}\n");
        toggle(&mut app, &ctx, 2, 2);
        toggle(&mut app, &ctx, 2, 2);
        let original = "int main(void) {\n    return 0;\n}\n";
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.c")).expect("read"),
            original
        );
        assert_eq!(app.project.graph.file_contents["a.c"], original);
    }

    #[test]
    fn toggling_on_the_demo_is_refused_and_writes_nothing() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.open_demo(&ctx);
        // No project root on disk: must not panic, and leaves the demo's text.
        let before = app.project.graph.file_contents.clone();
        toggle(&mut app, &ctx, 1, 1);
        assert_eq!(app.project.graph.file_contents, before);
    }
}
