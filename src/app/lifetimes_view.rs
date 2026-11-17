//! The pointer-lifetime matrix for one function — each pointer the function
//! frees and what became of it, line by line. Shown both in a memory
//! finding's explanation and, for any function, in its own window
//! (right-click a function → Pointer Lifetimes).

use egui::RichText;

use super::visible::visible;
use crate::analysis::memory::{Lifetimes, PointKind};

/// A freed pointer, in the lifetime row.
const FREED_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x9A, 0x4C);
/// A use-after-free or double-free, in the lifetime row.
const VIOLATION_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// Draws one row per freed pointer: its variable, then its lifeline as a
/// line-ordered sequence (`freed L12 -> used L15`), violations in red.
pub fn lifetime_rows(ui: &mut egui::Ui, lifetimes: &Lifetimes) {
    ui.weak(format!(
        "in lines {}\u{2013}{}",
        lifetimes.first_line, lifetimes.last_line
    ));
    for row in &lifetimes.rows {
        ui.horizontal_wrapped(|ui| {
            ui.monospace(RichText::new(format!("{}:", visible(&row.variable))).strong());
            for (index, point) in row.points.iter().enumerate() {
                if index > 0 {
                    ui.weak("->");
                }
                let (label, color) = chip(point.kind);
                ui.label(
                    RichText::new(format!("{label} L{}", point.line))
                        .monospace()
                        .color(color),
                );
            }
        });
    }
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

/// A window showing one function's pointer lifetimes.
pub struct LifetimesView {
    /// The function's qualified name, for the title.
    function: String,
    lifetimes: Lifetimes,
    open: bool,
}

impl LifetimesView {
    pub fn new(function: String, lifetimes: Lifetimes) -> Self {
        Self {
            function,
            lifetimes,
            open: true,
        }
    }

    /// Draws the window. Returns `false` once the user closes it.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = self.open;
        egui::Window::new(format!("Pointer lifetimes — {}", visible(&self.function)))
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                if self.lifetimes.rows.is_empty() {
                    ui.weak("This function frees no pointers.");
                } else {
                    lifetime_rows(ui, &self.lifetimes);
                }
            });
        self.open = open;
        self.open
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    use super::*;
    use crate::index::graph_of_files;

    #[test]
    fn the_window_shows_each_freed_pointers_lifeline() {
        let graph = graph_of_files(&[("a.c", "void f(char *p) {\n   free(p);\n   *p = 0;\n}\n")]);
        let lifetimes = crate::analysis::memory::lifetimes(&graph, "a.c", 2).expect("lifetimes");
        let view = LifetimesView::new("f".into(), lifetimes);
        let mut harness = Harness::new_ui_state(
            |ui, view: &mut LifetimesView| {
                let ctx = ui.ctx().clone();
                view.show(&ctx);
            },
            view,
        );
        harness.run();
        assert!(harness.query_by_label_contains("freed L2").is_some());
        assert!(harness.query_by_label_contains("used L3").is_some());
    }
}
