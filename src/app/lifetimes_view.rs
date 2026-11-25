//! The pointer-lifetime matrix for one function — each pointer the function
//! frees and what became of it, line by line. Shown both in a memory
//! finding's explanation and, for any function, in its own window
//! (right-click a function → Pointer Lifetimes). Each point is clickable and
//! jumps to that line in the source.

use egui::RichText;

use super::visible::visible;
use crate::analysis::memory::{Lifetimes, PointKind};
use crate::model::SymbolId;

/// A freed pointer, in the lifetime row.
const FREED_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x9A, 0x4C);
/// A use-after-free or double-free, in the lifetime row.
const VIOLATION_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// Draws one row per freed pointer: its variable, then its lifeline as a
/// line-ordered sequence (`freed L12 -> used L15`), violations in red. Each
/// point is clickable; returns the line of the point the user clicked, if any.
#[must_use]
pub fn lifetime_rows(ui: &mut egui::Ui, lifetimes: &Lifetimes) -> Option<u32> {
    ui.weak(format!(
        "in lines {}\u{2013}{}",
        lifetimes.first_line, lifetimes.last_line
    ));
    let mut clicked = None;
    for row in &lifetimes.rows {
        ui.horizontal_wrapped(|ui| {
            ui.monospace(RichText::new(format!("{}:", visible(&row.variable))).strong());
            for (index, point) in row.points.iter().enumerate() {
                if index > 0 {
                    ui.weak("->");
                }
                if point_chip(ui, point.line, point.kind) {
                    clicked = Some(point.line);
                }
            }
        });
    }
    clicked
}

/// Draws one clickable lifetime point (`freed L12`). Returns true when clicked.
fn point_chip(ui: &mut egui::Ui, line: u32, kind: PointKind) -> bool {
    let (label, color) = chip(kind);
    let text = RichText::new(format!("{label} L{line}"))
        .monospace()
        .color(color);
    let response = ui
        .add(egui::Label::new(text).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Go to line {line}"));
    response.clicked()
}

/// The label and color for one point in a pointer's lifetime.
fn chip(kind: PointKind) -> (&'static str, egui::Color32) {
    match kind {
        PointKind::Freed => ("freed", FREED_COLOR),
        PointKind::FreedAgain => ("freed again", VIOLATION_COLOR),
        PointKind::Used => ("used", VIOLATION_COLOR),
        PointKind::Reassigned => ("reassigned", egui::Color32::GRAY),
    }
}

/// What showing the window produced this frame.
pub enum LifetimesOutcome {
    /// Still open, nothing picked.
    Open,
    /// The user closed the window.
    Closed,
    /// The user clicked a point; reveal this line of the function.
    Reveal {
        symbol: SymbolId,
        file: String,
        line: u32,
    },
}

/// A window showing one function's pointer lifetimes.
pub struct LifetimesView {
    /// The function's qualified name, for the title.
    function: String,
    /// The function symbol, so a clicked point can reveal it.
    symbol: SymbolId,
    /// The file the function is in.
    file: String,
    lifetimes: Lifetimes,
    open: bool,
}

impl LifetimesView {
    pub fn new(function: String, symbol: SymbolId, file: String, lifetimes: Lifetimes) -> Self {
        Self {
            function,
            symbol,
            file,
            lifetimes,
            open: true,
        }
    }

    /// Draws the window and reports what the user did this frame.
    pub fn show(&mut self, ctx: &egui::Context) -> LifetimesOutcome {
        let mut open = self.open;
        let mut clicked = None;
        egui::Window::new(format!("Pointer lifetimes — {}", visible(&self.function)))
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                if self.lifetimes.rows.is_empty() {
                    ui.weak("This function frees no pointers.");
                } else {
                    ui.weak("Click a point to jump to that line.");
                    clicked = lifetime_rows(ui, &self.lifetimes);
                }
            });
        self.open = open;
        match (self.open, clicked) {
            (false, _) => LifetimesOutcome::Closed,
            (true, Some(line)) => LifetimesOutcome::Reveal {
                symbol: self.symbol,
                file: self.file.clone(),
                line,
            },
            (true, None) => LifetimesOutcome::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::index::graph_of_files;

    fn view() -> LifetimesView {
        let graph = graph_of_files(&[("a.c", "void f(char *p) {\n   free(p);\n   *p = 0;\n}\n")]);
        let lifetimes = crate::analysis::memory::lifetimes(&graph, "a.c", 2).expect("lifetimes");
        LifetimesView::new("f".into(), 7, "a.c".into(), lifetimes)
    }

    #[test]
    fn the_window_shows_each_freed_pointers_lifeline() {
        let mut harness = Harness::new_ui_state(
            |ui, view: &mut LifetimesView| {
                let ctx = ui.ctx().clone();
                let _ = view.show(&ctx);
            },
            view(),
        );
        harness.run();
        assert!(harness.query_by_label_contains("freed L2").is_some());
        assert!(harness.query_by_label_contains("used L3").is_some());
    }

    #[test]
    fn clicking_a_point_reveals_its_line() {
        struct State {
            view: LifetimesView,
            revealed: Option<(SymbolId, String, u32)>,
        }
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let LifetimesOutcome::Reveal { symbol, file, line } = state.view.show(&ctx) {
                    state.revealed = Some((symbol, file, line));
                }
            },
            State {
                view: view(),
                revealed: None,
            },
        );
        harness.run();
        harness.get_by_label_contains("used L3").click();
        harness.run();
        assert_eq!(harness.state().revealed, Some((7, "a.c".into(), 3)));
    }
}
