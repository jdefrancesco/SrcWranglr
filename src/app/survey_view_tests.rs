//! Tests for the Project Triage window.

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;
use crate::index::{build_graph, Extractor, SourceFile};
use crate::survey::{BinaryReport, Check, LanguageStats, LineCounts, Status};

const SOURCE: &str = "#include <sys/socket.h>\n\
    int serve(int port) {\n\
      int s = socket(AF_UNIX, SOCK_STREAM, 0);\n\
      if (port > 0 && s) { pipe(fds); }\n\
      return s;\n\
    }\n\
    int tiny(void) { return 1; }\n";

fn graph() -> CodeGraph {
    let facts = Extractor::new()
        .expect("grammar")
        .extract(SOURCE)
        .expect("parse");
    build_graph(vec![SourceFile {
        path: "ipc.c".into(),
        contents: SOURCE.into(),
        facts,
    }])
}

fn survey() -> Survey {
    let lines = |code, comments, blank| LineCounts {
        code,
        comments,
        blank,
    };
    Survey {
        file_count: 12,
        total_bytes: 3 * 1024 * 1024,
        languages: vec![
            LanguageStats {
                name: "C",
                kind: Kind::Code,
                files: 2,
                lines: lines(900, 50, 40),
            },
            LanguageStats {
                name: "C/C++ header",
                kind: Kind::Code,
                files: 1,
                lines: lines(100, 5, 5),
            },
        ],
        platforms: vec![("Linux", 2)],
        build_systems: vec![("CMake", 3)],
        docs: vec!["README.md".into()],
        license: Some("MIT"),
        binaries: vec![BinaryReport {
            path: "build/server".into(),
            size: 40_000,
            format: "ELF 64-bit".into(),
            arch: "X86_64".into(),
            kind: "executable".into(),
            is_stripped: true,
            symbols: "stripped: only dynamic symbols".into(),
            checks: vec![Check {
                name: "ASLR (PIE)",
                status: Status::Bad,
                detail: "not PIE: the code is at a fixed address".into(),
            }],
            libraries: vec!["libc.so.6".into()],
            capabilities: Vec::new(),
        }],
        ..Survey::default()
    }
}

struct State {
    view: SurveyView,
    graph: CodeGraph,
    action: Option<SurveyAction>,
}

fn harness(view: SurveyView, graph: CodeGraph) -> Harness<'static, State> {
    let state = State {
        view,
        graph,
        action: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(
            |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if let Some(action) = state.view.show(&ctx, "widget", &state.graph) {
                    state.action = Some(action);
                }
            },
            state,
        );
    harness.run();
    harness
}

fn open_tab(harness: &mut Harness<'_, State>, tab: SurveyTab) {
    harness.get_by_label(tab.label()).click();
    harness.run();
}

#[test]
fn the_overview_sums_up_folder_and_code_facts() {
    let graph = graph();
    let harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    for expected in [
        "3.0 MB in 12 files",
        "1,000 code, 55 comment, 45 blank lines in 3 source files",
        "C 90%, C/C++ header 10%",
        "Linux (2 files)",
        "MIT",
        "1 program or library (1 stripped)",
        "2 functions; average complexity 2.0, highest 3 (serve)",
        "Pipes, FIFOs & mailslots (1), Unix domain sockets (1)",
    ] {
        assert!(
            harness.query_by_label_contains(expected).is_some(),
            "missing {expected:?}"
        );
    }
}

#[test]
fn clicking_a_function_in_the_complexity_list_goes_to_it() {
    let graph = graph();
    let serve = graph.functions[0].symbol;
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Complexity);
    harness.get_by_label("serve").click();
    harness.run();
    assert_eq!(
        harness.state().action,
        Some(SurveyAction::GoTo {
            symbol: Some(serve),
            file: "ipc.c".into(),
            line: 2,
        })
    );
}

#[test]
fn the_complexity_filter_narrows_the_list() {
    let graph = graph();
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Complexity);
    harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .focus();
    harness.run();
    harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("tin");
    harness.run();
    assert!(harness.query_by_label("tiny").is_some());
    assert!(harness.query_by_label("serve").is_none());
}

#[test]
fn ipc_calls_are_grouped_and_lead_to_their_call_site() {
    let graph = graph();
    let serve = graph.functions[0].symbol;
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Ipc);
    harness
        .get_by_label_contains("Unix domain sockets — 1 call: socket")
        .click();
    harness.run();
    harness.get_by_label("socket ×1").click();
    harness.run();
    harness.get_by_label("ipc.c:3").click();
    harness.run();
    assert_eq!(
        harness.state().action,
        Some(SurveyAction::GoTo {
            symbol: Some(serve),
            file: "ipc.c".into(),
            line: 3,
        })
    );
}

