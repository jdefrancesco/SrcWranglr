//! The project's git history for the History pane: read in the background
//! when a project opens, then arranged per fix commit with the places in
//! today's code it last changed.

use std::collections::HashSet;
use std::path::PathBuf;

pub use super::background::BackgroundJob;
use super::visible::visible;
use crate::history::worker::{file_history_in_worker, read_in_worker};
use crate::history::{FileHistory, History, OwnedLines, Strength};
use crate::index::functions_holding_lines;
use crate::model::{CodeGraph, SymbolId};

/// Reads a project's history.
pub type HistoryJob = BackgroundJob<History>;

/// Reads the history of the checkout at `root` for its `files`.
pub fn read_history(root: PathBuf, files: HashSet<String>, ctx: egui::Context) -> HistoryJob {
    BackgroundJob::spawn(
        "Reading the git history",
        move |stop| read_in_worker(&root, files.into_iter().collect(), stop),
        ctx,
    )
}

/// Reads who last changed each line of `file` in the checkout at `root`.
pub fn read_file_history(
    root: PathBuf,
    file: String,
    ctx: egui::Context,
) -> BackgroundJob<FileHistory> {
    BackgroundJob::spawn(
        "Reading the file's history",
        move |stop| file_history_in_worker(&root, &file, stop),
        ctx,
    )
}

/// One fix commit as the pane lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    /// Index into `History::fix_commits`.
    pub commit: usize,
    /// Indices into `History::owned`, in file and line order.
    pub places: Vec<usize>,
    /// How many lines of today's code it last changed.
    pub lines: u32,
    /// Summary, keywords and id as shown, lowercased, for the filter.
    search: String,
}

/// The history arranged for the pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryView {
    pub history: History,
    /// Newest first, as the commits are.
    pub rows: Vec<CommitRow>,
    /// The function holding each run of owned lines (same order).
    pub functions: Vec<Option<SymbolId>>,
}

impl HistoryView {
    pub fn new(history: History, graph: &CodeGraph) -> Self {
        let places: Vec<(&str, u32)> = history
            .owned
            .iter()
            .map(|o| (o.file.as_str(), o.first_line))
            .collect();
        let functions = functions_holding_lines(graph, &places);
        let mut rows: Vec<CommitRow> = history
            .fix_commits
            .iter()
            .enumerate()
            .map(|(commit, c)| CommitRow {
                commit,
                places: Vec::new(),
                lines: 0,
                search: visible(&format!(
                    "{} {} {}",
                    c.summary,
                    c.keywords.join(" "),
                    c.short
                ))
                .to_lowercase(),
            })
            .collect();
        for (index, owned) in history.owned.iter().enumerate() {
            if let Some(row) = rows.get_mut(owned.commit) {
                row.places.push(index);
                row.lines = row.lines.saturating_add(owned.line_count);
            }
        }
        Self {
            history,
            rows,
            functions,
        }
    }

    /// Which rows (by index) match `query` (ignoring case), are security
    /// fixes if `security_only`, and still own lines if `owning_only`.
    pub fn listed(&self, query: &str, security_only: bool, owning_only: bool) -> Vec<usize> {
        let needle = query.trim().to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !owning_only || row.lines > 0)
            .filter(|(_, row)| {
                !security_only
                    || self.history.fix_commits[row.commit].strength == Strength::Security
            })
            .filter(|(_, row)| needle.is_empty() || row.search.contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn owned(&self, index: usize) -> Option<&OwnedLines> {
        self.history.owned.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FixCommit;
    use crate::index::graph_of_files;

    fn commit(summary: &str, strength: Strength) -> FixCommit {
        FixCommit {
            id: "0123456789".into(),
            short: "01234567".into(),
            summary: summary.into(),
            author: "Tess".into(),
            time: 0,
            keywords: vec!["fix".into()],
            strength,
        }
    }

    fn view() -> HistoryView {
        let graph = graph_of_files(&[("a.c", "int parse(void) {\n  return 0;\n}\n\nint other;\n")]);
        let history = History {
            fix_commits: vec![
                commit("Fix overflow", Strength::Security),
                commit("fix typo", Strength::Fix),
                commit("Fix build", Strength::Fix),
            ],
            owned: vec![
                OwnedLines {
                    file: "a.c".into(),
                    first_line: 2,
                    line_count: 1,
                    commit: 0,
                },
                OwnedLines {
                    file: "a.c".into(),
                    first_line: 5,
                    line_count: 1,
                    commit: 1,
                },
            ],
            ..History::default()
        };
        HistoryView::new(history, &graph)
    }

    #[test]
    fn rows_count_the_lines_each_commit_owns_and_where() {
        let view = view();
        assert_eq!(view.rows[0].lines, 1);
        assert_eq!(view.rows[0].places, [0]);
        assert_eq!(view.rows[2].lines, 0);
        assert!(view.functions[0].is_some(), "line 2 is in parse");
        assert_eq!(view.functions[1], None, "line 5 is at file scope");
    }

    #[test]
    fn the_list_filters_by_text_strength_and_owned_lines() {
        let view = view();
        assert_eq!(view.listed("", false, true), [0, 1]);
        assert_eq!(view.listed("", false, false), [0, 1, 2]);
        assert_eq!(view.listed("", true, true), [0]);
        assert_eq!(view.listed("  TYPO ", false, true), [1]);
        assert_eq!(view.listed("01234567", false, false).len(), 3);
    }
}
