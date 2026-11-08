//! The Trace pane: where a traced value comes from or goes, one path per
//! way, each step clickable to show it in the source.

use egui::RichText;

use super::layout::Layout;
use super::pane_bar::{self, Edge};
use super::session::TraceStart;
use crate::analysis::trace::{Direction, Endpoint, Step, Trace, TracePath};
use crate::analysis::Severity;
use crate::style::{severity_text_color, INPUT_COLOR};

const DEFAULT_WIDTH: f32 = 380.0;
/// Paths to outside input or a risky call start open; this many others do
/// too.
const OPEN_OTHER_PATHS: usize = 1;
const NOTHING_TRACED: &str = "Nothing traced yet. Right-click a variable in the source: \
    Trace Where It Comes From, or Trace Where It Goes.";
const LIMITS: &str = "Follows assignments, copies, calls, returns and the buffers a function \
    fills, as written. Writes through other pointers to the same memory, calls through function \
    pointers, and checks that limit the value aren't seen, so a path means it may go that way.";

/// What the user picked in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TracePanelAction {
    /// Show this step in the source.
    GoTo(Step),
    Close,
}

#[derive(Default)]
pub struct TracePanel {
    /// Whether the pane is shown; hiding it keeps the trace.
    is_open: bool,
    trace: Option<Trace>,
    /// What the trace was started from, to run it again.
    start: Option<TraceStart>,
    /// The step last shown: path, then step, by index.
    selected: Option<(usize, usize)>,
}

impl TracePanel {
    /// Shows `trace` (started from `start`), replacing the one shown.
    pub fn open(&mut self, trace: Trace, start: TraceStart) {
        self.is_open = true;
        self.trace = Some(trace);
        self.start = Some(start);
        self.selected = None;
    }

    /// Drops the trace and hides the pane.
    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// What the trace shown was started from.
    pub fn start(&self) -> Option<&TraceStart> {
        self.start.as_ref()
    }

    /// The path of the step last shown.
    pub fn shown_path(&self) -> Option<&TracePath> {
        let (path, _) = self.selected?;
        self.trace.as_ref()?.paths.get(path)
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Draws the pane (while open, sliding in and out) at the right of
    /// `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, layout: Layout) -> Option<TracePanelAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("trace_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| self.contents(ui));
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<TracePanelAction> {
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.heading("Trace");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if pane_bar::hide_button(ui, Edge::Right) {
                    self.is_open = false;
                }
                let can_close = self.trace.is_some();
                let close = ui.add_enabled(can_close, egui::Button::new("Close"));
                if close.on_hover_text("Drop this trace").clicked() {
                    picked = Some(TracePanelAction::Close);
                }
            });
        });
        let Some(trace) = self.trace.as_ref() else {
            ui.weak(NOTHING_TRACED);
            return picked;
        };
        let direction = match trace.direction {
            Direction::Back => "Traced back: where it comes from",
            Direction::Forward => "Traced forward: where it goes",
        };
        ui.label(direction);
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

/// "3 paths, 1 from outside input", "1 path to a risky call", or why
/// there are none.
fn summary(trace: &Trace) -> String {
    match trace.direction {
        Direction::Back => back_summary(trace),
        Direction::Forward => forward_summary(trace),
    }
}

fn back_summary(trace: &Trace) -> String {
    let total = trace.paths.len();
    let from_input = trace.paths.iter().filter(|p| p.endpoint.is_input()).count();
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

fn forward_summary(trace: &Trace) -> String {
    let to_sinks = trace.paths.iter().filter(|p| p.endpoint.is_sink()).count();
    let stops = trace.paths.len() - to_sinks;
    let found = match to_sinks {
        0 => "No path to a risky call found".to_string(),
        1 => "1 path to a risky call".to_string(),
        n => format!("{n} paths to risky calls"),
    };
    match stops {
        0 if to_sinks == 0 => format!("{found}: it isn't used in one this can follow."),
        0 => found,
        1 => format!("{found}, 1 that stops"),
        n => format!("{found}, {n} that stop"),
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
        let color = match path.endpoint {
            Endpoint::Input(_) => Some(INPUT_COLOR),
            Endpoint::Sink(_) => Some(severity_text_color(Severity::High, ui.visuals())),
            Endpoint::Stopped(_) => None,
        };
        let is_open = color.is_some() || {
            others_open += 1;
            others_open <= OPEN_OTHER_PATHS
        };
        let title = format!("Path {} — {}", index + 1, path.endpoint.label());
        let title = match color {
            Some(color) => RichText::new(title).color(color),
            None => RichText::new(title),
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
    use crate::analysis::trace::{Direction, Endpoint};

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
            direction: Direction::Back,
            paths: vec![
                TracePath {
                    endpoint: Endpoint::Input("the command line".into()),
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
                    endpoint: Endpoint::Stopped("set to a constant".into()),
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
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
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
        harness.get_by_label("Traced back: where it comes from");
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

    fn forward() -> Trace {
        Trace {
            subject: "`argv`".into(),
            line: 10,
            direction: Direction::Forward,
            paths: vec![
                TracePath {
                    endpoint: Endpoint::Sink("`strcpy()` (high)".into()),
                    steps: vec![
                        step(10, "load_user(argv[1]);", "`argv` here"),
                        step(
                            3,
                            "strcpy(buf, raw);",
                            "`raw` reaches `strcpy()`, argument 2",
                        ),
                    ],
                },
                TracePath {
                    endpoint: Endpoint::Stopped("stored in `g`, a global".into()),
                    steps: vec![step(4, "g = raw;", "`raw` is stored in `g`")],
                },
            ],
            is_cut_short: false,
            ..trace()
        }
    }

    #[test]
    fn a_forward_trace_says_where_the_value_goes() {
        let harness = harness(Some(forward()));
        harness.get_by_label("Traced forward: where it goes");
        harness.get_by_label("1 path to a risky call, 1 that stops");
        harness.get_by_label_contains("Path 1 — reaches `strcpy()` (high)");
        harness.get_by_label("`raw` reaches `strcpy()`, argument 2");
    }

    #[test]
    fn a_forward_trace_that_reaches_nothing_says_so() {
        let empty = Trace {
            paths: Vec::new(),
            ..forward()
        };
        let harness = harness(Some(empty));
        harness.get_by_label_contains("No path to a risky call found");
    }

    #[test]
    fn the_path_of_the_step_clicked_is_the_one_shown() {
        let mut harness = harness(Some(forward()));
        assert!(harness.state().panel.shown_path().is_none());
        harness.get_by_label("2. user.c:3").click();
        harness.run();
        let path = harness.state().panel.shown_path().cloned();
        assert_eq!(path.map(|p| p.steps.len()), Some(2));
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
