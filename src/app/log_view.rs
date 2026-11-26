//! The Log window: SrcWranglr's own diagnostics — panics (with their
//! backtrace), background-job failures and internal errors — so a problem,
//! or a crash the previous run left in the file, can be read and copied.
//! The log itself lives in [`super::diag`].

use egui::{Color32, RichText};

use super::diag::{self, Level, Line};
use super::{SrcWranglrApp, Status};
use crate::notes::journal::date_time;

const ERROR_COLOR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);
const WARN_COLOR: Color32 = Color32::from_rgb(0xE0, 0xB0, 0x6C);
const DEFAULT_SIZE: [f32; 2] = [720.0, 440.0];

/// The Log window's state.
pub struct LogView {
    is_open: bool,
    /// Keep the newest line in view as lines arrive.
    follow: bool,
}

impl LogView {
    pub fn open() -> Self {
        Self {
            is_open: true,
            follow: true,
        }
    }

    /// Draws the window. Returns whether it is still open.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut is_open = true;
        egui::Window::new("SrcWranglr Log")
            .open(&mut is_open)
            .default_size(DEFAULT_SIZE)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui));
        self.is_open = is_open && self.is_open;
        self.is_open
    }

    fn contents(&mut self, ui: &mut egui::Ui) {
        let lines = diag::lines();
        ui.horizontal(|ui| {
            if ui.button("Copy All").clicked() {
                ui.ctx().copy_text(as_text(&lines));
            }
            if ui.button("Clear").clicked() {
                diag::clear();
            }
            ui.checkbox(&mut self.follow, "Follow");
        });
        if let Some(path) = diag::file_path() {
            ui.horizontal(|ui| {
                ui.weak("Log file:");
                ui.monospace(path.display().to_string());
                if ui.small_button("Copy path").clicked() {
                    ui.ctx().copy_text(path.display().to_string());
                }
            });
        }
        ui.separator();
        if lines.is_empty() {
            ui.weak(
                "Nothing logged yet. Analyzer runs, background-job failures, errors and \
                 panics show here.",
            );
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow)
            .show(ui, |ui| {
                for line in &lines {
                    show_line(ui, line);
                }
            });
    }
}

fn show_line(ui: &mut egui::Ui, line: &Line) {
    let color = level_color(ui, line.level);
    let head = format!("{} {}", date_time(line.when), line.level.tag());
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(head).monospace().color(color).strong());
        // The first line beside the heading; a backtrace flows below.
        ui.label(RichText::new(line.text.as_str()).monospace());
    });
}

fn level_color(ui: &egui::Ui, level: Level) -> Color32 {
    match level {
        Level::Info => ui.visuals().weak_text_color(),
        Level::Warn => WARN_COLOR,
        Level::Error | Level::Panic => ERROR_COLOR,
    }
}

/// The lines as plain text for the clipboard.
fn as_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| format!("[{} {}] {}", date_time(l.when), l.level.tag(), l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

impl SrcWranglrApp {
    /// Opens the Log window (Help › Show Log, or ⌘P).
    pub(super) fn open_log(&mut self) {
        self.log_view = Some(LogView::open());
    }

    pub(super) fn show_log_view(&mut self, ctx: &egui::Context) {
        let Some(view) = self.log_view.as_mut() else {
            return;
        };
        if !view.show(ctx) {
            self.log_view = None;
        }
    }

    /// Copies a new status-bar error into the log, so internal errors are
    /// kept with the panics. Logged once while it stays shown.
    pub(super) fn mirror_status_to_log(&mut self) {
        match &self.status {
            Status::Error(text) if self.logged_status.as_deref() != Some(text) => {
                diag::record(Level::Error, text.clone());
                self.logged_status = Some(text.clone());
            }
            Status::Error(_) => {}
            Status::Info(_) => self.logged_status = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn the_window_opens_and_offers_copy_and_clear() {
        // diag is not installed in tests, so the log is empty.
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, view: &mut LogView| {
                let ctx = ui.ctx().clone();
                view.show(&ctx);
            },
            LogView::open(),
        );
        harness.run();
        harness.get_by_label("Copy All");
        harness.get_by_label("Clear");
        harness.get_by_label_contains("Nothing logged");
    }

    #[test]
    fn copy_text_has_a_line_each_with_its_level() {
        let lines = [
            Line {
                when: 1_700_000_000,
                level: Level::Error,
                text: "bad thing".into(),
            },
            Line {
                when: 1_700_000_001,
                level: Level::Panic,
                text: "boom".into(),
            },
        ];
        let text = as_text(&lines);
        assert!(text.contains("ERROR] bad thing"), "{text}");
        assert!(text.contains("PANIC] boom"), "{text}");
        assert_eq!(text.lines().count(), 2);
    }
}
