use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::app::notebook::Notebook;
use crate::app::project::Project;
use crate::index::{build_graph, Extractor, SourceFile};
use crate::notes::NoteTarget;

const SOURCE: &str = "int helper() { return 1; }\n\
                      int first() { return helper(); }\n\
                      int second() { return helper() + helper(); }\n";

const HARNESS_HEIGHT: f32 = 400.0;
const SCROLL_ANIMATION_STEPS: usize = 30;

struct State {
    panel: CodePanel,
    graph: CodeGraph,
    selected: Option<SymbolId>,
    clicked: Option<SymbolId>,
    is_open_requested: bool,
    notebook: Notebook,
    findings: Findings,
    last_action: Option<PanelAction>,
}

fn graph_of(source: &str) -> CodeGraph {
    let facts = Extractor::new()
        .expect("grammar")
        .extract(source)
        .expect("parse");
    build_graph(vec![SourceFile {
        path: "a.cpp".into(),
        contents: source.into(),
        facts,
    }])
}

fn id(graph: &CodeGraph, name: &str) -> SymbolId {
    graph
        .symbols
        .values()
        .find(|s| s.qualified_name == name)
        .map(|s| s.id)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn harness(selected: &str) -> Harness<'static, State> {
    harness_for(SOURCE, selected)
}

fn harness_for(source: &str, selected: &str) -> Harness<'static, State> {
    let graph = graph_of(source);
    let notebook = Notebook::open(&Project::new("t", None, graph.clone()));
    let findings = Findings::for_graph(&graph, &Default::default());
    let state = State {
        panel: CodePanel::default(),
        selected: Some(id(&graph, selected)),
        graph,
        clicked: None,
        is_open_requested: false,
        findings,
        notebook,
        last_action: None,
    };
    let mut harness = crate::app::fonts::test_harness()
        .with_size(egui::vec2(800.0, HARNESS_HEIGHT))
        .build_ui_state(
            |ui, state: &mut State| {
                let action = state.panel.show(
                    ui,
                    &state.graph,
                    state.selected,
                    &state.notebook,
                    &state.findings,
                    crate::app::pane_bar::Edge::Right,
                );
                match &action {
                    Some(PanelAction::Explore(id)) => state.clicked = Some(*id),
                    Some(PanelAction::OpenInEditor) => state.is_open_requested = true,
                    _ => {}
                }
                if action.is_some() {
                    state.last_action = action;
                }
            },
            state,
        );
    harness.run();
    harness
}

#[test]
fn a_narrow_pane_wraps_its_header_instead_of_spilling_out_of_it() {
    const NARROW: f32 = 300.0;
    let mut harness = harness("helper");
    harness.set_size(egui::vec2(NARROW, HARNESS_HEIGHT));
    harness.run();
    let file = harness.get_by_label("a.cpp").rect();
    let note = harness.get_by_label("Note…").rect();
    let arrow = harness.get_by_label("▸").rect();
    assert!(file.left() >= 0.0, "spilled left: {file:?}");
    assert!(arrow.right() <= NARROW, "spilled right: {arrow:?}");
    assert!(note.top() > file.top(), "didn't wrap: {file:?} {note:?}");
    assert!(
        (arrow.center().y - file.center().y).abs() < file.height(),
        "{arrow:?}"
    );
}

#[test]
fn its_arrow_hides_the_pane() {
    let mut harness = harness("helper");
    harness.get_by_label("▸").click();
    harness.run();
    assert_eq!(harness.state().last_action, Some(PanelAction::Hide));
}

fn click_at(harness: &mut Harness<State>, pos: Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
}

#[test]
fn starts_at_the_definition_and_steps_through_call_sites() {
    let mut harness = harness("helper");
    harness.get_by_label("1 / 4");
    harness.get_by_label("declaration");

    harness.get_by_label("▶").click();
    harness.run();
    harness.get_by_label("2 / 4");
    harness.get_by_label("call in");

    harness.get_by_label("◀").click();
    harness.run();
    harness.get_by_label("◀").click();
    harness.run();
    harness.get_by_label("4 / 4");
}

#[test]
fn open_in_editor_button_asks_to_open() {
    let mut harness = harness("helper");
    harness.get_by_label("Open in Editor").click();
    harness.run();
    assert!(harness.state().is_open_requested);
    assert_eq!(harness.state().clicked, None);
}

