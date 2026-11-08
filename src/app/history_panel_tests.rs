use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::history::{FixCommit, OwnedLines};
use crate::index::graph_of_files;

struct State {
    panel: HistoryPanel,
    graph: CodeGraph,
    action: Option<HistoryPanelAction>,
}

fn commit(short: &str, summary: &str, strength: Strength) -> FixCommit {
    FixCommit {
        id: format!("{short}ffff"),
        short: short.into(),
        summary: summary.into(),
        author: "Tess".into(),
        time: 1_700_000_000,
        keywords: vec!["fix".into()],
        strength,
    }
}

fn sample() -> History {
    History {
        head: "cafef00d".into(),
        commits_read: 40,
        fix_commits: vec![
            commit("aaaa1111", "Fix overflow in parse", Strength::Security),
            commit("bbbb2222", "fix typo \u{202E}", Strength::Fix),
            commit("cccc3333", "Fix build", Strength::Fix),
        ],
        owned: vec![
            OwnedLines {
                file: "net.c".into(),
                first_line: 2,
                line_count: 2,
                commit: 0,
            },
            OwnedLines {
                file: "net.c".into(),
                first_line: 6,
                line_count: 1,
                commit: 1,
            },
        ],
        files_traced: 1,
        ..History::default()
    }
}

fn harness_with(history: Option<History>) -> Harness<'static, State> {
    let graph = graph_of_files(&[(
        "net.c",
        "int parse(char *b) {\n  char t[8];\n  strcpy(t, b);\n  return 0;\n}\nint typo;\n",
    )]);
    let mut panel = HistoryPanel::default();
    panel.set_open(true);
    if let Some(history) = history {
        panel.set_history(history, &graph);
    }
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui_state(
            |ui, state: &mut State| {
                let shown = state.panel.show(ui, &state.graph, Layout::default(), None);
                if let Some(action) = shown {
                    state.action = Some(action);
                }
            },
            State {
                panel,
                graph,
                action: None,
            },
        );
    harness.run();
    harness
}

#[test]
fn lists_the_fix_commits_still_in_todays_code() {
    let harness = harness_with(Some(sample()));
    harness.get_by_label("Git History (2)");
    harness.get_by_label("2023-11-14  aaaa1111  Fix overflow in parse  (2 lines)");
    // Hidden characters in a commit message are marked.
    harness.get_by_label("2023-11-14  bbbb2222  fix typo ‹U+202E›  (1 line)");
    assert!(harness.query_by_label_contains("Fix build").is_none());
    harness.get_by_label("3 fix commits in all 40 commits from cafef00d (the checked-out commit).");
}

#[test]
fn picking_a_commit_lists_its_lines_and_a_line_goes_there() {
    let mut harness = harness_with(Some(sample()));
    harness
        .get_by_label("2023-11-14  aaaa1111  Fix overflow in parse  (2 lines)")
        .click();
    harness.run();
    harness.get_by_label("Lines of today's code it last changed:");
    harness.get_by_label("net.c:2–3  parse").click();
    harness.run();
    let Some(HistoryPanelAction::GoTo { symbol, file, line }) = harness.state().action.clone()
    else {
        panic!("no place picked");
    };
    assert_eq!((file.as_str(), line), ("net.c", 2));
    assert!(symbol.is_some());
}

#[test]
fn filters_show_commits_with_no_lines_left_and_security_ones() {
    let mut harness = harness_with(Some(sample()));
    harness.get_by_label("Only ones in today's code").click();
    harness.run();
    harness.get_by_label("2023-11-14  cccc3333  Fix build  (none left)");
    harness.get_by_label("Security words only").click();
    harness.run();
    assert!(harness.query_by_label_contains("Fix build").is_none());
    harness.get_by_label_contains("Fix overflow in parse");
}

#[test]
fn says_why_there_is_no_history() {
    let mut harness = harness_with(None);
    harness.get_by_label("Open a project to read its git history.");
    harness
        .state_mut()
        .panel
        .set_unavailable("The demo project has no git history.");
    harness.run();
    harness.get_by_label("The demo project has no git history.");
}

#[test]
fn the_selected_function_tab_lists_who_last_changed_its_lines() {
    let Some(repo) = crate::history::test_repo::sample() else {
        return;
    };
    let a = std::fs::read_to_string(repo.path().join("a.c")).expect("a.c");
    let graph = graph_of_files(&[("a.c", &a)]);
    let parse = graph
        .symbols
        .values()
        .find(|s| s.name == "parse")
        .map(|s| s.id);
    let mut panel = HistoryPanel::default();
    panel.set_open(true);
    panel.root = Some(repo.path().to_path_buf());
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui_state(
            move |ui, state: &mut State| {
                let shown = state.panel.show(ui, &state.graph, Layout::default(), parse);
                if let Some(action) = shown {
                    state.action = Some(action);
                }
            },
            State {
                panel,
                graph,
                action: None,
            },
        );
    harness.run();
    harness.get_by_label("Selected function").click();
    for _ in 0..200 {
        // The spinner keeps asking for frames while the file is read.
        harness.step();
        if harness
            .query_by_label_contains("commit(s) last changed its")
            .is_some()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    harness.run();
    harness.get_by_label_contains("2 commit(s) last changed its 5 lines");
    harness
        .get_by_label_contains("Fix stack overflow in parse")
        .click();
    harness.run();
    let Some(HistoryPanelAction::GoTo { line, .. }) = harness.state().action.clone() else {
        panic!("no line picked");
    };
    assert_eq!(line, 3);
}

#[test]
fn the_selected_function_tab_asks_for_a_function() {
    let mut harness = harness_with(Some(sample()));
    harness.get_by_label("Selected function").click();
    harness.run();
    harness.get_by_label("Select a function to see who last changed its lines.");
}
