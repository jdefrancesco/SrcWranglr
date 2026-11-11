//! The History pane: the commits whose message says they fixed a bug or a
//! security problem, newest first, with the lines of today's code each one
//! last changed. Pick a commit to list them; click one to show it.

use egui::text::LayoutJob;
use egui::{FontId, RichText};

use std::path::PathBuf;

use super::history::{HistoryJob, HistoryView};
use super::history_cves::{cve_list, CveAction};
use super::history_function::FunctionHistory;
use super::layout::Layout;
use super::pane_bar::{self, Edge};
use super::visible::{self, visible};
use crate::analysis::Severity;
use crate::history::{date, History, Limits, Strength};
use crate::model::{CodeGraph, SymbolId};
use crate::style::severity_text_color;

const DEFAULT_WIDTH: f32 = 420.0;
/// The places list's height until dragged, and its bounds.
const PLACES_HEIGHT: f32 = 220.0;
const MIN_PLACES_HEIGHT: f32 = 80.0;
/// The most of the pane's height the places list may take.
const MAX_PLACES_SHARE: f32 = 0.6;

/// What the user picked in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryPanelAction {
    /// Show this line in the source.
    GoTo {
        symbol: Option<SymbolId>,
        file: String,
        line: u32,
    },
}

/// Which list the pane shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum HistoryTab {
    /// Fix commits and the lines they last changed.
    #[default]
    FixCommits,
    /// Who last changed the selected function's lines.
    Function,
    /// The CVEs named in the fix commits.
    Cves,
}

/// Where reading the history is.
enum HistoryState {
    /// Not read, and why (the demo, no project).
    Unavailable(String),
    Reading(HistoryJob),
    Read(Box<HistoryView>),
    Failed(String),
}

impl Default for HistoryState {
    fn default() -> Self {
        HistoryState::Unavailable("Open a project to read its git history.".into())
    }
}

pub struct HistoryPanel {
    is_open: bool,
    tab: HistoryTab,
    /// The checkout whose history is read.
    root: Option<PathBuf>,
    state: HistoryState,
    function: FunctionHistory,
    query: String,
    is_security_only: bool,
    is_owning_only: bool,
    /// The rows listed (indices into the view's rows), for `rows_for`.
    rows: Vec<usize>,
    rows_for: Option<(String, bool, bool)>,
    /// The commit whose places are listed, by row index.
    selected: Option<usize>,
}

impl Default for HistoryPanel {
    fn default() -> Self {
        Self {
            is_open: false,
            tab: HistoryTab::default(),
            root: None,
            state: HistoryState::default(),
            function: FunctionHistory::default(),
            query: String::new(),
            is_security_only: false,
            is_owning_only: true,
            rows: Vec::new(),
            rows_for: None,
            selected: None,
        }
    }
}