#[test]
fn position_follows_the_current_location_in_text_columns() {
    let mut harness = harness_for(
        "int hélper() { return 1; }\nint f() { /* é */ return hélper(); }\n",
        "hélper",
    );
    let position = |h: &Harness<State>| {
        let state = h.state();
        state.panel.position(&state.graph, state.selected)
    };
    let expected = |line, column| {
        Some(FilePosition {
            file: "a.cpp".into(),
            line,
            column,
        })
    };
    // `hélper` starts at byte 4 on line 1: text column 5.
    assert_eq!(position(&harness), expected(1, 5));
    harness.get_by_label("▶").click();
    harness.run();
    // Line 2: the call starts at byte 26, but `é` is two bytes: column 26.
    assert_eq!(position(&harness), expected(2, 26));
}

fn line(number: u32) -> NoteTarget {
    NoteTarget::Line {
        file: "a.cpp".into(),
        line: number,
    }
}

#[test]
fn header_buttons_bookmark_and_annotate_the_symbol() {
    let mut harness = harness("helper");
    harness.get_by_label("☆").click();
    harness.run();
    let helper = NoteTarget::Symbol("helper".into());
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::ToggleBookmark(helper.clone()))
    );
    harness.get_by_label("Note…").click();
    harness.run();
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::EditNote(helper))
    );
}

#[test]
fn a_starred_symbol_shows_a_filled_star_and_its_note() {
    let mut harness = harness("helper");
    let project = Project::new("t", None, harness.state().graph.clone());
    let target = NoteTarget::Symbol("helper".into());
    harness
        .state_mut()
        .notebook
        .set(&project, target, "always returns 1", true)
        .expect("set");
    harness.run();
    harness.get_by_label("★");
    harness.get_by_label("always returns 1");
}

#[test]
fn right_clicking_a_line_offers_line_notes_and_bookmarks() {
    let mut harness = harness("helper");
    harness
        .get_by_label("int first() { return helper(); }")
        .click_secondary();
    harness.run();
    harness.get_by_label("Bookmark Line").click();
    harness.run();
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::ToggleBookmark(line(2)))
    );

    harness
        .get_by_label("int second() { return helper() + helper(); }")
        .click_secondary();
    harness.run();
    harness.get_by_label("Add Note…").click();
    harness.run();
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::EditNote(line(3)))
    );
}

#[test]
fn line_menu_offers_editing_and_unbookmarking_marked_lines() {
    let mut harness = harness("helper");
    let project = Project::new("t", None, harness.state().graph.clone());
    harness
        .state_mut()
        .notebook
        .set(&project, line(2), "caller", true)
        .expect("set");
    harness.run();
    harness
        .get_by_label("int first() { return helper(); }")
        .click_secondary();
    harness.run();
    harness.get_by_label("Edit Note…");
    harness.get_by_label("Remove Bookmark").click();
    harness.run();
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::ToggleBookmark(line(2)))
    );
}

#[test]
fn line_marks_combine_notes_and_the_worst_finding() {
    let source = "void f(char *d, const char *s) {\n  memcpy(d, s, 1); strcpy(d, s);\n}\n";
    let graph = graph_of(source);
    let project = Project::new("t", None, graph.clone());
    let mut notebook = Notebook::open(&project);
    notebook
        .set(&project, line(2), "checked?", true)
        .expect("set");
    notebook.set(&project, line(3), "end", false).expect("set");
    let findings = Findings::for_graph(&graph, &Default::default());
    let marks = line_marks_for("a.cpp", &notebook, &findings);
    let two = marks[&2];
    assert!(two.is_starred && two.note == "checked?");
    assert_eq!(two.warning.map(|w| w.title), Some("strcpy"));
    assert_eq!(marks[&3].warning, None);
    assert_eq!(marks.len(), 2);
}

#[test]
fn clicking_a_highlighted_risky_call_asks_for_its_explanation() {
    let source = "void f(char *d, const char *s) {\nstrcpy(d, s);\n}\n";
    let mut harness = harness_for(source, "f");
    let row = harness.get_by_label("strcpy(d, s);").rect();
    click_at(&mut harness, Pos2::new(row.left() + 10.0, row.center().y));
    let strcpy = harness
        .state()
        .findings
        .all()
        .iter()
        .position(|f| f.rule == "strcpy");
    assert!(strcpy.is_some());
    assert_eq!(
        harness.state().last_action,
        strcpy.map(PanelAction::ShowFinding)
    );
}

#[test]
fn line_menu_offers_to_mark_each_called_function_as_dangerous() {
    let source =
        "int helper() { return 1; }\nvoid f(char *d, char *s) {\nstrcpy(d, s); helper();\n}\n";
    let mut harness = harness_for(source, "f");
    harness
        .get_by_label("strcpy(d, s); helper();")
        .click_secondary();
    harness.run();
    harness.get_by_label("Mark as Dangerous").click();
    harness.run();
    harness.get_by_label("helper");
    harness.get_by_label("strcpy").click();
    harness.run();
    assert_eq!(
        harness.state().last_action,
        Some(PanelAction::MarkDangerous("strcpy".into()))
    );
}

