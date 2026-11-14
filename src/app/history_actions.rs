//! What the app does with the git history: reading it when a project
//! opens, marking the lines fix commits last changed as findings once it
//! is read, and the History pane.

use super::history::read_history;
use super::history_panel::HistoryPanelAction;
use super::{SrcWranglrApp, Status};
use crate::history::findings::{self, TOOL};
use crate::index::functions_holding_lines;

impl SrcWranglrApp {
    /// Starts reading the open project's history in the background.
    pub(super) fn start_history(&mut self, ctx: &egui::Context) {
        let Some(root) = self.project.root.clone() else {
            self.history_panel
                .set_unavailable("The demo project has no git history.");
            return;
        };
        let files = self.project.graph.file_contents.keys().cloned().collect();
        let job = read_history(root.clone(), files, ctx.clone());
        self.history_panel.start(job, root);
    }

    /// Once the history is read, marks what fix commits last changed.
    pub(super) fn poll_history(&mut self) {
        self.history_panel.poll_changes(&self.project.graph);
        let Some(view) = self.history_panel.poll(&self.project.graph) else {
            return;
        };
        let mut found = findings::findings(&view.history);
        let fixes = view.history.fix_commits.len();
        let places: Vec<(&str, u32)> = found.iter().map(|f| (f.file.as_str(), f.line)).collect();
        let functions = functions_holding_lines(&self.project.graph, &places);
        for (finding, function) in found.iter_mut().zip(functions) {
            finding.symbol = function;
        }
        let marked = found.len();
        self.findings = self.findings.with_tool_results(TOOL, found);
        self.refresh_verdicts();
        if marked > 0 {
            self.status = Status::Info(format!(
                "Git history: {fixes} fix commits; {marked} places they last changed are in \
                 Findings and the History pane."
            ));
        }
    }

    pub(super) fn show_history_panel(&mut self, ui: &mut egui::Ui) {
        let selected = self.graph_view.selected();
        let picked = self.history_panel.show(
            ui,
            &self.project.graph,
            self.layout,
            selected,
            self.findings.all(),
        );
        match picked {
            Some(HistoryPanelAction::GoTo { symbol, file, line }) => {
                self.go_to_place(symbol, &file, line);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::app::SrcWranglrApp;
    use crate::history::test_repo::sample;
    use crate::index::graph_of_files;

    #[test]
    fn a_read_history_marks_what_fix_commits_last_changed() {
        let Some(repo) = sample() else {
            return;
        };
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        let a = std::fs::read_to_string(repo.path().join("a.c")).expect("a.c");
        let b = std::fs::read_to_string(repo.path().join("b.c")).expect("b.c");
        let graph = graph_of_files(&[("a.c", &a), ("b.c", &b)]);
        app.project = Project::new("p", Some(repo.path().to_path_buf()), graph);
        app.start_history(&ctx);
        for _ in 0..400 {
            app.poll_history();
            if app.findings.all().iter().any(|f| f.tool == super::TOOL) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let marked: Vec<(&str, u32, &str)> = app
            .findings
            .all()
            .iter()
            .filter(|f| f.tool == super::TOOL)
            .map(|f| (f.file.as_str(), f.line, f.rule.as_str()))
            .collect();
        assert_eq!(marked.len(), 2, "{marked:?}");
        assert!(marked
            .iter()
            .any(|(file, line, _)| (*file, *line) == ("a.c", 3)));
        let overflow = app
            .findings
            .all()
            .iter()
            .find(|f| f.file == "a.c" && f.tool == super::TOOL)
            .expect("a.c marked");
        assert!(overflow.symbol.is_some(), "line 3 is in parse");
    }

    #[test]
    fn the_demo_has_no_history_to_read() {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.start_history(&ctx);
        app.poll_history();
        assert!(app.findings.all().iter().all(|f| f.tool != super::TOOL));
    }
}
