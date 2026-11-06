//! The Trace pane: where a traced value comes from, one path per way it
//! gets there, each step clickable to show it in the source.

use egui::RichText;

use super::layout::Layout;
use super::session::TraceStart;
use crate::analysis::trace::{Step, Trace, TracePath};
use crate::style::INPUT_COLOR;

const DEFAULT_WIDTH: f32 = 380.0;
/// Paths from outside input start open; this many others do too.
const OPEN_OTHER_PATHS: usize = 1;
const LIMITS: &str = "Follows assignments, copies, calls, returns and the buffers a function \
    fills, as written. Writes through other pointers to the same memory, calls through function \
    pointers, and checks that limit the value aren't seen, so a path means it may come from there.";

/// What the user picked in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TracePanelAction {
    /// Show this step in the source.
    GoTo(Step),
    Close,
}

#[derive(Default)]
pub struct TracePanel {
    trace: Option<Trace>,
    /// What the trace was started from, to run it again.
    start: Option<TraceStart>,
    /// The step last shown: path, then step, by index.
    selected: Option<(usize, usize)>,
}

impl TracePanel {
    /// Shows `trace` (started from `start`), replacing the one shown.
    pub fn open(&mut self, trace: Trace, start: TraceStart) {
        self.trace = Some(trace);
        self.start = Some(start);
        self.selected = None;
    }

    pub fn close(&mut self) {
        self.trace = None;
        self.start = None;
        self.selected = None;
    }

    /// What the trace shown was started from.
    pub fn start(&self) -> Option<&TraceStart> {
        self.start.as_ref()
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.trace.is_some()
    }

    /// Draws the pane (when a trace is open) at the right of `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, layout: Layout) -> Option<TracePanelAction> {
        self.trace.as_ref()?;
        egui::Panel::right(layout.pane_id("trace_panel"))
            .resizable(true)
            .default_size(DEFAULT_WIDTH)
            .show(ui, |ui| self.contents(ui))
            .inner
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<TracePanelAction> {
        let trace = self.trace.as_ref()?;
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.heading("Trace");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    picked = Some(TracePanelAction::Close);
                }
            });
        });
        ui.monospace(&trace.subject);
        ui.weak(format!("{}:{}", trace.file, trace.line));
        ui.label(summary(trace)).on_hover_text(LIMITS);
        if trace.is_cut_short {
            ui.weak("Limits were reached, so there may be more paths.");
        }
        ui.separator();
        let selected = self.selected;
        let clicked = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| paths(ui, &trace.paths, selected))
            .inner;
        if let Some((path, step)) = clicked {
            self.selected = Some((path, step));
            let shown = trace.paths.get(path).and_then(|p| p.steps.get(step));
            picked = shown.cloned().map(TracePanelAction::GoTo);
        }
        picked
    }
}

/// "3 paths, 1 from outside input", or why there are none.
fn summary(trace: &Trace) -> String {
    let total = trace.paths.len();
    let from_input = trace.paths.iter().filter(|p| p.origin.is_input()).count();
    match (total, from_input) {
        (0, _) => {
            "No path found: what it uses is constant, or set where this can't follow.".to_string()
        }
        (1, 1) => "1 path, from outside input".to_string(),
        (1, _) => "1 path, not from outside input".to_string(),
        (_, 0) => format!("{total} paths, none from outside input"),
        _ => format!("{total} paths, {from_input} from outside input"),
    }
}

/// Every path, each a collapsible list of steps. The step clicked, if any.
fn paths(
    ui: &mut egui::Ui,
    paths: &[TracePath],
    selected: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let mut clicked = None;
    let mut others_open = 0;
    for (index, path) in paths.iter().enumerate() {
        let is_input = path.origin.is_input();
        let is_open = is_input || {
            others_open += 1;
            others_open <= OPEN_OTHER_PATHS
        };
        let title = format!("Path {} — {}", index + 1, path.origin.label());
        let title = if is_input {
            RichText::new(title).color(INPUT_COLOR)
        } else {
            RichText::new(title)
        };
        egui::CollapsingHeader::new(title)
            .id_salt(("trace_path", index))
            .default_open(is_open)
            .show(ui, |ui| {
                for (number, step) in path.steps.iter().enumerate() {
                    let is_selected = selected == Some((index, number));
                    if step_row(ui, number + 1, step, is_selected) {
                        clicked = Some((index, number));
                    }
                }
            });
    }
    clicked
}

