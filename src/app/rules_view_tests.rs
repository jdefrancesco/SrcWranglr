use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::analysis::wrappers::{Passes, Wrapper};
use crate::analysis::Severity;
use crate::app::rule_library::load_from;
use crate::index::graph_of_files;

struct State {
    view: RulesView,
    graph: CodeGraph,
    actions: Vec<RulesAction>,
}

struct Folders {
    yours: tempfile::TempDir,
    project: tempfile::TempDir,
}

const RULE: &str = "rules:\n  - id: mine\n    languages: [c]\n    pattern: gets($X)\n";
const HOSTILE: &str = "rules:\n  - id: theirs\u{202E}\n    pattern: x\n";

fn folders() -> Folders {
    let folders = Folders {
        yours: tempfile::tempdir().expect("dir"),
        project: tempfile::tempdir().expect("dir"),
    };
    std::fs::write(folders.yours.path().join("mine.yaml"), RULE).expect("write");
    std::fs::write(folders.project.path().join("theirs.yaml"), HOSTILE).expect("write");
    folders
}

fn harness(folders: &Folders, is_trusted: bool) -> Harness<'static, State> {
    let library = load_from(
        Some(folders.yours.path().to_path_buf()),
        Some(folders.project.path().to_path_buf()),
    );
    let view = RulesView::open(library, true, is_trusted);
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(1100.0, 800.0))
        .build_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(Some(action)) = state.view.show(&ctx, &state.graph) {
                    state.actions.push(action);
                }
            },
            State {
                view,
                graph: graph_of_files(&[("a.c", "int x;\n")]),
                actions: Vec::new(),
            },
        );
    harness.run();
    harness
}

#[test]
fn a_new_rule_starts_from_the_template_and_saves_as_a_new_file() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("New rule");
    assert_eq!(harness.state().view.editor.yaml, TEMPLATE);
    harness.get_by_label("Save for All Projects").click();
    harness.run();
    assert!(matches!(
        harness.state().actions.last(),
        Some(RulesAction::Editor(EditorAction::Check(SaveTarget::New(RuleHome::Yours, id), _)))
            if id == "my-rule"
    ));
}

#[test]
fn your_rule_is_edited_and_saved_over_its_file() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    assert_eq!(harness.state().view.editor.yaml, RULE);
    harness.get_by_label("Save").click();
    harness.run();
    let path = folders.yours.path().join("mine.yaml");
    assert!(matches!(
        harness.state().actions.last(),
        Some(RulesAction::Editor(EditorAction::Check(SaveTarget::Existing(RuleHome::Yours, p), _)))
            if *p == path
    ));
}

#[test]
fn srcwranglrs_own_rules_are_read_only_and_copied_under_a_new_id() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("srcwranglr.double-free").click();
    harness.run();
    assert!(harness.query_by_label("Save").is_none());
    harness.get_by_label("Copy to Yours").click();
    harness.run();
    let yaml = &harness.state().view.editor.yaml;
    assert!(
        yaml.starts_with("rules:\n  - id: my-double-free\n"),
        "{yaml}"
    );
    harness.get_by_label("Save for All Projects");
}

#[test]
fn an_untrusted_projects_rule_is_shown_escaped_and_can_be_trusted() {
    let folders = folders();
    let mut harness = harness(&folders, false);
    harness.get_by_label("Not used until you trust this project's files.");
    harness.get_by_label("theirs.yaml").click();
    harness.run();
    let shown = &harness.state().view.editor.yaml;
    assert!(!shown.contains('\u{202E}'), "{shown}");
    assert!(harness.query_by_label("Save").is_none());
    assert!(harness.query_by_label("Delete…").is_none());
    harness.get_by_label("Trust This Project's Files").click();
    harness.run();
    assert_eq!(harness.state().actions.last(), Some(&RulesAction::Trust));
}

#[test]
fn unsaved_changes_are_kept_unless_discarded() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    harness.state_mut().view.editor.yaml.push_str("# changed\n");
    harness.get_by_label("srcwranglr.double-free").click();
    harness.run();
    harness.get_by_label("This rule has changes you haven't saved.");
    assert!(harness.state().view.editor.yaml.ends_with("# changed\n"));
    harness.get_by_label("Discard Them").click();
    harness.run();
    assert!(harness
        .state()
        .view
        .editor
        .yaml
        .contains("srcwranglr.double-free"));
}

#[test]
fn deleting_asks_first() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    harness.get_by_label("Delete…").click();
    harness.run();
    assert!(harness.state().actions.is_empty());
    harness.get_by_label("Delete").click();
    harness.run();
    let path = folders.yours.path().join("mine.yaml");
    assert_eq!(
        harness.state().actions.last(),
        Some(&RulesAction::Delete(RuleHome::Yours, path))
    );
}

#[test]
fn wrappers_are_looked_for_and_their_rules_edited() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("Suggested for This Project").click();
    harness.run();
    assert_eq!(
        harness.state().actions.last(),
        Some(&RulesAction::FindWrappers)
    );
    harness.state_mut().view.wrappers.set_found(vec![Wrapper {
        name: "copy_name".into(),
        symbol: 1,
        file: "a.c".into(),
        line: 3,
        callee: "strcpy".into(),
        call_line: 3,
        passes: Passes::Arguments,
        passed: vec!["name".into()],
        callers: 2,
        severity: Severity::High,
        why: "Unbounded.".into(),
    }]);
    harness.run();
    harness.get_by_label("copy_name()");
    harness.get_by_label("Include its rule").click();
    harness.run();
    harness.get_by_label("Edit Rules for the 1 Picked").click();
    harness.run();
    let yaml = &harness.state().view.editor.yaml;
    assert!(yaml.contains("id: wraps-strcpy-copy-name"), "{yaml}");
    assert!(yaml.contains("pattern: \"copy_name(...)\""), "{yaml}");
    harness.get_by_label("Save for This Project");
}

#[test]
fn a_save_that_fails_keeps_the_edits_guarded() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    harness.state_mut().view.editor.yaml.push_str("# changed\n");
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();
    // The check (and the save) never comes back: picking another rule
    // still asks first.
    harness.get_by_label("srcwranglr.double-free").click();
    harness.run();
    harness.get_by_label("This rule has changes you haven't saved.");
}

#[test]
fn wrapper_rules_wait_for_edits_to_be_discarded() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    harness.state_mut().view.editor.yaml.push_str("# changed\n");
    harness
        .state_mut()
        .view
        .edit_new("rules: []\n".into(), "wrappers");
    harness.run();
    harness.get_by_label("This rule has changes you haven't saved.");
    assert!(harness.state().view.editor.yaml.ends_with("# changed\n"));
    harness.get_by_label("Discard Them").click();
    harness.run();
    assert_eq!(harness.state().view.editor.yaml, "rules: []\n");
}

#[test]
fn a_reload_keeps_edits_and_forgets_a_pick_waiting_on_them() {
    let folders = folders();
    let mut harness = harness(&folders, true);
    harness.get_by_label("mine.yaml").click();
    harness.run();
    harness.state_mut().view.editor.yaml.push_str("# changed\n");
    harness.get_by_label("srcwranglr.double-free").click();
    harness.run();
    let library = load_from(
        Some(folders.yours.path().to_path_buf()),
        Some(folders.project.path().to_path_buf()),
    );
    harness.state_mut().view.reload(library, true, None);
    harness.run();
    assert!(harness.query_by_label("Discard Them").is_none());
    assert!(harness.state().view.editor.yaml.ends_with("# changed\n"));
}
