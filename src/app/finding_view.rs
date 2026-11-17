//! Explains one finding: what was flagged, why it can be a bug, and how to
//! fix it. Opened by clicking a highlighted snippet in the code view.

use egui::{Id, RichText};

use super::findings::verdict_label;
use super::visible::{visible, visible_lines};
use crate::analysis::memory::Lifetimes;
use crate::analysis::{risky, silenced, Finding};
use crate::model::SymbolId;
use crate::notes::triage::TriageStatus;
use crate::style::{severity_text_color, INPUT_COLOR};

const WIDTH: f32 = 460.0;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingViewAction {
    Close,
    Explore(SymbolId),
    OpenInEditor,
    /// Add (or change) the called function's dangerous-list entry.
    MarkDangerous(String),
    /// Record your verdict (`None` clears it) and its note.
    Triage(Option<TriageStatus>, String),
    /// Trace where the call's arguments come from.
    Trace,
}

pub struct FindingView {
    finding: Finding,
    /// Qualified name of the enclosing function, if known.
    function: Option<String>,
    /// How outside input reaches that function, if it does.
    input: Option<String>,
    /// Your verdict as being edited.
    verdict: Option<TriageStatus>,
    note: String,
    /// The pointer lifetimes in the enclosing function, for a memory-safety
    /// finding — the invariant matrix that led to it.
    lifetimes: Option<Lifetimes>,
}

impl FindingView {
    /// `input` says how outside input reaches the enclosing function;
    /// `verdict` is the one already recorded, with its note.
    pub fn new(
        finding: Finding,
        function: Option<String>,
        input: Option<String>,
        verdict: Option<(TriageStatus, String)>,
    ) -> Self {
        let (verdict, note) = verdict.map_or((None, String::new()), |(s, n)| (Some(s), n));
        Self {
            finding,
            function,
            input,
            verdict,
            note,
            lifetimes: None,
        }
    }

    /// Attaches the pointer-lifetime matrix (for a use-after-free or
    /// double-free finding).
    pub fn with_lifetimes(mut self, lifetimes: Option<Lifetimes>) -> Self {
        self.lifetimes = lifetimes;
        self
    }

    pub fn finding(&self) -> &Finding {
        &self.finding
    }