/// One step: where, its code, what happens there. Whether it was clicked.
fn step_row(ui: &mut egui::Ui, number: usize, step: &Step, is_selected: bool) -> bool {
    let place = format!("{number}. {}:{}", step.file, step.line);
    let row = ui
        .selectable_label(is_selected, RichText::new(place).strong())
        .on_hover_text("Show in the source");
    ui.indent(("trace_step", number), |ui| {
        ui.label(RichText::new(&step.code).monospace());
        ui.weak(&step.note);
    });
    ui.add_space(2.0);
    row.clicked()
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::analysis::trace::Origin;

    fn step(line: u32, code: &str, note: &str) -> Step {
        Step {
            file: "user.c".into(),
            line,
            span: None,
            symbol: Some(1),
            code: code.into(),
            note: note.into(),
        }
    }

    fn trace() -> Trace {
        Trace {
            subject: "strcpy(buf, name)".into(),
            file: "user.c".into(),
            line: 8,
            paths: vec![
                TracePath {
                    origin: Origin::Input("the command line".into()),
                    steps: vec![
                        step(
                            10,
                            "int main(int argc, char **argv) {",
                            "`argv` is the command line",
                        ),
                        step(8, "strcpy(buf, name);", "the risky call to `strcpy()`"),
                    ],
                },
                TracePath {
                    origin: Origin::Stopped("set to a constant".into()),
                    steps: vec![step(3, "int n = 4;", "`n` set from `4`")],
                },
            ],
            is_cut_short: true,
        }
    }

    struct State {
        panel: TracePanel,
        action: Option<TracePanelAction>,
    }

    fn harness(trace: Option<Trace>) -> Harness<'static, State> {
        let mut panel = TracePanel::default();
        if let Some(trace) = trace {
            let start = TraceStart::Call {
                file: "user.c".into(),
                line: 8,
                column: 0,
            };
            panel.open(trace, start);
        }
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                if let Some(action) = state.panel.show(ui, Layout::default()) {
                    state.action = Some(action);
                }
            },
            State {
                panel,
                action: None,
            },
        );
        harness.run();
        harness
    }

    #[test]
    fn shows_each_path_with_where_it_starts() {
        let harness = harness(Some(trace()));
        harness.get_by_label("strcpy(buf, name)");
        harness.get_by_label("2 paths, 1 from outside input");
        harness.get_by_label_contains("Path 1 — from the command line");
        harness.get_by_label_contains("Path 2 — stops: set to a constant");
        harness.get_by_label("`argv` is the command line");
        harness.get_by_label("Limits were reached, so there may be more paths.");
    }

    #[test]
    fn clicking_a_step_shows_it_in_the_source() {
        let mut harness = harness(Some(trace()));
        harness.get_by_label("1. user.c:10").click();
        harness.run();
        let action = harness.state().action.clone();
        let Some(TracePanelAction::GoTo(step)) = action else {
            panic!("expected a step, got {action:?}");
        };
        assert_eq!(step.line, 10);
        assert_eq!(harness.state().panel.selected, Some((0, 0)));
    }

    #[test]
    fn close_closes_it() {
        let mut harness = harness(Some(trace()));
        harness.get_by_label("Close").click();
        harness.run();
        assert_eq!(harness.state().action, Some(TracePanelAction::Close));
    }

    #[test]
    fn says_so_when_nothing_was_found() {
        let empty = Trace {
            paths: Vec::new(),
            is_cut_short: false,
            ..trace()
        };
        let harness = harness(Some(empty));
        harness.get_by_label_contains("No path found");
    }

    #[test]
    fn nothing_shows_without_a_trace() {
        let harness = harness(None);
        assert!(harness.query_by_label("Trace").is_none());
    }
}
