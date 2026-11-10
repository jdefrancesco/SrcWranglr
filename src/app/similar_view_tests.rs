use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::analysis::{Finding, Severity};
use crate::app::rule_editor::SaveTarget;
use crate::app::rule_files::RuleHome;
use crate::index::graph_of_files;

struct State {
    view: SimilarView,
    graph: CodeGraph,
    actions: Vec<EditorAction>,
}

fn harness_on(source: &str, places: Vec<Example>) -> Harness<'static, State> {
    let graph = graph_of_files(&[("src/net.c", source)]);
    let view = SimilarView::open_on(&graph, places);
    let mut harness = crate::app::fonts::test_harness().build_ui_state(
        |ui, state: &mut State| {
            let ctx = ui.ctx().clone();
            if let Some(Some(action)) = state.view.show(&ctx, &state.graph) {
                state.actions.push(action);
            }
        },
        State {
            view,
            graph,
            actions: Vec::new(),
        },
    );
    harness.run();
    harness
}

fn harness(source: &str, line: u32) -> Harness<'static, State> {
    harness_on(source, vec![Example::line("src/net.c", line)])
}

const SOURCE: &str = "void f(char *b, char *p, int n) {\n  memcpy(b, p, ntohs(n));\n}\n";

const CHECKED: &str = "\
void f(char *b, char *p, int n) {
  if (n > sizeof(b)) return;
  memcpy(b, p, n);
}
void g(char *d, char *s, int len) {
  memcpy(d, s, len);
}
";

#[test]
fn drafts_a_rule_from_the_line_and_loosens_it_on_request() {
    let mut harness = harness(SOURCE, 2);
    harness.get_by_label("From src/net.c:2");
    assert!(harness
        .state()
        .view
        .editor
        .yaml
        .contains("memcpy($A, $B, ntohs($C))"));
    harness.get_by_label("Any values").click();
    harness.run();
    assert!(harness
        .state()
        .view
        .editor
        .yaml
        .contains("memcpy($A, $B, ntohs(...))"));
}

#[test]
fn test_and_save_hand_over_the_rule_as_edited() {
    let mut harness = harness(SOURCE, 2);
    let edited = harness
        .state()
        .view
        .editor
        .yaml
        .replace("like-memcpy-2", "my-rule");
    harness.state_mut().view.editor.yaml = edited;
    harness.run();
    harness.get_by_label("Test").click();
    harness.run();
    harness.get_by_label("Save for This Project").click();
    harness.run();
    let actions = &harness.state().actions;
    assert!(matches!(&actions[0], EditorAction::Test(yaml) if yaml.contains("my-rule")));
    assert!(matches!(
        &actions[1],
        EditorAction::Check(SaveTarget::New(RuleHome::Project, id), yaml)
            if id == "like-memcpy-2" && yaml.contains("my-rule")
    ));
}

#[test]
fn matches_are_listed_and_picking_one_goes_there() {
    let mut harness = harness(SOURCE, 2);
    let found = Finding {
        tool: "semgrep".into(),
        rule: "like-memcpy-2".into(),
        severity: Severity::Medium,
        message: String::new(),
        fix: None,
        cwe: None,
        file: "src/net.c".into(),
        line: 2,
        columns: Some((2, 24)),
        symbol: None,
    };
    harness
        .state_mut()
        .view
        .editor
        .set_matches(Ok(vec![found.clone()]));
    harness.run();
    harness.get_by_label("1 match:");
    harness
        .get_by_label("src/net.c:2  memcpy(b, p, ntohs(n));")
        .click();
    harness.run();
    assert_eq!(harness.state().actions, [EditorAction::GoTo(found)]);
}

#[test]
fn a_line_without_code_says_so() {
    let harness = harness("int x;\n\n", 2);
    harness.get_by_label("No call or statement starts on this line.");
    assert!(harness.query_by_label("Test").is_none());
}

#[test]
fn another_place_drafts_what_both_share() {
    let mut harness = harness(CHECKED, 3);
    let state = harness.state_mut();
    state
        .view
        .add_place(&state.graph, Example::line("src/net.c", 6));
    harness.run();
    harness.get_by_label("From 2 places:");
    assert!(harness
        .state()
        .view
        .editor
        .yaml
        .contains("pattern: \"memcpy($A, $B, $C)\""));
    // The same place twice says so.
    let state = harness.state_mut();
    state
        .view
        .add_place(&state.graph, Example::line("src/net.c", 6));
    harness.run();
    harness.get_by_label("src/net.c:6 is already a place of this rule.");
}

#[test]
fn a_check_becomes_an_exception_with_the_rules_metavariables() {
    let mut harness = harness(CHECKED, 3);
    let state = harness.state_mut();
    state.view.add_exception_from(&state.graph, "src/net.c", 2);
    harness.run();
    harness.get_by_label("Except:");
    let yaml = &harness.state().view.editor.yaml;
    assert!(yaml.contains("      - pattern-not-inside: |\n"), "{yaml}");
    assert!(
        yaml.contains("if (<... $C > sizeof($A) ...>) ..."),
        "{yaml}"
    );
}

#[test]
fn several_lines_can_be_kept_together_or_spread_out() {
    let mut harness = harness_on(CHECKED, vec![Example::lines("src/net.c", 2, 3)]);
    harness.get_by_label("From src/net.c:2-3");
    assert!(harness.state().view.editor.yaml.contains("\n      ...\n"));
    harness
        .get_by_label("Other code may come between the statements")
        .click();
    harness.run();
    assert!(!harness.state().view.editor.yaml.contains("\n      ...\n"));
}
