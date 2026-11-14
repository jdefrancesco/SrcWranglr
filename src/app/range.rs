//! A compared commit range, arranged for the app: which lines of each file
//! changed (in the checkout's numbering, as indexed), where lines were
//! removed, and which functions changed. The History pane's Changes tab
//! lists it, the source marks it, and Findings can show only what's in it.
//!
//! The changes come from a worker process reading a hostile repository, so
//! every hunk is kept inside its file's lines here, and lookups are binary
//! searches over sorted runs: the Findings pane asks about every finding
//! each frame.

use std::collections::HashMap;

use crate::analysis::Finding;
use crate::code_view::changes::contains;
use crate::code_view::{FileChanges, Removal};
use crate::history::changes::{Changes, Hunk};
use crate::model::{CodeGraph, FunctionMetrics, SymbolId};

/// A function the range changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFunction {
    pub symbol: SymbolId,
    pub name: String,
    pub file: String,
    /// Where its definition starts, and its last line.
    pub line: u32,
    pub last_line: u32,
    /// Its lines that are new or changed (a removal inside counts as one).
    pub changed_lines: u32,
}

/// One file's changes, kept inside its lines.
#[derive(Debug, Default)]
struct FileMarks {
    /// New or changed lines, as sorted, merged runs.
    runs: Vec<(u32, u32)>,
    /// Sorted by line.
    removals: Vec<Removal>,
}

pub struct RangeView {
    pub changes: Changes,
    files: HashMap<String, FileMarks>,
    /// Most changed first.
    pub functions: Vec<ChangedFunction>,
    /// The changed functions' line spans, by file, as sorted, merged runs.
    function_spans: HashMap<String, Vec<(u32, u32)>>,
}

impl RangeView {
    pub fn new(changes: Changes, graph: &CodeGraph) -> Self {
        let files: HashMap<String, FileMarks> = changes
            .files
            .iter()
            .filter_map(|f| {
                let lines = graph.file_contents.get(&f.file)?.lines().count();
                let lines = u32::try_from(lines).unwrap_or(u32::MAX);
                Some((f.file.clone(), file_marks(&f.hunks, lines)))
            })
            .collect();
        let functions = changed_functions(graph, &files);
        let mut function_spans: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        for function in &functions {
            function_spans
                .entry(function.file.clone())
                .or_default()
                .push((function.line, function.last_line));
        }
        let function_spans = function_spans
            .into_iter()
            .map(|(file, spans)| (file, merged(spans)))
            .collect();
        Self {
            changes,
            files,
            functions,
            function_spans,
        }
    }

    /// `v6.6 (1a2b3c4d) .. 5e6f7a8b`.
    pub fn label(&self) -> String {
        format!(
            "{} ({}) .. {}",
            self.changes.base, self.changes.base_short, self.changes.head
        )
    }

    /// What the range changed in `file`, for the source to mark.
    pub fn file_changes(&self, file: &str) -> Option<FileChanges<'_>> {
        let marks = self.files.get(file)?;
        Some(FileChanges {
            base: &self.changes.base,
            runs: &marks.runs,
            removals: &marks.removals,
        })
    }

    /// Whether `line` of `file` is new or changed in the range.
    pub fn is_changed_line(&self, file: &str, line: u32) -> bool {
        self.files
            .get(file)
            .is_some_and(|marks| contains(&marks.runs, line))
    }

    /// Whether `line` of `file` is changed code: a changed line, or any
    /// line of a function the range changed.
    pub fn is_changed_code(&self, file: &str, line: u32) -> bool {
        self.is_changed_line(file, line)
            || self
                .function_spans
                .get(file)
                .is_some_and(|spans| contains(spans, line))
    }

    /// How many of `findings` are in each changed function, in the order
    /// of `functions`.
    pub fn findings_in_functions(&self, findings: &[Finding]) -> Vec<usize> {
        let mut counts = vec![0; self.functions.len()];
        let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, function) in self.functions.iter().enumerate() {
            by_file.entry(function.file.as_str()).or_default().push(i);
        }
        for finding in findings {
            let Some(indices) = by_file.get(finding.file.as_str()) else {
                continue;
            };
            for &i in indices {
                let function = &self.functions[i];
                if (function.line..=function.last_line).contains(&finding.line) {
                    counts[i] += 1;
                }
            }
        }
        counts
    }
}

/// A file's `hunks`, kept inside its `lines`: changed lines as merged
/// runs, removals at the line now after them (or after the last line).
fn file_marks(hunks: &[Hunk], lines: u32) -> FileMarks {
    let mut runs = Vec::new();
    let mut removals = Vec::new();
    for hunk in hunks {
        let first = hunk.first_line.max(1);
        if hunk.line_count == 0 {
            if lines > 0 {
                removals.push(Removal {
                    line: first.min(lines),
                    count: hunk.removed,
                    is_after: first > lines,
                });
            }
            continue;
        }
        let last = first.saturating_add(hunk.line_count - 1).min(lines);
        if first <= last {
            runs.push((first, last));
        }
    }
    removals.sort_by_key(|r| (r.line, r.is_after));
    removals.dedup_by_key(|r| (r.line, r.is_after));
    FileMarks {
        runs: merged(runs),
        removals,
    }
}

/// `runs` sorted, with overlapping and touching ones joined.
fn merged(mut runs: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    runs.sort_unstable();
    let mut joined: Vec<(u32, u32)> = Vec::with_capacity(runs.len());
    for (first, last) in runs {
        match joined.last_mut() {
            Some(previous) if first <= previous.1.saturating_add(1) => {
                previous.1 = previous.1.max(last);
            }
            _ => joined.push((first, last)),
        }
    }
    joined
}

/// The functions with changed lines (or a removal inside), most changed
/// first.
fn changed_functions(
    graph: &CodeGraph,
    files: &HashMap<String, FileMarks>,
) -> Vec<ChangedFunction> {
    let mut functions: Vec<ChangedFunction> = graph
        .functions
        .iter()
        .filter_map(|function| {
            let marks = files.get(&function.file)?;
            changed_function(graph, function, marks)
        })
        .collect();
    functions.sort_by(|a, b| {
        b.changed_lines
            .cmp(&a.changed_lines)
            .then_with(|| (&a.file, a.line).cmp(&(&b.file, b.line)))
    });
    functions
}

fn changed_function(
    graph: &CodeGraph,
    function: &FunctionMetrics,
    marks: &FileMarks,
) -> Option<ChangedFunction> {
    let first = function.line;
    let last = first.saturating_add(function.lines.saturating_sub(1));
    let changed = marks
        .runs
        .iter()
        .map(|(start, end)| {
            let (start, end) = ((*start).max(first), (*end).min(last));
            if start <= end {
                end - start + 1
            } else {
                0
            }
        })
        .fold(0u32, u32::saturating_add);
    // A removal inside: before one of its lines other than its first (a
    // removal just above it is in the gap before it), or after its last
    // line at the end of the file.
    let removals = marks
        .removals
        .iter()
        .filter(|r| match r.is_after {
            true => r.line == last,
            false => r.line > first && r.line <= last,
        })
        .count();
    let changed_lines = changed.saturating_add(u32::try_from(removals).unwrap_or(u32::MAX));
    (changed_lines > 0).then(|| ChangedFunction {
        symbol: function.symbol,
        name: graph
            .symbols
            .get(&function.symbol)
            .map_or_else(String::new, |s| s.qualified_name.clone()),
        file: function.file.clone(),
        line: first,
        last_line: last,
        changed_lines,
    })
}

#[cfg(test)]
#[path = "range_tests.rs"]
mod tests;
