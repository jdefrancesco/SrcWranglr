//! What the app does with traces: running one from a finding or from a
//! variable picked in the source (back to where it comes from, or forward
//! to where it goes), and showing its steps.

use super::session::TraceStart;
use super::trace_panel::TracePanelAction;
use super::{SrcWranglrApp, Status};
use crate::analysis::trace::{self, Sink, Step, Trace};
use crate::analysis::Finding;
use crate::model::Span;

impl SrcWranglrApp {
    /// Traces where `finding`'s call gets its values, into the Trace pane.
    pub(super) fn trace_finding(&mut self, finding: &Finding) {
        let column = finding.columns.map_or(0, |(start, _)| start);
        self.run_trace(TraceStart::Call {
            file: finding.file.clone(),
            line: finding.line,
            column,
        });
    }

    /// Traces where the variable at `file:line`, byte `column`, gets its
    /// value, into the Trace pane.
    pub(super) fn trace_variable(&mut self, file: &str, line: u32, column: u32) {
        self.run_trace(TraceStart::Variable {
            file: file.to_string(),
            line,
            column,
        });
    }

    /// Traces where the value of the variable at `file:line`, byte
    /// `column`, goes: the paths to the risky calls (findings) it reaches,
    /// into the Trace pane.
    pub(super) fn trace_forward(&mut self, file: &str, line: u32, column: u32) {
        self.run_trace(TraceStart::Forward {
            file: file.to_string(),
            line,
            column,
        });
    }

    fn run_trace(&mut self, start: TraceStart) {
        match self.traced(&start) {
            Some(found) => {
                let subject = &found.subject;
                let what = match start {
                    TraceStart::Forward { .. } => format!("where {subject} goes"),
                    TraceStart::Call { .. } | TraceStart::Variable { .. } => subject.clone(),
                };
                self.status = Status::Info(format!("Traced {what} in the Trace pane."));
                self.trace_panel.open(found, start);
                self.code_panel.set_trail(Vec::new());
            }
            None => {
                let missing = match start {
                    TraceStart::Call { .. } => {
                        "No call was found where this finding is, so nothing was traced."
                    }
                    TraceStart::Variable { .. } | TraceStart::Forward { .. } => {
                        "That isn't a variable inside a function, so nothing was traced."
                    }
                };
                self.status = Status::Info(missing.to_string());
            }
        }
    }

    /// Runs the trace `start` again (when a session is restored), without
    /// a word in the status bar.
    pub(super) fn rerun_trace(&mut self, start: &TraceStart) {
        if let Some(found) = self.traced(start) {
            self.trace_panel.open(found, start.clone());
        }
    }

    fn traced(&self, start: &TraceStart) -> Option<Trace> {
        let graph = &self.project.graph;
        match start {
            TraceStart::Call { file, line, column } => {
                trace::trace_call(graph, file, *line, *column)
            }
            TraceStart::Variable { file, line, column } => {
                trace::trace_variable(graph, file, *line, *column)
            }
            TraceStart::Forward { file, line, column } => {
                let sinks: Vec<Sink> = self.findings.all().iter().map(Sink::of).collect();
                trace::trace_forward(graph, &sinks, file, *line, *column)
            }
        }
    }

    /// Shows `step` in the source, with the traced name highlighted.
    pub(super) fn go_to_step(&mut self, step: &Step) {
        self.go_to_place(step.symbol, &step.file, step.line);
        self.code_panel.highlight(step.span);
    }

    pub(super) fn show_trace_panel(&mut self, ui: &mut egui::Ui) {
        match self.trace_panel.show(ui, self.layout) {
            Some(TracePanelAction::GoTo(step)) => {
                self.go_to_step(&step);
                self.code_panel.set_trail(self.trail_shown());
            }
            Some(TracePanelAction::Close) => {
                self.trace_panel.close();
                self.code_panel.set_trail(Vec::new());
            }
            None => {}
        }
    }

    /// Where the steps of the path shown in the Trace pane are.
    fn trail_shown(&self) -> Vec<(String, Span)> {
        let steps = self.trace_panel.shown_path().map(|p| p.steps.as_slice());
        steps
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.span.map(|span| (s.file.clone(), span)))
            .collect()
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
    fn tracing_forward_shows_the_paths_to_risky_calls() {
        let mut app = app();
        app.findings = crate::app::findings::Findings::for_graph(
            &app.project.graph,
            &crate::analysis::dangerous::DangerousList::default(),
        );
        let column = "load_user(".len() as u32;
        app.trace_forward("user.c", 6, column);
        assert!(app.trace_panel.is_open());
        assert!(matches!(
            app.trace_panel.start(),
            Some(TraceStart::Forward { line: 6, .. })
        ));
        let Status::Info(text) = &app.status else {
            panic!("expected info");
        };
        assert!(text.contains("goes"), "{text}");
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
