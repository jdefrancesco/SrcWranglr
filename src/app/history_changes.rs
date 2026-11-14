//! The History pane's Changes tab: compare the checkout with a base you
//! pick (a tag, a branch or a commit) to audit what changed since: the
//! functions it changed, most findings first, the files and the commits.
//! The source then marks the changed lines, and Findings can show only the
//! changed code. Everything here comes from the audited repository, so it
//! is shown with hidden characters marked.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::RichText;

use super::history::BackgroundJob;
use super::history_panel::HistoryPanelAction;
use super::range::RangeView;
use super::survey_view::ERROR_COLOR;
use super::visible::visible;
use crate::analysis::Finding;
use crate::history::changes::{Changes, Refs};
use crate::history::date;
use crate::history::worker::{changes_in_worker, refs_in_worker};
use crate::model::CodeGraph;

/// Tags and branches listed in the picker at most.
const MAX_REFS_SHOWN: usize = 100;
/// Rows of each list drawn at most; filtering finds the rest.
const MAX_ROWS_SHOWN: usize = 300;

#[derive(Default)]
enum RefsState {
    #[default]
    NotRead,
    Reading(BackgroundJob<Refs>),
    Read(Refs),
    Failed,
}

#[derive(Default)]
pub struct ChangesTab {
    base: String,
    refs: RefsState,
    job: Option<BackgroundJob<Changes>>,
    /// Shared with the source view, which marks its changed lines.
    range: Option<Arc<RangeView>>,
    error: Option<String>,
    filter: String,
    /// Findings per changed function, and how many findings there were
    /// when counted.
    counts: Option<(usize, Vec<usize>)>,
    /// The changed functions listed, in order, and the filter they are
    /// for.
    order: Option<(String, Vec<usize>)>,
}

impl ChangesTab {
    /// Forgets the compared range and what was being read (a new project).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The range compared, if any.
    pub fn range(&self) -> Option<&RangeView> {
        self.range.as_deref()
    }

    /// The range compared, to share.
    pub fn shared_range(&self) -> Option<Arc<RangeView>> {
        self.range.clone()
    }

