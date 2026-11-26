//! The step-through emulator window: set the function's unknown input sizes,
//! then Step/Run through its statements watching each allocation and copy, with
//! any write that runs past its buffer flagged. The engine is in
//! `crate::analysis::emulate`; this is just its view.

use egui::{Color32, RichText};

use crate::analysis::emulate::{Step, Trace};

const OVERFLOW_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);
const CURRENT_COLOR: Color32 = Color32::from_rgb(0x5C, 0x9C, 0xE0);
const DEFAULT_SIZE: [f32; 2] = [560.0, 460.0];

/// What the window asks the app to do.
pub enum EmulateOutcome {
    /// Recompute the trace with these input values (name → value).
    Rerun(Vec<(String, i64)>),
    /// Show this source line (the current step moved).
    Reveal(u32),
    /// The window was closed.
    Closed,
}

pub struct EmulateView {
    /// The file and line the function was found from (for re-running).
    pub file: String,
    pub line: u32,
    trace: Trace,
    /// The inputs as edited (name, value); `i64` for the number fields.
    inputs: Vec<(String, i64)>,
    /// How many steps are shown so far (the step cursor).
    cursor: usize,
    is_open: bool,
}

impl EmulateView {
    pub fn new(file: String, line: u32, trace: Trace) -> Self {
        let inputs = trace
            .inputs
            .iter()
            .map(|i| (i.name.clone(), i.value as i64))
            .collect();
        Self {
            file,
            line,
            trace,
            inputs,
            cursor: 0,
            is_open: true,
        }
    }

    /// Replaces the trace after a re-run, keeping the inputs as edited and the
    /// cursor in range.
    pub fn set_trace(&mut self, trace: Trace) {
        // Keep any new inputs the re-run discovered, preserving edited values.
        for input in &trace.inputs {
            if !self.inputs.iter().any(|(n, _)| n == &input.name) {
                self.inputs.push((input.name.clone(), input.value as i64));
            }
        }
        self.cursor = self.cursor.min(trace.steps.len());
        self.trace = trace;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<EmulateOutcome> {
        let mut is_open = true;
        let title = format!("Emulate: {}", self.trace.function);
        let outcome = egui::Window::new(title)
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui))
            .and_then(|r| r.inner)
            .flatten();
        self.is_open = is_open && self.is_open;
        if !self.is_open {
            return Some(EmulateOutcome::Closed);
        }
        outcome
    }

    fn contents(&mut self, ui: &mut egui::Ui) -> Option<EmulateOutcome> {
        ui.label(RichText::new(
            "Set the unknown input sizes, then step through. A write past its buffer is flagged.",
        ).weak());
        let rerun = self.inputs_row(ui);
        ui.separator();
        let moved = self.controls(ui);
        ui.separator();
        self.step_log(ui);
        // A re-run takes priority; otherwise report a moved cursor so the
        // source follows along.
        rerun.or(moved)
    }

    /// The editable inputs; returns a Rerun if any changed.
    fn inputs_row(&mut self, ui: &mut egui::Ui) -> Option<EmulateOutcome> {
        if self.inputs.is_empty() {
            ui.weak("This function needs no inputs.");
            return None;
        }
        let mut changed = false;
        egui::Grid::new("emulate_inputs")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for (name, value) in &mut self.inputs {
                    let label = ui.monospace(name.as_str());
                    changed |= ui
                        .add(egui::DragValue::new(value).range(0..=i64::MAX))
                        .labelled_by(label.id)
                        .changed();
                    ui.end_row();
                }
            });
        changed.then(|| EmulateOutcome::Rerun(self.inputs.clone()))
    }

    /// Step / Run / Reset. Returns a Reveal when the cursor moves.
    fn controls(&mut self, ui: &mut egui::Ui) -> Option<EmulateOutcome> {
        let total = self.trace.steps.len();
        let mut moved = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.cursor < total, egui::Button::new("Step"))
                .clicked()
            {
                self.cursor += 1;
                moved = true;
            }
            if ui
                .add_enabled(self.cursor < total, egui::Button::new("Run"))
                .on_hover_text("Run to the first overflow, or the end")
                .clicked()
            {
                self.cursor = self.run_target();
                moved = true;
            }
            if ui
                .add_enabled(self.cursor > 0, egui::Button::new("Reset"))
                .clicked()
            {
                self.cursor = 0;
            }
            ui.weak(format!("step {} / {total}", self.cursor));
            if let Some(step) = self.trace.first_overflow() {
                ui.colored_label(OVERFLOW_COLOR, format!("⚠ overflow at line {}", step.line));
            }
        });
        moved
            .then(|| self.current_line())
            .flatten()
            .map(EmulateOutcome::Reveal)
    }

    /// Where **Run** stops: just after the first overflow, else the end.
    fn run_target(&self) -> usize {
        self.trace
            .steps
            .iter()
            .position(|s| s.overflow.is_some())
            .map_or(self.trace.steps.len(), |i| i + 1)
    }

    /// The source line of the step the cursor is on (the last shown).
    fn current_line(&self) -> Option<u32> {
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.trace.steps.get(i))
            .map(|s| s.line)
    }

    fn step_log(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, step) in self.trace.steps.iter().enumerate() {
                    show_step(ui, index, step, self.cursor);
                }
            });
    }
}

fn show_step(ui: &mut egui::Ui, index: usize, step: &Step, cursor: usize) {
    let shown = index < cursor;
    let current = index + 1 == cursor;
    let line = RichText::new(format!("L{:<4}", step.line)).monospace();
    ui.horizontal_wrapped(|ui| {
        ui.label(if current {
            line.color(CURRENT_COLOR).strong()
        } else if shown {
            line
        } else {
            line.weak()
        });
        let text = RichText::new(&step.text).monospace();
        ui.label(if shown { text } else { text.weak() });
        if let Some(overflow) = &step.overflow {
            if shown {
                ui.colored_label(
                    OVERFLOW_COLOR,
                    format!(
                        "⚠ writes to byte {} of {}, which is {} bytes",
                        overflow.end, overflow.buffer, overflow.size
                    ),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::analysis::emulate::{Input, Overflow, Step as EStep, Trace};

    fn overflowing_trace() -> Trace {
        Trace {
            function: "f".into(),
            first_line: 1,
            last_line: 5,
            inputs: vec![Input {
                name: "n".into(),
                value: 16,
            }],
            steps: vec![
                EStep {
                    line: 2,
                    text: "b = malloc(8)".into(),
                    overflow: None,
                },
                EStep {
                    line: 3,
                    text: "memcpy(…) writes 16 bytes".into(),
                    overflow: Some(Overflow {
                        buffer: "b".into(),
                        end: 16,
                        size: 8,
                    }),
                },
            ],
        }
    }

    #[test]
    fn it_flags_the_overflow_and_steps_through() {
        let view = EmulateView::new("a.c".into(), 1, overflowing_trace());
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, view: &mut EmulateView| {
                let ctx = ui.ctx().clone();
                view.show(&ctx);
            },
            view,
        );
        harness.run();
        // The overflow is summarised as soon as the window opens.
        harness.get_by_label("⚠ overflow at line 3");
        // Run advances the cursor and reveals the detailed overflow note.
        harness.get_by_label("Run").click();
        harness.run();
        assert!(harness
            .query_by_label("⚠ writes to byte 16 of b, which is 8 bytes")
            .is_some());
    }
}