    /// Draws the explanation. Returns what the user picked, if anything.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<FindingViewAction> {
        let response = egui::Modal::new(Id::unique("finding_view")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            self.contents(ui)
        });
        let should_close = response.should_close();
        response
            .inner
            .or_else(|| should_close.then_some(FindingViewAction::Close))
    }

    /// The pointer-lifetime rows — the invariant matrix that led to a
    /// use-after-free or double-free: each freed pointer and what became of
    /// it, line by line.
    fn lifetimes_section(&self, ui: &mut egui::Ui) {
        let Some(lifetimes) = &self.lifetimes else {
            return;
        };
        ui.add_space(4.0);
        ui.strong("Pointer lifetimes");
        super::lifetimes_view::lifetime_rows(ui, lifetimes);
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<FindingViewAction> {
        let f = &self.finding;
        ui.horizontal(|ui| {
            // Rules and tools may be named by an imported report.
            ui.heading(visible(&f.rule));
            let severity = RichText::new(f.severity.label())
                .strong()
                .color(severity_text_color(f.severity, ui.visuals()));
            ui.label(severity);
            if let Some(cwe) = f.cwe {
                ui.label(format!("CWE-{cwe}"));
            }
        });
        let place = match &self.function {
            Some(function) => format!("{}:{} in {function}", f.file, f.line),
            None => format!("{}:{}", f.file, f.line),
        };
        ui.monospace(place);
        match (&self.input, &self.function) {
            (Some(path), _) => {
                ui.label(RichText::new(format!("Input may reach this: {path}")).color(INPUT_COLOR));
            }
            (None, Some(_)) => {
                ui.weak(
                    "No direct call path from input was found (calls through function \
                     pointers and callbacks aren't followed).",
                );
            }
            (None, None) => {}
        }
        ui.separator();
        ui.strong("Why it can be a bug");
        // A tool's message may quote the code; a commit's is the repository's.
        ui.label(visible_lines(&f.message));
        if let Some(fix) = &f.fix {
            ui.add_space(4.0);
            ui.strong("How to fix");
            ui.label(visible_lines(fix));
        }
        self.lifetimes_section(ui);
        ui.add_space(4.0);
        ui.weak(format!("Found by {}", visible(&f.tool)));
        ui.separator();
        let triaged = self.triage_section(ui);
        ui.separator();
        let picked = ui.horizontal(|ui| self.buttons(ui)).inner;
        triaged.or(picked)
    }

    /// Your verdict on it, with a note.
    fn triage_section(&mut self, ui: &mut egui::Ui) -> Option<FindingViewAction> {
        ui.strong("Your verdict");
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut self.verdict, None, "Untriaged");
            for status in TriageStatus::ALL {
                let label = verdict_label(&self.finding.rule, status);
                ui.radio_value(&mut self.verdict, Some(status), label);
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.note)
                .hint_text("Why (optional)")
                .desired_width(f32::INFINITY),
        );
        let save = ui.button("Save Verdict").on_hover_text(
            "Reviewed and false positives stop being highlighted; confirmed bugs are \
             also added to your notes",
        );
        save.clicked()
            .then(|| FindingViewAction::Triage(self.verdict, self.note.trim().to_string()))
    }

    /// The function whose call this is: for SrcWranglr's own findings,
    /// the rule names it (`sprintf: unbounded %s` → `sprintf`).
    fn called_function(&self) -> Option<&str> {
        let f = &self.finding;
        match f.tool.as_str() {
            // The rule is the called name itself (maybe `ns::f`).
            risky::LIST_TOOL => Some(f.rule.as_str()),
            // `sprintf: unbounded %s` → `sprintf`.
            risky::TOOL => f.rule.split(": ").next(),
            _ => None,
        }
    }

    fn buttons(&self, ui: &mut egui::Ui) -> Option<FindingViewAction> {
        let mut action = None;
        if let (Some(id), Some(function)) = (self.finding.symbol, &self.function) {
            if ui.button(format!("Explore {function}")).clicked() {
                action = Some(FindingViewAction::Explore(id));
            }
        }
        // A comment has no values to follow.
        if self.finding.tool != silenced::TOOL {
            let trace = ui.button("Trace Back").on_hover_text(
                "Follow the values this call uses back to where they come from, in the Trace pane",
            );
            if trace.clicked() {
                action = Some(FindingViewAction::Trace);
            }
        }
        if ui.button("Open in Editor").clicked() {
            action = Some(FindingViewAction::OpenInEditor);
        }
        if let Some(function) = self.called_function() {
            let mark = ui.button("Mark as Dangerous…");
            if mark
                .on_hover_text(format!("Change how calls to {function} are flagged"))
                .clicked()
            {
                action = Some(FindingViewAction::MarkDangerous(function.to_string()));
            }
        }
        if ui.button("Close").clicked() {
            action = Some(FindingViewAction::Close);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::analysis::Severity;

    struct State {
        view: FindingView,
        action: Option<FindingViewAction>,
    }

    fn finding() -> Finding {
        Finding {
            tool: "SrcWranglr".into(),
            rule: "strcpy".into(),
            severity: Severity::High,
            message: "Copies without knowing the destination's size.".into(),
            fix: Some("Use snprintf.".into()),
            cwe: Some(120),
            file: "net.c".into(),
            line: 42,
            columns: Some((4, 10)),
            symbol: Some(7),
        }
    }

    fn harness(function: Option<&str>) -> Harness<'static, State> {
        harness_with_input(function, None)
    }

    fn harness_with_input(function: Option<&str>, input: Option<&str>) -> Harness<'static, State> {
        let state = State {
            view: FindingView::new(
                finding(),
                function.map(String::from),
                input.map(String::from),
                None,
            ),
            action: None,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(action) = state.view.show(&ctx) {
                    state.action = Some(action);
                }
            },
            state,
        );
        harness.run();
        harness
    }

    #[test]
    fn says_how_input_reaches_the_call() {
        let harness = harness_with_input(
            Some("net::read_packet"),
            Some("recv() in net::serve » net::read_packet"),
        );
        assert!(harness
            .query_by_label_contains(
                "Input may reach this: recv() in net::serve » net::read_packet"
            )
            .is_some());
    }

    #[test]
    fn says_plainly_when_no_path_from_input_was_found() {
        let harness = harness(Some("net::read_packet"));
        assert!(harness
            .query_by_label_contains("No direct call path from input was found")
            .is_some());
    }

    #[test]
    fn explains_why_and_how_to_fix() {
        let harness = harness(Some("net::read_packet"));
        harness.get_by_label("strcpy");
        harness.get_by_label("CWE-120");
        harness.get_by_label("net.c:42 in net::read_packet");
        harness.get_by_label("Copies without knowing the destination's size.");
        harness.get_by_label("Use snprintf.");
        harness.get_by_label("Found by SrcWranglr");
    }

    #[test]
    fn buttons_explore_open_and_close() {
        let mut harness = harness(Some("net::read_packet"));
        harness.get_by_label("Explore net::read_packet").click();
        harness.run();
        assert_eq!(harness.state().action, Some(FindingViewAction::Explore(7)));
        harness.get_by_label("Open in Editor").click();
        harness.run();
        assert_eq!(
            harness.state().action,
            Some(FindingViewAction::OpenInEditor)
        );
        harness.get_by_label("Mark as Dangerous…").click();
        harness.run();
        assert_eq!(
            harness.state().action,
            Some(FindingViewAction::MarkDangerous("strcpy".into()))
        );
        harness.get_by_label("Close").click();
        harness.run();
        assert_eq!(harness.state().action, Some(FindingViewAction::Close));
    }

    #[test]
    fn trace_back_asks_where_the_call_gets_its_values() {
        let mut harness = harness(Some("net::read_packet"));
        harness.get_by_label("Trace Back").click();
        harness.run();
        assert_eq!(harness.state().action, Some(FindingViewAction::Trace));
    }

    #[test]
    fn records_a_verdict_with_a_note() {
        let mut harness = harness(None);
        harness.get_by_label("Confirmed bug").click();
        harness.run();
        harness.get_by_role(egui::Role::TextInput).focus();
        harness.run();
        harness
            .get_by_role(egui::Role::TextInput)
            .type_text("len from packet");
        harness.run();
        harness.get_by_label("Save Verdict").click();
        harness.run();
        assert_eq!(
            harness.state().action,
            Some(FindingViewAction::Triage(
                Some(TriageStatus::ConfirmedBug),
                "len from packet".into()
            ))
        );
    }

    #[test]
    fn a_prompt_injection_is_judged_benign_or_confirmed_and_withheld() {
        let injection = Finding {
            rule: crate::analysis::injection::RULE.into(),
            ..finding()
        };
        let view = FindingView::new(injection, None, None, None);
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(action) = state.view.show(&ctx) {
                    state.action = Some(action);
                }
            },
            State { view, action: None },
        );
        harness.run();
        harness.get_by_label("Benign");
        assert!(harness.query_by_label("Confirmed bug").is_none());
        harness
            .get_by_label("Confirmed injection (withheld from agents)")
            .click();
        harness.run();
        harness.get_by_label("Save Verdict").click();
        harness.run();
        assert_eq!(
            harness.state().action,
            Some(FindingViewAction::Triage(
                Some(TriageStatus::ConfirmedBug),
                String::new()
            ))
        );
    }

    #[test]
    fn without_a_known_function_there_is_nothing_to_explore() {
        let harness = harness(None);
        assert!(harness.query_by_label_contains("Explore").is_none());
    }

    #[test]
    fn shows_the_pointer_lifetime_matrix_for_a_memory_finding() {
        use crate::index::graph_of_files;
        let graph = graph_of_files(&[("a.c", "void f(char *p) {\n   free(p);\n   *p = 0;\n}\n")]);
        let lifetimes = crate::analysis::memory::lifetimes(&graph, "a.c", 2);
        assert!(lifetimes.is_some());
        let view =
            FindingView::new(finding(), Some("f".into()), None, None).with_lifetimes(lifetimes);
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(action) = state.view.show(&ctx) {
                    state.action = Some(action);
                }
            },
            State { view, action: None },
        );
        harness.run();
        harness.get_by_label("Pointer lifetimes");
        assert!(harness.query_by_label_contains("freed L2").is_some());
        assert!(harness.query_by_label_contains("used L3").is_some());
    }
}