    /// Draws the tab for the checkout at `root`. Returns what was clicked.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &CodeGraph,
        root: Option<&Path>,
        findings: &[Finding],
    ) -> Option<HistoryPanelAction> {
        let Some(root) = root else {
            ui.weak("This project has no git history to compare.");
            return None;
        };
        self.read_refs(root, ui.ctx());
        self.poll_jobs(graph);
        ui.label("Compare the checkout with an older tag, branch or commit to audit what changed.");
        if let Some(base) = self.base_row(ui) {
            self.start(base, graph, root, ui.ctx());
        }
        if self.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Comparing…");
            });
            return None;
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(visible(error)).color(ERROR_COLOR));
        }
        self.range.as_ref()?;
        self.range_view(ui, findings)
    }

    /// Starts reading the tags and branches, once.
    fn read_refs(&mut self, root: &Path, ctx: &egui::Context) {
        if matches!(self.refs, RefsState::NotRead) {
            let root = root.to_path_buf();
            self.refs = RefsState::Reading(BackgroundJob::spawn(
                "Reading the tags",
                move |stop| refs_in_worker(&root, stop),
                ctx.clone(),
            ));
        }
    }

    /// Takes in finished reads: called every frame, shown or not, so the
    /// source and Findings get a comparison as soon as it is done.
    pub fn poll_jobs(&mut self, graph: &CodeGraph) {
        if let RefsState::Reading(job) = &self.refs {
            if let Some(result) = job.poll() {
                self.refs = result.map_or(RefsState::Failed, RefsState::Read);
            }
        }
        let Some(result) = self.job.as_ref().and_then(BackgroundJob::poll) else {
            return;
        };
        self.job = None;
        match result {
            Ok(changes) => self.range = Some(Arc::new(RangeView::new(changes, graph))),
            Err(error) => self.error = Some(error),
        }
        self.counts = None;
        self.order = None;
    }

    /// The base field, Compare, and the picker; the base to compare with,
    /// if asked.
    fn base_row(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let mut asked = None;
        ui.horizontal_wrapped(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.base)
                    .hint_text("v6.6, main, or a commit id")
                    .desired_width(180.0),
            );
            let is_entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can_compare = self.job.is_none() && !self.base.trim().is_empty();
            let compare = ui.add_enabled(can_compare, egui::Button::new("Compare"));
            if compare.clicked() || (is_entered && can_compare) {
                asked = Some(self.base.trim().to_string());
            }
            let picked = ui
                .add_enabled_ui(self.job.is_none(), |ui| self.picker(ui))
                .inner;
            if let Some(picked) = picked {
                self.base = picked.clone();
                asked = Some(picked);
            }
        });
        asked
    }

    /// The tags (newest first) and branches to pick a base from.
    fn picker(&self, ui: &mut egui::Ui) -> Option<String> {
        let RefsState::Read(refs) = &self.refs else {
            return None;
        };
        if refs.tags.is_empty() && refs.branches.is_empty() {
            return None;
        }
        let mut picked = None;
        let caption = ui.weak("or");
        egui::ComboBox::from_id_salt("changes_base")
            .selected_text("Tags and branches")
            .show_ui(ui, |ui| {
                let names = refs.tags.iter().chain(&refs.branches).take(MAX_REFS_SHOWN);
                for name in names {
                    if ui.selectable_label(false, visible(name)).clicked() {
                        picked = Some(name.clone());
                    }
                }
            })
            .response
            .labelled_by(caption.id);
        picked
    }

    fn start(&mut self, base: String, graph: &CodeGraph, root: &Path, ctx: &egui::Context) {
        let files: Vec<String> = graph.file_contents.keys().cloned().collect();
        let root: PathBuf = root.to_path_buf();
        self.error = None;
        self.range = None;
        self.job = Some(BackgroundJob::spawn(
            "Comparing commits",
            move |stop| changes_in_worker(&root, &base, files, stop),
            ctx.clone(),
        ));
    }

    /// What the range changed: a summary, then the functions, files and
    /// commits.
    fn range_view(
        &mut self,
        ui: &mut egui::Ui,
        findings: &[Finding],
    ) -> Option<HistoryPanelAction> {
        let range = Arc::clone(self.range.as_ref()?);
        if self
            .counts
            .as_ref()
            .is_none_or(|(n, _)| *n != findings.len())
        {
            self.counts = Some((findings.len(), range.findings_in_functions(findings)));
            self.order = None;
        }
        if summary(ui, &range) {
            self.range = None;
            self.counts = None;
            self.order = None;
            return None;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter functions and files…")
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        let needle = self.filter.trim().to_lowercase();
        let counts = self.counts.as_ref().map_or(&[][..], |(_, c)| c.as_slice());
        if self.order.as_ref().is_none_or(|(n, _)| *n != needle) {
            self.order = Some((needle.clone(), function_order(&range, counts, &needle)));
        }
        let order = self.order.as_ref().map_or(&[][..], |(_, o)| o.as_slice());
        egui::ScrollArea::vertical()
            .id_salt("changes_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let picked = function_rows(ui, &range, counts, order);
                let file = file_rows(ui, &range, &needle);
                commit_rows(ui, &range);
                picked.or(file)
            })
            .inner
    }
}

/// The range's ends, sizes and caveats, with Clear; whether Clear was
/// clicked.
fn summary(ui: &mut egui::Ui, range: &RangeView) -> bool {
    let changes = &range.changes;
    let (added, removed) = changes.files.iter().fold((0u64, 0u64), |(a, r), f| {
        (a + u64::from(f.added), r + u64::from(f.removed))
    });
    let more = if changes.is_walk_cut_short { "+" } else { "" };
    let mut is_cleared = false;
    ui.horizontal_wrapped(|ui| {
        ui.strong(visible(&range.label()));
        is_cleared = ui
            .small_button("Clear")
            .on_hover_text("Stop marking this range in the source and Findings")
            .clicked();
    });
    ui.label(format!(
        "{}{more} commits · {} files changed (+{added} −{removed}) · {} functions",
        changes.commit_count,
        changes.files.len(),
        range.functions.len()
    ));
    caveats(ui, changes);
    is_cleared
}

/// What to know about the comparison: a base newer than the checkout,
/// files left out.
fn caveats(ui: &mut egui::Ui, changes: &Changes) {
    let base = visible(&changes.base);
    if changes.base_ahead > 0 && changes.commit_count == 0 {
        ui.label(
            RichText::new(format!(
                "{base} is newer than the checkout ({} commits it lacks): these changes run \
                 backwards. Check out the newer end, and compare it with the older.",
                changes.base_ahead
            ))
            .color(ERROR_COLOR),
        );
    } else if changes.base_ahead > 0 {
        ui.weak(format!(
            "{base} has {} commits the checkout lacks: they went separate ways.",
            changes.base_ahead
        ));
    }
    let notes = [
        (changes.deleted.len(), "deleted"),
        (
            changes.files_changed_on_disk.len(),
            "that differ from the commit on disk (edited, or converted by the repository's \
             settings), left out",
        ),
        (changes.files_not_read, "not read (too large or binary)"),
    ];
    for (count, what) in notes.into_iter().filter(|(count, _)| *count > 0) {
        ui.weak(format!("{count} files {what}."));
    }
    if changes.is_cut_short {
        ui.weak("Not every changed file was compared (limits).");
    }
}