impl HistoryPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Starts showing what `job` reads of the checkout at `root`,
    /// forgetting the last project's.
    pub fn start(&mut self, job: HistoryJob, root: PathBuf) {
        self.reset(HistoryState::Reading(job), Some(root));
    }

    /// Says the history won't be read, and why.
    pub fn set_unavailable(&mut self, why: &str) {
        self.reset(HistoryState::Unavailable(why.to_string()), None);
    }

    fn reset(&mut self, state: HistoryState, root: Option<PathBuf>) {
        self.state = state;
        self.root = root;
        self.function.clear();
        self.rows_for = None;
        self.selected = None;
    }

    /// Once the history is read: it, arranged for `graph`. Only the frame it
    /// arrives on.
    pub fn poll(&mut self, graph: &CodeGraph) -> Option<&HistoryView> {
        let HistoryState::Reading(job) = &self.state else {
            return None;
        };
        let outcome = job.poll()?;
        self.state = match outcome {
            Ok(history) => HistoryState::Read(Box::new(HistoryView::new(history, graph))),
            Err(error) => HistoryState::Failed(error),
        };
        match &self.state {
            HistoryState::Read(view) => Some(view),
            _ => None,
        }
    }

    /// Draws the pane (while open, sliding in and out) at the right of
    /// `ui`.
    /// `selected` is the symbol selected in the graph.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        layout: Layout,
        selected: Option<SymbolId>,
    ) -> Option<HistoryPanelAction> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("history_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| {
                self.contents(ui, graph, layout, selected)
            });
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        layout: Layout,
        selected: Option<SymbolId>,
    ) -> Option<HistoryPanelAction> {
        let title = match &self.state {
            HistoryState::Read(view) => {
                let owning = view.rows.iter().filter(|r| r.lines > 0).count();
                format!("Git History ({owning})")
            }
            _ => "Git History".to_string(),
        };
        if pane_bar::heading(ui, title, Edge::Right) {
            self.is_open = false;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, HistoryTab::FixCommits, "Fix commits");
            ui.selectable_value(&mut self.tab, HistoryTab::Function, "Selected function");
            ui.selectable_value(&mut self.tab, HistoryTab::Cves, "CVEs");
        });
        if self.tab == HistoryTab::Function {
            return self
                .function
                .show(ui, graph, self.root.as_deref(), selected);
        }
        let view = match &self.state {
            HistoryState::Unavailable(why) | HistoryState::Failed(why) => {
                ui.weak(why.as_str());
                return None;
            }
            HistoryState::Reading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the git history…");
                });
                return None;
            }
            HistoryState::Read(view) => view,
        };
        if self.tab == HistoryTab::Cves {
            let picked = cve_list(ui, view, graph);
            return match picked? {
                CveAction::ShowCommit(commit) => {
                    self.show_commit(commit);
                    None
                }
                CveAction::GoTo(action) => Some(action),
            };
        }
        summary(ui, &view.history);
        self.controls(ui);
        ui.separator();
        self.refresh();
        let HistoryState::Read(view) = &self.state else {
            return None;
        };
        if self.rows.is_empty() {
            ui.weak(
                "No commits match. Fix commits are found by words in their messages: \
                 fix, bug, overflow, CVE-…, and the like.",
            );
            return None;
        }
        let picked = self
            .selected
            .and_then(|row| view.rows.get(row).map(|r| (row, r)));
        let action = picked
            .and_then(|(_, row)| show_places(ui, view, row.commit, &row.places, graph, layout));
        let clicked = commit_rows(ui, view, &self.rows, self.selected);
        if clicked.is_some() {
            self.selected = clicked;
        }
        action
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let filter = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Filter commits…")
            .desired_width(f32::INFINITY);
        ui.add(filter);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.is_security_only, "Security words only");
            ui.checkbox(&mut self.is_owning_only, "Only ones in today's code");
        });
    }

    /// Recomputes the rows if the filter changed, letting go of a picked
    /// commit it now hides.
    fn refresh(&mut self) {
        let HistoryState::Read(view) = &self.state else {
            return;
        };
        let wanted = (
            self.query.clone(),
            self.is_security_only,
            self.is_owning_only,
        );
        if self.rows_for.as_ref() == Some(&wanted) {
            return;
        }
        self.rows = view.listed(&wanted.0, wanted.1, wanted.2);
        self.rows_for = Some(wanted);
        if self.selected.is_some_and(|s| !self.rows.contains(&s)) {
            self.selected = None;
        }
    }

    /// Shows `commit` (by index into the fix commits) picked in Fix
    /// commits, whatever the filter hid.
    fn show_commit(&mut self, commit: usize) {
        self.tab = HistoryTab::FixCommits;
        self.query.clear();
        self.is_security_only = false;
        self.is_owning_only = false;
        self.refresh();
        let HistoryState::Read(view) = &self.state else {
            return;
        };
        self.selected = view.rows.iter().position(|row| row.commit == commit);
    }

    #[cfg(test)]
    fn set_history(&mut self, history: History, graph: &CodeGraph) {
        let state = HistoryState::Read(Box::new(HistoryView::new(history, graph)));
        self.reset(state, None);
    }
}

/// What was read: from which commit, how much, and what was left out.
fn summary(ui: &mut egui::Ui, history: &History) {
    let read = match history.is_walk_cut_short {
        true => format!("the newest {} commits", history.commits_read),
        false => format!("all {} commits", history.commits_read),
    };
    ui.weak(format!(
        "{} fix commits in {read} from {} (the checked-out commit).",
        history.fix_commits.len(),
        history.head
    ));
    if history.commits_read >= Limits::default().max_commits {
        ui.weak("Older history wasn't read.");
    }
    if !history.files_changed.is_empty() {
        let names: Vec<String> = history
            .files_changed
            .iter()
            .map(|f| visible(f).into_owned())
            .collect();
        ui.weak(format!(
            "{} file(s) changed since that commit weren't traced.",
            names.len()
        ))
        .on_hover_text(names.join("\n"));
    }
    if history.files_not_traced > 0 {
        ui.weak(format!(
            "{} file(s) weren't traced (time or size limits).",
            history.files_not_traced
        ));
    }
}