#[test]
fn the_calling_symbol_link_explores_it() {
    let mut harness = harness("helper");
    harness.get_by_label("▶").click();
    harness.run();
    harness.get_by_label("first").click();
    harness.run();
    let first = id(&harness.state().graph, "first");
    assert_eq!(harness.state().clicked, Some(first));
}

#[test]
fn clicking_a_name_in_the_source_explores_it() {
    let mut harness = harness("second");
    let line = SOURCE.lines().nth(2).expect("third line");
    let rect = harness.get_by_label(line).rect();
    // Monospace: aim at the middle of the first `helper` on that line.
    let column = line.find("helper").expect("call") + 3;
    let char_width = rect.width() / line.chars().count() as f32;
    let pos = Pos2::new(
        rect.min.x + char_width * (column as f32 + 0.5),
        rect.center().y,
    );
    click_at(&mut harness, pos);
    let helper = id(&harness.state().graph, "helper");
    assert_eq!(harness.state().clicked, Some(helper));
}

#[test]
fn clicking_plain_code_does_nothing() {
    let mut harness = harness("second");
    let line = SOURCE.lines().nth(2).expect("third line");
    let rect = harness.get_by_label(line).rect();
    click_at(&mut harness, Pos2::new(rect.min.x + 2.0, rect.center().y)); // on `int`
    assert_eq!(harness.state().clicked, None);
}

#[test]
fn clicking_another_occurrence_of_the_selected_symbol_focuses_it() {
    let mut harness = harness("helper");
    let line = SOURCE.lines().nth(2).expect("third line");
    let rect = harness.get_by_label(line).rect();
    let column = line.rfind("helper").expect("second call") + 3;
    let char_width = rect.width() / line.chars().count() as f32;
    let pos = Pos2::new(
        rect.min.x + char_width * (column as f32 + 0.5),
        rect.center().y,
    );
    click_at(&mut harness, pos);
    assert_eq!(harness.state().clicked, None);
    harness.get_by_label("4 / 4");
}

#[test]
fn stepping_to_a_distant_call_scrolls_it_into_view() {
    let filler = "// filler\n".repeat(150);
    let source =
        format!("int helper() {{ return 1; }}\n{filler}int far() {{ return helper(); }}\n");
    let mut harness = harness_for(&source, "helper");
    assert!(harness
        .query_by_label("int far() { return helper(); }")
        .is_none());

    harness.get_by_label("▶").click();
    harness.run_steps(SCROLL_ANIMATION_STEPS); // scrolling is animated
    let rect = harness
        .get_by_label("int far() { return helper(); }")
        .rect();
    assert!(rect.max.y <= HARNESS_HEIGHT, "focused line at {rect:?}");
}

#[test]
fn stepping_to_a_visible_location_does_not_scroll() {
    let mut harness = harness("helper");
    let first_line = SOURCE.lines().next().expect("first line");
    let before = harness.get_by_label(first_line).rect();
    harness.get_by_label("▶").click();
    harness.run();
    assert_eq!(harness.get_by_label(first_line).rect(), before);
}

#[test]
fn a_traced_path_is_marked_in_the_file_shown_only() {
    let mut panel = CodePanel::default();
    let span = |line, start| Span {
        line,
        start,
        end: start + 3,
    };
    panel.set_trail(vec![
        ("a.c".into(), span(3, 4)),
        ("b.c".into(), span(9, 0)),
        ("a.c".into(), span(7, 2)),
    ]);
    assert_eq!(panel.trail_in("a.c"), [span(3, 4), span(7, 2)]);
    assert!(panel.trail_in("c.c").is_empty());
    panel.set_trail(Vec::new());
    assert!(panel.trail_in("a.c").is_empty());
}

#[test]
fn line_menu_offers_to_find_more_like_the_line() {
    let source = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";
    let mut harness = harness_for(source, "f");
    harness.get_by_label("strcpy(d, s);").click_secondary();
    harness.run();
    harness.get_by_label("Find More Like This…").click();
    harness.run();
    let Some(PanelAction::FindSimilar { line, .. }) = harness.state().last_action.clone() else {
        panic!(
            "expected FindSimilar, got {:?}",
            harness.state().last_action
        );
    };
    assert_eq!(line, 2);
}
