//! The Matching pane: look-alike groups, code like one function, and
//! functions by behavior.

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::index::graph_of_files;

const SOURCE: &str =
    "int sum_a(int *p, int n) { int t = 0; for (int i = 0; i < n; i++) { t += p[i] * \
    2; if (t > 9) { t = t - 9; } } return t + n; }\n\
int sum_b(int *q, int m) { int s = 0; for (int j = 0; j < m; j++) { s += q[j] * 5; if (s > 7) \
    { s = s - 4; } } return s + m; }\n\
void serve(int fd, char *out) { char b[8]; int n = recv(fd, b, 8, 0); memcpy(out, b, n); }\n";

struct State {
    panel: MatchingPanel,
    graph: Arc<CodeGraph>,
    picked: Option<MatchingAction>,
}

fn harness() -> Harness<'static, State> {
    let state = State {
        panel: MatchingPanel {
            is_open: true,
            ..MatchingPanel::default()
        },
        graph: Arc::new(graph_of_files(&[("a.c", SOURCE)])),
        picked: None,
    };
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui_state(
            |ui, state: &mut State| {
                state.panel.poll(ui.ctx());
                let graph = Arc::clone(&state.graph);
                if let Some(action) = state.panel.show(ui, &graph, Layout::default()) {
                    state.picked = Some(action);
                }
            },
            state,
        );
    harness.run();
    harness
}

/// Steps the harness until `label` shows (shapes are read in the
/// background).
fn wait_for(harness: &mut Harness<'static, State>, label: &str) {
    for _ in 0..600 {
        harness.step();
        if harness.query_by_label_contains(label).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{label} never showed");
}

fn id(graph: &CodeGraph, name: &str) -> SymbolId {
    graph
        .symbols
        .values()
        .find(|s| s.name == name)
        .map(|s| s.id)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn finds_look_alike_functions_and_goes_to_one() {
    let mut harness = harness();
    harness.get_by_label("Find Look-alike Code").click();
    wait_for(&mut harness, "1 groups of look-alike functions");
    harness.get_by_label_contains("2 functions").click();
    harness.run();
    harness.get_by_label_contains("a.c:2").click();
    harness.run();
    let sum_b = id(&harness.state().graph, "sum_b");
    assert_eq!(
        harness.state().picked,
        Some(MatchingAction::GoTo {
            symbol: Some(sum_b),
            file: "a.c".into(),
            line: 2,
        })
    );
}

#[test]
fn lists_the_code_like_a_function_once_read() {
    let mut harness = harness();
    let (graph, ctx) = (Arc::clone(&harness.state().graph), harness.ctx.clone());
    let sum_a = id(&graph, "sum_a");
    harness.state_mut().panel.find_like(sum_a, &graph, &ctx);
    wait_for(&mut harness, "100%");
    harness.get_by_label_contains("Code like sum_a");
    harness.get_by_label_contains("100%  sum_b  a.c:2");
    harness.get_by_label("Clear").click();
    harness.run();
    assert!(harness.query_by_label_contains("Code like").is_none());
}

#[test]
fn lists_functions_with_every_behavior_picked() {
    let mut harness = harness();
    harness.get_by_label("By behavior").click();
    harness.run();
    harness.get_by_label_contains("Pick behaviors above");
    harness.get_by_label("reads input (1)").click();
    harness.run();
    harness.get_by_label("copies memory (1)").click();
    harness.run();
    harness.get_by_label_contains("1 function with every one of: reads input, copies memory");
    harness.get_by_label_contains("serve").click();
    harness.run();
    let serve = id(&harness.state().graph, "serve");
    assert!(matches!(
        harness.state().picked,
        Some(MatchingAction::GoTo { symbol: Some(s), .. }) if s == serve
    ));
}
