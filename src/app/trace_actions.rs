//! What the app does with traces: running one from a finding or from a
//! variable picked in the source, and showing its steps.

use super::trace_panel::TracePanelAction;
use super::{SrcWranglrApp, Status};
use crate::analysis::trace::{self, Step, Trace};
use crate::analysis::Finding;

impl SrcWranglrApp {
    /// Traces where `finding`'s call gets its values, into the Trace pane.
    pub(super) fn trace_finding(&mut self, finding: &Finding) {
        let found = trace::trace_finding(&self.project.graph, finding);
        self.show_trace(
            found,
            "No call was found where this finding is, so nothing was traced.",
        );
    }

    /// Traces where the variable at `file:line`, byte `column`, gets its
    /// value, into the Trace pane.
    pub(super) fn trace_variable(&mut self, file: &str, line: u32, column: u32) {
        let found = trace::trace_variable(&self.project.graph, file, line, column);
        self.show_trace(
            found,
            "That isn't a variable inside a function, so nothing was traced.",
        );
    }

    fn show_trace(&mut self, found: Option<Trace>, missing: &str) {
        match found {
            Some(trace) => {
                self.status = Status::Info(format!("Traced {} in the Trace pane.", trace.subject));
                self.trace_panel.open(trace);
            }
            None => self.status = Status::Info(missing.to_string()),
        }
    }

    /// Shows `step` in the source, with the traced name highlighted.
    pub(super) fn go_to_step(&mut self, step: &Step) {
        self.go_to_place(step.symbol, &step.file, step.line);
        self.code_panel.highlight(step.span);
    }

    pub(super) fn show_trace_panel(&mut self, ui: &mut egui::Ui) {
        match self.trace_panel.show(ui, self.layout) {
            Some(TracePanelAction::GoTo(step)) => self.go_to_step(&step),
            Some(TracePanelAction::Close) => self.trace_panel.close(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::launch::LaunchOptions;
    use crate::app::project::Project;
    use crate::index::graph_of_files;

    const USER: &str = "void load_user(const char *raw) {\n\
        char buf[16];\n\
        strcpy(buf, raw);\n\
    }\n\
    int main(int argc, char **argv) {\n\
        load_user(argv[1]);\n\
    }\n";

    fn app() -> SrcWranglrApp {
        let ctx = egui::Context::default();
        let mut app = SrcWranglrApp::new(&ctx, None, LaunchOptions::default());
        app.project = Project::new("t", None, graph_of_files(&[("user.c", USER)]));
        app.is_project_open = true;
        app
    }

    #[test]
    fn tracing_a_variable_opens_the_pane_and_a_step_goes_to_its_line() {
        let mut app = app();
        let column = "strcpy(buf, ".len() as u32;
        app.trace_variable("user.c", 3, column);
        assert!(app.trace_panel.is_open());
        let trace = trace::trace_variable(&app.project.graph, "user.c", 3, column).expect("trace");
        let first = &trace.paths[0].steps[0];
        app.go_to_step(first);
        assert_eq!(app.graph_view.selected(), first.symbol);
        assert_eq!(
            app.project.graph.symbols[&first.symbol.expect("in main")].name,
            "main"
        );
    }

    #[test]
    fn says_so_when_there_is_nothing_to_trace() {
        let mut app = app();
        app.trace_variable("user.c", 2, 0);
        assert!(!app.trace_panel.is_open());
        let Status::Info(text) = &app.status else {
            panic!("expected info");
        };
        assert!(text.contains("nothing was traced"), "{text}");
    }
}
