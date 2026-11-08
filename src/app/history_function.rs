//! The History pane's "Selected function" tab: who last changed each line
//! of the function you're looking at, as checked out. Each file's history
//! is read on demand, in the background, and kept while the project is
//! open.

use std::collections::HashMap;
use std::path::Path;

use egui::text::LayoutJob;
use egui::{Color32, FontId};

use super::history::{read_file_history, BackgroundJob};
use super::history_panel::HistoryPanelAction;
use super::visible::{self, visible};
use crate::analysis::Severity;
use crate::history::{commits_in, date, FileHistory, Strength};
use crate::model::{CodeGraph, FunctionMetrics, SymbolId};
use crate::style::severity_text_color;

/// Files whose history is kept at once; more and the oldest are let go.
const MAX_FILES_KEPT: usize = 32;

enum FileState {
    Reading(BackgroundJob<FileHistory>),
    Read(Result<FileHistory, String>),
}

#[derive(Default)]
pub struct FunctionHistory {
    files: HashMap<String, FileState>,
}

impl FunctionHistory {
    /// Forgets every file's history (a new project).
    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Draws the tab for `selected` in `graph`, reading its file's history
    /// from the checkout at `root` if needed. Returns the line clicked.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        root: Option<&Path>,
        selected: Option<SymbolId>,
    ) -> Option<HistoryPanelAction> {
        let Some(function) =
            selected.and_then(|id| graph.functions.iter().find(|f| f.symbol == id))
        else {
            ui.weak("Select a function to see who last changed its lines.");
            return None;
        };
        let name = graph
            .symbols
            .get(&function.symbol)
            .map_or("", |s| s.qualified_name.as_str());
        let end = function
            .line
            .saturating_add(function.lines.saturating_sub(1));
        ui.strong(visible(name));
        ui.weak(format!(
            "{}:{}–{end}",
            visible(&function.file),
            function.line
        ));
        let Some(root) = root else {
            ui.weak("This project has no git history to read.");
            return None;
        };
        match self.state_of(&function.file, root, ui.ctx()) {
            FileState::Reading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading this file's history…");
                });
                None
            }
            FileState::Read(Err(error)) => {
                ui.weak(error.as_str());
                None
            }
            FileState::Read(Ok(history)) => commit_list(ui, history, function),
        }
    }

    /// `file`'s state, starting to read it if it hasn't been, and taking in
    /// a finished read.
    fn state_of(&mut self, file: &str, root: &Path, ctx: &egui::Context) -> &FileState {
        if !self.files.contains_key(file) {
            // One read at a time: one still running for another file is
            // stopped (its worker killed), and read again if picked again.
            self.files
                .retain(|_, state| matches!(state, FileState::Read(_)));
            if self.files.len() >= MAX_FILES_KEPT {
                self.files.clear();
            }
            let job = read_file_history(root.to_path_buf(), file.to_string(), ctx.clone());
            self.files.insert(file.to_string(), FileState::Reading(job));
        }
        let state = self.files.get_mut(file).expect("just inserted");
        if let FileState::Reading(job) = state {
            if let Some(result) = job.poll() {
                *state = FileState::Read(result);
            }
        }
        state
    }
}

/// The commits that last changed `function`'s lines, newest first; the
/// one clicked, as its first line there.
fn commit_list(
    ui: &mut egui::Ui,
    history: &FileHistory,
    function: &FunctionMetrics,
) -> Option<HistoryPanelAction> {
    let lines = function.line..function.line.saturating_add(function.lines);
    let commits = commits_in(history, lines);
    let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
        ui.weak("None of its lines are in the checked-out commit.");
        return None;
    };
    ui.label(format!(
        "{} commit(s) last changed its {} lines, from {} to {}:",
        commits.len(),
        function.lines,
        date(history.commits[oldest.0].time),
        date(history.commits[newest.0].time)
    ));
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let visuals = ui.visuals().clone();
    egui::ScrollArea::vertical()
        .id_salt("function_history_rows")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let mut picked = None;
            for &(commit, count, first) in &commits {
                let label = row_label(history, commit, count, font.clone(), &visuals);
                if ui.selectable_label(false, label).clicked() {
                    picked = Some(HistoryPanelAction::GoTo {
                        symbol: Some(function.symbol),
                        file: function.file.clone(),
                        line: first,
                    });
                }
            }
            picked
        })
        .inner
}

/// `2024-03-01  1a2b3c4d  Tess  Fix overflow  (12 lines)`: security fixes
/// in orange, other fixes stronger than the rest.
fn row_label(
    history: &FileHistory,
    commit: usize,
    lines: u32,
    font: FontId,
    visuals: &egui::Visuals,
) -> LayoutJob {
    let commit = &history.commits[commit];
    let weak = visuals.weak_text_color();
    let color: Color32 = match commit.strength {
        Some(Strength::Security) => severity_text_color(Severity::Medium, visuals),
        Some(Strength::Fix) => visuals.strong_text_color(),
        None => visuals.text_color(),
    };
    let mut job = visible::job(
        &format!("{}  {}  ", date(commit.time), commit.short),
        font.clone(),
        weak,
    );
    visible::append(
        &mut job,
        &format!("{}  ", commit.author),
        font.clone(),
        weak,
    );
    visible::append(&mut job, &commit.summary, font.clone(), color);
    let owned = match lines {
        1 => "  (1 line)".to_string(),
        n => format!("  ({n} lines)"),
    };
    visible::append(&mut job, &owned, font, weak);
    job
}