/// The commits, one per row: date, id, summary, lines owned. Returns the
/// row clicked.
fn commit_rows(
    ui: &mut egui::Ui,
    view: &HistoryView,
    rows: &[usize],
    selected: Option<usize>,
) -> Option<usize> {
    let row_height = ui.spacing().interact_size.y;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let visuals = ui.visuals().clone();
    egui::ScrollArea::vertical()
        .id_salt("history_rows")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, rows.len(), |ui, range| {
            // Rows must stay one line high for `show_rows` to place them.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let mut clicked = None;
            for &index in &rows[range] {
                let Some(row) = view.rows.get(index) else {
                    continue;
                };
                let label = row_label(view, row.commit, row.lines, font.clone(), &visuals);
                if ui
                    .selectable_label(selected == Some(index), label)
                    .clicked()
                {
                    clicked = Some(index);
                }
            }
            clicked
        })
        .inner
}

/// `2024-03-01  1a2b3c4d  Fix overflow in parse  (12 lines)`.
fn row_label(
    view: &HistoryView,
    commit: usize,
    lines: u32,
    font: FontId,
    visuals: &egui::Visuals,
) -> LayoutJob {
    let (text, weak) = (visuals.text_color(), visuals.weak_text_color());
    let commit = &view.history.fix_commits[commit];
    let color = match commit.strength {
        Strength::Security => severity_text_color(Severity::Medium, visuals),
        Strength::Fix => text,
    };
    let mut job = visible::job(
        &format!("{}  {}  ", date(commit.time), commit.short),
        font.clone(),
        weak,
    );
    visible::append(&mut job, &commit.summary, font.clone(), color);
    let owned = match lines {
        0 => "  (none left)".to_string(),
        1 => "  (1 line)".to_string(),
        n => format!("  ({n} lines)"),
    };
    visible::append(&mut job, &owned, font, weak);
    job
}

/// The commit's details and the places it last changed, in a list at the
/// bottom of `ui`. Returns the place clicked.
fn show_places(
    ui: &mut egui::Ui,
    view: &HistoryView,
    commit: usize,
    places: &[usize],
    graph: &CodeGraph,
    layout: Layout,
) -> Option<HistoryPanelAction> {
    let commit = &view.history.fix_commits[commit];
    let most = (ui.available_height() * MAX_PLACES_SHARE).max(MIN_PLACES_HEIGHT);
    egui::Panel::bottom(layout.pane_id("history_places"))
        .resizable(true)
        .default_size(PLACES_HEIGHT)
        .size_range(MIN_PLACES_HEIGHT..=most)
        .show(ui, |ui| {
            ui.add_space(ui.spacing().item_spacing.y);
            let font = egui::TextStyle::Body.resolve(ui.style());
            ui.label(visible::job(
                &commit.summary,
                font,
                ui.visuals().strong_text_color(),
            ));
            ui.weak(format!(
                "{}  ·  {}  ·  {}  ·  {}",
                commit.short,
                date(commit.time),
                visible(&commit.author),
                commit.keywords.join(", ")
            ));
            ui.label(RichText::new(match places.len() {
                0 => "None of today's code was last changed by it.".to_string(),
                _ => "Lines of today's code it last changed:".to_string(),
            }));
            egui::ScrollArea::vertical()
                .id_salt("history_places_rows")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let mut picked = None;
                    for &index in places {
                        let (Some(owned), function) = (
                            view.owned(index),
                            view.functions.get(index).copied().flatten(),
                        ) else {
                            continue;
                        };
                        let name = function
                            .and_then(|id| graph.symbols.get(&id))
                            .map_or("(file scope)", |s| s.qualified_name.as_str());
                        let lines = match owned.line_count {
                            1 => owned.first_line.to_string(),
                            _ => format!("{}–{}", owned.first_line, owned.last_line()),
                        };
                        let label = format!("{}:{lines}  {}", visible(&owned.file), visible(name));
                        if ui.selectable_label(false, label).clicked() {
                            picked = Some(HistoryPanelAction::GoTo {
                                symbol: function,
                                file: owned.file.clone(),
                                line: owned.first_line,
                            });
                        }
                    }
                    picked
                })
                .inner
        })
        .inner
}

#[cfg(test)]
#[path = "history_panel_tests.rs"]
mod tests;
