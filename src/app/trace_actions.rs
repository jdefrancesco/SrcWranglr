//! What the app does with traces: running one from a finding or from a
//! variable picked in the source (back to where it comes from, forward to
//! where it goes, or to a sink picked after it), and showing its steps.

use super::session::{TraceSource, TraceStart};
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

    /// Makes `source` the variable a trace to a sink starts from (`None`:
    /// forgets the one picked).
    pub(super) fn set_trace_source(&mut self, source: Option<TraceSource>) {
        if let Some(source) = &source {
            self.status = Status::Info(format!(
                "Source: {}. Now right-click the call it should reach and pick Trace `{}` to \
                 Here.",
                source.label(),
                source.name
            ));
        }
        let name = source.as_ref().map(|s| s.name.clone());
        self.trace_panel.set_source(source);
        self.code_panel.set_trace_source(name);
    }

    /// Traces from the source picked to the call to `call` at `file:line`,
    /// into the Trace pane; the source is then forgotten.
    pub(super) fn trace_to_sink(&mut self, file: &str, line: u32, call: &str) {
        let Some(source) = self.trace_panel.source().cloned() else {
            self.status = Status::Info(
                "Pick a source first: right-click a variable, then Set as Trace Source.".into(),
            );
            return;
        };
        let is_traced = self.run_trace(TraceStart::Between {
            file: source.file,
            line: source.line,
            column: source.column,
            sink_file: file.to_string(),
            sink_line: line,
            sink_call: call.to_string(),
        });
        // Kept when nothing was traced, so it needn't be picked again.
        if is_traced {
            self.set_trace_source(None);
        }
    }

    /// Runs the trace `start` into the Trace pane. Whether there was one.
    fn run_trace(&mut self, start: TraceStart) -> bool {
        match self.traced(&start) {
            Some(found) => {
                let subject = &found.subject;
                let what = match start {
                    TraceStart::Forward { .. } => format!("where {subject} goes"),
                    TraceStart::Call { .. }
                    | TraceStart::Variable { .. }
                    | TraceStart::Between { .. } => subject.clone(),
                };
                self.status = Status::Info(format!("Traced {what} in the Trace pane."));
                self.trace_panel.open(found, start);
                self.code_panel.set_trail(Vec::new());
                true
            }
            None => {
                let missing = match start {
                    TraceStart::Call { .. } => {
                        "No call was found where this finding is, so nothing was traced."
                    }
                    TraceStart::Variable { .. }
                    | TraceStart::Forward { .. }
                    | TraceStart::Between { .. } => {
                        "That isn't a variable inside a function, so nothing was traced."
                    }
                };
                self.status = Status::Info(missing.to_string());
                false
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
                let sinks: Vec<Sink> = self
                    .findings
                    .all()
                    .iter()
                    .filter(|f| is_about_a_call(f))
                    .map(Sink::of)
                    .collect();
                trace::trace_forward(graph, &sinks, file, *line, *column)
            }
            TraceStart::Between {
                file,
                line,
                column,
                sink_file,
                sink_line,
                sink_call,
            } => {
                let sink = Sink::picked(sink_file, *sink_line, sink_call);
                trace::trace_between(graph, file, *line, *column, &sink)
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
            Some(TracePanelAction::ClearSource) => self.set_trace_source(None),
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

/// Whether `finding` is about a call a value can reach, so a trace may end
/// there: not a comment silencing a tool, nor lines a fix commit changed
/// (which mark whole lines, and would match any call on them).
fn is_about_a_call(finding: &crate::analysis::Finding) -> bool {
    ![
        crate::analysis::silenced::TOOL,
        crate::history::findings::TOOL,
    ]
    .contains(&finding.tool.as_str())
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
    fn only_findings_about_calls_end_a_trace() {
        let finding = |tool: &str| crate::analysis::Finding {
            tool: tool.into(),
            rule: "r".into(),
            severity: crate::analysis::Severity::Low,
            message: String::new(),
            fix: None,
            cwe: None,
            file: "a.c".into(),
            line: 1,
            columns: None,
            symbol: None,
        };
        assert!(super::is_about_a_call(&finding("semgrep")));
        assert!(super::is_about_a_call(&finding(
            crate::analysis::risky::TOOL
        )));
        assert!(!super::is_about_a_call(&finding(
            crate::history::findings::TOOL
        )));
        assert!(!super::is_about_a_call(&finding(
            crate::analysis::silenced::TOOL
        )));
    }

    fn argv_source() -> TraceSource {
        TraceSource {
            file: "user.c".into(),
            line: 6,
            column: "load_user(".len() as u32,
            name: "argv".into(),
        }
    }

    #[test]
    fn a_source_then_a_sink_traces_between_them_and_forgets_the_source() {
        let mut app = app();
        app.set_trace_source(Some(argv_source()));
        assert!(app.trace_panel.is_open(), "the pane shows the source");
        assert_eq!(app.trace_panel.source(), Some(&argv_source()));
        app.trace_to_sink("user.c", 3, "strcpy");
        assert!(matches!(
            app.trace_panel.start(),
            Some(TraceStart::Between { sink_line: 3, .. })
        ));
        assert_eq!(app.trace_panel.source(), None);
        let start = app.trace_panel.start().cloned().expect("a start");
        let trace = app.traced(&start).expect("a trace");
        assert_eq!(trace.direction, trace::Direction::Between);
        assert!(
            trace.paths.iter().any(|p| p.endpoint.is_sink()),
            "{trace:#?}"
        );
    }

    #[test]
    fn a_sink_without_a_source_says_what_to_do_first() {
        let mut app = app();
        app.trace_to_sink("user.c", 3, "strcpy");
        assert!(app.trace_panel.start().is_none());
        let Status::Info(text) = &app.status else {
            panic!("expected info");
        };
        assert!(text.contains("Set as Trace Source"), "{text}");
    }

    #[test]
    fn a_source_is_cleared_but_kept_when_a_trace_is_closed() {
        let mut app = app();
        app.set_trace_source(Some(argv_source()));
        app.trace_panel.close();
        assert_eq!(app.trace_panel.source(), Some(&argv_source()));
        app.set_trace_source(None);
        assert_eq!(app.trace_panel.source(), None);
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