const FUZZ_SOURCE: &str = "#include <stdint.h>\n\
    int parse_chunk(const uint8_t *buf, size_t len) { return buf[0]; }\n\
    int add(int a, int b) { return a + b; }\n";

fn fuzz_graph() -> CodeGraph {
    crate::index::graph_of_files(&[("png.c", FUZZ_SOURCE)])
}

#[test]
fn the_fuzz_tab_ranks_targets_with_their_reasons() {
    let graph = fuzz_graph();
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Fuzz);
    assert!(harness
        .query_by_label_contains("1 function worth fuzzing (1 easy to harness)")
        .is_some());
    assert!(harness.query_by_label("parse_chunk").is_some());
    assert!(harness.query_by_label("add").is_none());
    assert!(harness
        .query_by_label_contains("takes a buffer and its length (buf, len)")
        .is_some());
}

#[test]
fn the_fuzz_tab_goes_to_a_target_or_drafts_its_harness() {
    let graph = fuzz_graph();
    let parse = graph.functions[0].symbol;
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Fuzz);
    harness.get_by_label("parse_chunk").click();
    harness.run();
    assert_eq!(
        harness.state().action,
        Some(SurveyAction::GoTo {
            symbol: Some(parse),
            file: "png.c".into(),
            line: 2,
        })
    );
    harness.get_by_label("Harness…").click();
    harness.run();
    assert_eq!(
        harness.state().action,
        Some(SurveyAction::Harness {
            symbol: parse,
            file: "png.c".into(),
            line: 2,
        })
    );
}

#[test]
fn the_overview_names_the_best_fuzz_target() {
    let graph = fuzz_graph();
    let harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    assert!(harness
        .query_by_label_contains("1 function worth fuzzing (1 easy to harness); best: parse_chunk")
        .is_some());
}

#[test]
fn the_overview_says_what_input_reaches() {
    let source = "void copy(char *d, const char *s) { strcpy(d, s); }\n\
                  void run(char *c) { system(c); }\n\
                  void serve(int fd) { char b[64]; read(fd, b, 64); run(b); }\n";
    let graph = crate::index::graph_of_files(&[("x.c", source)]);
    let harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    assert!(harness
        .query_by_label_contains("2 of 3 functions, from read(); 1 risky call in them")
        .is_some());
}

#[test]
fn the_overview_says_when_no_input_was_found() {
    let graph = fuzz_graph();
    let harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    assert!(harness
        .query_by_label_contains("no code that reads outside input was found")
        .is_some());
}

#[test]
fn the_binaries_tab_shows_failed_mitigations() {
    let graph = graph();
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Binaries);
    assert!(harness.query_by_label("FAIL").is_some());
    assert!(harness
        .query_by_label_contains("not PIE: the code is at a fixed address")
        .is_some());
    assert!(harness.query_by_label("libc.so.6").is_some());
}

#[test]
fn the_links_tab_offers_a_github_search() {
    let graph = graph();
    let mut harness = harness(SurveyView::with_survey(survey(), &graph), graph);
    open_tab(&mut harness, SurveyTab::Links);
    assert!(harness
        .query_by_label("No repository links found.")
        .is_some());
    assert!(harness
        .query_by_label("Search GitHub for “widget”")
        .is_some());
}

#[test]
fn the_demo_has_no_folder_to_scan() {
    let view = SurveyView {
        is_open: true,
        tab: SurveyTab::Languages,
        ..SurveyView::default()
    };
    let harness = harness(view, CodeGraph::default());
    assert!(harness
        .query_by_label_contains("isn't a folder on disk")
        .is_some());
}

#[test]
fn opening_on_a_folder_scans_it_in_the_background() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("main.c"), "int main(void) { return 0; }\n").expect("write");
    let ctx = egui::Context::default();
    let mut view = SurveyView::default();
    view.open(
        SurveyTab::Overview,
        &CodeGraph::default(),
        &[],
        Some(dir.path()),
        &ctx,
    );
    for _ in 0..500 {
        view.poll();
        if !matches!(view.scan, Scan::Running(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let Scan::Done(survey) = &view.scan else {
        panic!("the scan didn't finish");
    };
    assert_eq!(survey.code_lines().code, 1);
    view.reset();
    assert!(!view.is_open() && matches!(view.scan, Scan::NoFolder));
}

#[test]
fn formats_numbers() {
    assert_eq!(thousands(1_234_567), "1,234,567");
    assert_eq!(thousands(999), "999");
    assert_eq!(percent(1, 3), 33);
    assert_eq!(percent(5, 0), 0);
    assert_eq!(plural(1, "file"), "file");
    assert_eq!(plural(2, "file"), "files");
}
