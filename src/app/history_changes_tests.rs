use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::history::test_repo::ranged;
use crate::index::graph_of_files;

struct State {
    tab: ChangesTab,
    graph: CodeGraph,
    root: Option<std::path::PathBuf>,
    actions: Vec<HistoryPanelAction>,
    /// Whether the tab is hidden (another one shown): only polled.
    is_hidden: bool,
}

/// The repository's files as checked out, indexed.
fn graph_of(root: &Path) -> CodeGraph {
    let files: Vec<(String, String)> = ["a.c", "d.c", "e.c", "f.c"]
        .iter()
        .map(|f| {
            let text = std::fs::read_to_string(root.join(f)).expect("read");
            (f.to_string(), text)
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();
    graph_of_files(&refs)
}

fn harness(root: Option<&Path>) -> Harness<'static, State> {
    let graph = root.map_or_else(|| graph_of_files(&[("a.c", "int a;\n")]), graph_of);
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(700.0, 700.0))
        .build_ui_state(
            |ui, state: &mut State| {
                if state.is_hidden {
                    state.tab.poll_jobs(&state.graph);
                    return;
                }
                if let Some(action) = state.tab.show(ui, &state.graph, state.root.as_deref(), &[]) {
                    state.actions.push(action);
                }
            },
            State {
                tab: ChangesTab::default(),
                graph,
                root: root.map(Path::to_path_buf),
                actions: Vec::new(),
                is_hidden: false,
            },
        );
    harness.run();
    harness
}

/// Steps until `done`, for background reads.
fn step_until(
    harness: &mut Harness<'static, State>,
    done: impl Fn(&Harness<'static, State>) -> bool,
) {
    for _ in 0..500 {
        harness.step();
        if done(harness) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never done");
}

fn compare(harness: &mut Harness<'static, State>, base: &str) {
    harness.get_by_role(egui::Role::TextInput).focus();
    harness.run();
    harness.get_by_role(egui::Role::TextInput).type_text(base);
    harness.run();
    harness.get_by_label("Compare").click();
    // The spinner keeps it drawing until the comparison is done.
    harness.step();
}

#[test]
fn comparing_lists_what_changed_and_goes_to_it() {
    let Some(repo) = ranged() else {
        return;
    };
    let mut harness = harness(Some(repo.path()));
    compare(&mut harness, "v1");
    step_until(&mut harness, |h| h.state().tab.range().is_some());
    harness.run();
    harness.get_by_label_contains("5 commits");
    harness.get_by_label_contains("4 files changed");
    // a() changed: listed, and a click goes there.
    harness.get_by_label("a()").click();
    harness.run();
    assert_eq!(
        harness.state().actions.last(),
        Some(&HistoryPanelAction::GoTo {
            symbol: harness.state().tab.range().map(|r| r.functions[0].symbol),
            file: "a.c".into(),
            line: 1,
        })
    );
}

#[test]
fn a_base_that_names_nothing_says_so() {
    let Some(repo) = ranged() else {
        return;
    };
    let mut harness = harness(Some(repo.path()));
    compare(&mut harness, "nope");
    step_until(&mut harness, |h| {
        h.query_by_label_contains("Nothing here is named").is_some()
    });
    assert!(harness.state().tab.range().is_none());
}

#[test]
fn tags_and_branches_are_offered_as_bases() {
    let Some(repo) = ranged() else {
        return;
    };
    let mut harness = harness(Some(repo.path()));
    step_until(&mut harness, |h| {
        h.query_by_role(egui::Role::ComboBox).is_some()
    });
    harness.get_by_role(egui::Role::ComboBox).click();
    harness.run();
    harness.get_by_label("v1a").click();
    harness.step();
    step_until(&mut harness, |h| h.state().tab.range().is_some());
    assert_eq!(
        harness.state().tab.range().map(|r| r.changes.base.as_str()),
        Some("v1a")
    );
}

#[test]
fn without_a_checkout_there_is_nothing_to_compare() {
    let harness = harness(None);
    harness.get_by_label_contains("no git history");
}

#[test]
fn clearing_forgets_the_range() {
    let Some(repo) = ranged() else {
        return;
    };
    let mut harness = harness(Some(repo.path()));
    compare(&mut harness, "v1");
    step_until(&mut harness, |h| h.state().tab.range().is_some());
    harness.run();
    harness.get_by_label("Clear").click();
    harness.run();
    assert!(harness.state().tab.range().is_none());
}

#[test]
fn a_comparison_finishes_while_the_tab_is_hidden() {
    let Some(repo) = ranged() else {
        return;
    };
    let mut harness = harness(Some(repo.path()));
    compare(&mut harness, "v1");
    harness.state_mut().is_hidden = true;
    step_until(&mut harness, |h| h.state().tab.range().is_some());
}