/// The changed functions `needle` matches (by name or file), most
/// findings then most changed lines first.
fn function_order(range: &RangeView, counts: &[usize], needle: &str) -> Vec<usize> {
    let mut order: Vec<usize> = (0..range.functions.len())
        .filter(|&i| {
            let f = &range.functions[i];
            needle.is_empty()
                || f.name.to_lowercase().contains(needle)
                || f.file.to_lowercase().contains(needle)
        })
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(counts.get(i).copied().unwrap_or(0)));
    order
}

/// The first `MAX_ROWS_SHOWN` of the functions in `order`; the one
/// clicked.
fn function_rows(
    ui: &mut egui::Ui,
    range: &RangeView,
    counts: &[usize],
    order: &[usize],
) -> Option<HistoryPanelAction> {
    ui.strong(format!("Changed functions ({})", order.len()));
    let mut picked = None;
    for &i in order.iter().take(MAX_ROWS_SHOWN) {
        let function = &range.functions[i];
        ui.horizontal_wrapped(|ui| {
            let name = RichText::new(format!("{}()", visible(&function.name))).monospace();
            if ui.link(name).on_hover_text("Show it").clicked() {
                picked = Some(HistoryPanelAction::GoTo {
                    symbol: Some(function.symbol),
                    file: function.file.clone(),
                    line: function.line,
                });
            }
            let findings = match counts.get(i).copied().unwrap_or(0) {
                0 => String::new(),
                1 => " · 1 finding".to_string(),
                n => format!(" · {n} findings"),
            };
            ui.weak(format!(
                "{}:{} · {} changed{findings}",
                visible(&function.file),
                function.line,
                lines(function.changed_lines)
            ));
        });
    }
    more_rows(ui, order.len());
    picked
}

/// Says how many rows weren't shown, if any.
fn more_rows(ui: &mut egui::Ui, total: usize) {
    if total > MAX_ROWS_SHOWN {
        ui.weak(format!(
            "…and {} more: filter to find them.",
            total - MAX_ROWS_SHOWN
        ));
    }
}

/// The changed files; the one clicked, at its first change.
fn file_rows(ui: &mut egui::Ui, range: &RangeView, needle: &str) -> Option<HistoryPanelAction> {
    let files: Vec<_> = range
        .changes
        .files
        .iter()
        .filter(|f| needle.is_empty() || f.file.to_lowercase().contains(needle))
        .collect();
    let mut picked = None;
    egui::CollapsingHeader::new(format!("Files ({})", files.len()))
        .id_salt("changes_files")
        .show(ui, |ui| {
            for file in files.iter().take(MAX_ROWS_SHOWN) {
                ui.horizontal_wrapped(|ui| {
                    if ui.link(visible(&file.file)).clicked() {
                        picked = Some(HistoryPanelAction::GoTo {
                            symbol: None,
                            file: file.file.clone(),
                            line: file.hunks.first().map_or(1, |h| h.first_line),
                        });
                    }
                    let how = match (&file.renamed_from, file.is_new) {
                        (Some(from), _) => format!(" · moved from {}", visible(from)),
                        (None, true) => " · new".to_string(),
                        (None, false) => String::new(),
                    };
                    ui.weak(format!("+{} −{}{how}", file.added, file.removed));
                });
            }
            more_rows(ui, files.len());
        });
    picked
}

/// The range's commits, newest first.
fn commit_rows(ui: &mut egui::Ui, range: &RangeView) {
    let changes = &range.changes;
    let listed = changes.commits.len();
    let title = match listed < changes.commit_count {
        true => format!("Commits (newest {listed} of {})", changes.commit_count),
        false => format!("Commits ({listed})"),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("changes_commits")
        .show(ui, |ui| {
            for commit in changes.commits.iter().take(MAX_ROWS_SHOWN) {
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(visible(&commit.short));
                    ui.weak(format!("{} {}", date(commit.time), visible(&commit.author)));
                    ui.label(visible(&commit.summary));
                });
            }
            more_rows(ui, listed);
        });
}

fn lines(count: u32) -> String {
    match count {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    }
}

#[cfg(test)]
#[path = "history_changes_tests.rs"]
mod tests;
