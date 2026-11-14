//! Tests for arranging a compared commit range.

use super::*;
use crate::analysis::{Finding, Severity};
use crate::history::changes::{ChangedFile, Changes, Hunk};
use crate::index::graph_of_files;

const CODE: &str =
    "int first(void) {\n  return 1;\n}\n\nint second(int x) {\n  int y = x;\n  return y;\n}\n";

fn hunk(first_line: u32, line_count: u32, removed: u32) -> Hunk {
    Hunk {
        first_line,
        line_count,
        removed,
    }
}

fn view() -> RangeView {
    let graph = graph_of_files(&[("a.c", CODE), ("b.c", "int other(void) { return 0; }\n")]);
    let changes = Changes {
        base: "v1".into(),
        base_short: "1a2b3c4d".into(),
        head: "5e6f7a8b".into(),
        files: vec![ChangedFile {
            file: "a.c".into(),
            renamed_from: None,
            is_new: false,
            // Line 6 changed; a line removed before line 8.
            hunks: vec![hunk(6, 1, 1), hunk(8, 0, 1)],
            added: 1,
            removed: 2,
        }],
        ..Changes::default()
    };
    RangeView::new(changes, &graph)
}

fn finding(file: &str, line: u32) -> Finding {
    Finding {
        tool: "SrcWranglr".into(),
        rule: "strcpy".into(),
        severity: Severity::High,
        message: String::new(),
        fix: None,
        cwe: None,
        file: file.into(),
        line,
        columns: None,
        symbol: None,
    }
}

#[test]
fn only_functions_with_changed_lines_are_listed() {
    let view = view();
    let names: Vec<&str> = view.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["second"]);
    // Line 6, and the removal before its closing brace.
    assert_eq!(view.functions[0].changed_lines, 2);
    assert_eq!(
        (view.functions[0].file.as_str(), view.functions[0].line),
        ("a.c", 5)
    );
}

#[test]
fn changed_lines_and_removals_are_told_apart() {
    let view = view();
    assert!(view.is_changed_line("a.c", 6));
    assert!(!view.is_changed_line("a.c", 7));
    assert!(!view.is_changed_line("a.c", 8), "only removed before it");
    let removals = view.file_changes("a.c").expect("changes").removals;
    assert_eq!(removals.len(), 1, "a removal before line 8");
    assert_eq!(removals[0].line, 8);
    assert!(!view.is_changed_line("b.c", 1));
}

#[test]
fn changed_code_is_a_changed_line_or_anything_in_a_changed_function() {
    let view = view();
    assert!(view.is_changed_code("a.c", 7), "in second()");
    assert!(!view.is_changed_code("a.c", 2), "first() didn't change");
    assert!(!view.is_changed_code("b.c", 1));
}

#[test]
fn findings_are_counted_per_changed_function() {
    let view = view();
    let findings = [
        finding("a.c", 7),
        finding("a.c", 2),
        finding("a.c", 6),
        finding("b.c", 1),
    ];
    assert_eq!(view.findings_in_functions(&findings), [2]);
}

#[test]
fn it_is_labelled_by_its_ends() {
    assert_eq!(view().label(), "v1 (1a2b3c4d) .. 5e6f7a8b");
}

fn view_with(hunks: Vec<Hunk>) -> RangeView {
    let graph = graph_of_files(&[("a.c", CODE)]);
    let changes = Changes {
        base: "v1".into(),
        files: vec![ChangedFile {
            file: "a.c".into(),
            renamed_from: None,
            is_new: false,
            hunks,
            added: 0,
            removed: 0,
        }],
        ..Changes::default()
    };
    RangeView::new(changes, &graph)
}

#[test]
fn the_source_gets_runs_of_changed_lines_and_removals() {
    use crate::code_view::{LineChange, Removal};
    let view = view();
    let changes = view.file_changes("a.c").expect("changes");
    assert_eq!(changes.runs, [(6, 6)]);
    assert_eq!(
        changes.removals,
        [Removal {
            line: 8,
            count: 1,
            is_after: false
        }]
    );
    assert_eq!(changes.at(6), Some(LineChange::Changed { base: "v1" }));
    assert!(view.file_changes("b.c").is_none());
}

#[test]
fn lines_removed_at_the_end_are_marked_on_the_last_line() {
    use crate::code_view::Removal;
    // CODE has 8 lines; lines removed after them come back as line 9.
    let view = view_with(vec![hunk(9, 0, 3)]);
    let changes = view.file_changes("a.c").expect("changes");
    assert_eq!(
        changes.removals,
        [Removal {
            line: 8,
            count: 3,
            is_after: true
        }]
    );
}

#[test]
fn a_removal_just_above_a_function_isnt_in_it() {
    // Removed just before line 5, second()'s first: the gap above it.
    let view = view_with(vec![hunk(5, 0, 2)]);
    assert!(view.functions.is_empty());
}

#[test]
fn hunks_from_a_bad_answer_are_kept_inside_the_file() {
    let view = view_with(vec![hunk(6, u32::MAX, u32::MAX), hunk(u32::MAX, 0, 1)]);
    let changes = view.file_changes("a.c").expect("changes");
    assert_eq!(changes.runs, [(6, 8)]);
    assert_eq!(changes.removals.len(), 1);
    assert!(changes.removals[0].line <= 8);
    // Lines 6 to 8, and the removal after its last line (the file's end).
    assert_eq!(view.functions[0].changed_lines, 4);
}
