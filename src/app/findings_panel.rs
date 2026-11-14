//! The Findings panel: risky calls (and imported tool results), most
//! severe first, filterable by severity, text, and whether outside input
//! reaches them; click one to go there.

use super::visible::{visible, visible_lines};
use crate::history::findings as history_findings;
use egui::RichText;

use super::findings::Findings;
use super::layout::Layout;
use super::pane_bar::{self, Edge};
use super::range::RangeView;
use crate::analysis::reach::{describe, Reach};
use crate::analysis::{Finding, Severity};
use crate::model::CodeGraph;
use crate::notes::triage::TriageStatus;
use crate::style::{severity_text_color, INPUT_COLOR};

const DEFAULT_WIDTH: f32 = 340.0;

#[derive(Default)]
pub struct FindingsPanel {
    is_open: bool,
    filter: Filter,
}

/// Which findings the panel lists.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this severity; `None` shows all.
    pub severity: Option<Severity>,
    /// Matched against rule, file and function, ignoring case.
    pub query: String,
    /// Also list findings you marked reviewed or false positive.
    pub show_dismissed: bool,
    /// Only findings in functions outside input reaches.
    pub only_reachable: bool,
    /// Leave out the lines fix commits last changed (git history marks).
    pub hide_history: bool,
    /// Only findings in code the compared range changed (while one is
    /// compared).
    pub only_changed: bool,
}

impl FindingsPanel {
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, is_open: bool) {
        self.is_open = is_open;
    }

    /// Draws the panel (while open, sliding in and out). Returns the
    /// finding the user picked.
    pub fn show<'f>(
        &mut self,
        ui: &mut egui::Ui,
        findings: &'f Findings,
        graph: &CodeGraph,
        reach: &Reach,
        layout: Layout,
        range: Option<&RangeView>,
    ) -> Option<&'f Finding> {
        let mut is_open = self.is_open;
        let shown = pane_bar::right_pane(layout.pane_id("findings_panel"), DEFAULT_WIDTH)
            .show_collapsible(ui, &mut is_open, |ui| {
                self.contents(ui, findings, graph, reach, range)
            });
        // Shut by dragging, or by its hide arrow.
        self.is_open = is_open && self.is_open;
        shown.and_then(|shown| shown.inner)
    }

    fn contents<'f>(
        &mut self,
        ui: &mut egui::Ui,
        findings: &'f Findings,
        graph: &CodeGraph,
        reach: &Reach,
        range: Option<&RangeView>,
    ) -> Option<&'f Finding> {
        let title = format!("Findings ({})", findings.all().len());
        if pane_bar::heading(ui, title, Edge::Right) {
            self.is_open = false;
        }
        let filter = &mut self.filter;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut filter.severity, None, "All");
            for severity in Severity::ALL {
                let label = format!("{} ({})", severity.label(), findings.count(severity));
                ui.selectable_value(&mut filter.severity, Some(severity), label);
            }
        });
        let search = egui::TextEdit::singleline(&mut filter.query)
            .hint_text("Filter by function, file, or rule…")
            .desired_width(f32::INFINITY);
        ui.add(search);
        ui.checkbox(
            &mut filter.show_dismissed,
            "Show reviewed and false positives",
        );
        let reachable = findings
            .all()
            .iter()
            .filter(|f| reach.reaches_finding(f))
            .count();
        ui.checkbox(
            &mut filter.only_reachable,
            format!("Only what input can reach ({reachable})"),
        )
        .on_hover_text(REACH_HINT);
        let history = findings
            .all()
            .iter()
            .filter(|f| f.tool == history_findings::TOOL)
            .count();
        if history > 0 {
            let mut is_shown = !filter.hide_history;
            ui.checkbox(
                &mut is_shown,
                format!("Lines fix commits changed ({history})"),
            )
            .on_hover_text("Marks from the git history: code a fix commit last changed");
            filter.hide_history = !is_shown;
        }
        if let Some(range) = range {
            let changed = findings
                .all()
                .iter()
                .filter(|f| range.is_changed_code(&f.file, f.line))
                .count();
            ui.checkbox(
                &mut filter.only_changed,
                format!(
                    "Only changed code since {} ({changed})",
                    visible(&range.changes.base)
                ),
            )
            .on_hover_text(
                "Findings on lines the compared range changed, or in functions it changed \
                 (History → Changes)",
            );
        }
        ui.separator();
        let rows = listed(findings, filter, graph, reach);
        let rows = match filter.only_changed {
            true => in_changed_code(rows, range),
            false => rows,
        };
        if rows.is_empty() {
            ui.weak(if findings.all().is_empty() {
                "No risky calls found."
            } else {
                "No matching findings."
            });
            return None;
        }
        let row_height = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                rows[range]
                    .iter()
                    .filter(|(i, f)| finding_row(ui, f, findings.verdict(*i), graph, reach))
                    .last()
                    .map(|(_, f)| *f)
            })
            .inner
    }
}

/// One finding on one line; hover for the explanation. Returns whether
/// it was clicked.
fn finding_row(
    ui: &mut egui::Ui,
    finding: &Finding,
    verdict: Option<TriageStatus>,
    graph: &CodeGraph,
    reach: &Reach,
) -> bool {
    ui.horizontal(|ui| {
        if reach.reaches_finding(finding) {
            ui.label(RichText::new("input").color(INPUT_COLOR).small())
                .on_hover_ui(|ui| {
                    if let Some(path) = reach.finding_path(finding) {
                        ui.label(format!("Input may reach this: {}", describe(&path, graph)));
                    }
                });
        }
        if let Some(verdict) = verdict {
            ui.label(RichText::new(format!("[{}]", verdict.label().to_lowercase())).strong());
        }
        let severity = RichText::new(finding.severity.label())
            .color(severity_text_color(finding.severity, ui.visuals()))
            .monospace();
        ui.label(severity);
        // Rules, tools and paths may come from an imported report.
        let text = visible(&format!(
            "{}  {}:{}  {}",
            finding.rule,
            finding.file,
            finding.line,
            symbol_name(finding, graph)
        ))
        .into_owned();
        let cwe = finding
            .cwe
            .map(|c| format!(" (CWE-{c})"))
            .unwrap_or_default();
        let hover = format!(
            "{}{cwe}\nfound by {}",
            visible_lines(&finding.message),
            visible(&finding.tool)
        );
        ui.selectable_label(false, text)
            .on_hover_text(hover)
            .clicked()
    })
    .inner
}

fn symbol_name<'g>(finding: &Finding, graph: &'g CodeGraph) -> &'g str {
    finding
        .symbol
        .and_then(|id| graph.symbols.get(&id))
        .map_or("", |s| s.qualified_name.as_str())
}

/// What "reachable" can and can't promise.
const REACH_HINT: &str = "Findings in functions that a call path leads to from code reading \
    outside input (recv, read, fread, getenv, the command line, ...). Only direct calls are \
    followed: calls through function pointers and callbacks aren't, so a finding left out \
    isn't proven unreachable.";

/// Of `rows`, those in code `range` changed: all of them without a range.
pub fn in_changed_code<'f>(
    rows: Vec<(usize, &'f Finding)>,
    range: Option<&RangeView>,
) -> Vec<(usize, &'f Finding)> {
    let Some(range) = range else {
        return rows;
    };
    rows.into_iter()
        .filter(|(_, f)| range.is_changed_code(&f.file, f.line))
        .collect()
}

/// The findings `filter` lets through, in severity order.
pub fn listed<'f>(
    findings: &'f Findings,
    filter: &Filter,
    graph: &CodeGraph,
    reach: &Reach,
) -> Vec<(usize, &'f Finding)> {
    let needle = filter.query.trim().to_lowercase();
    let matches = |f: &Finding| {
        needle.is_empty()
            || [f.rule.as_str(), f.file.as_str(), symbol_name(f, graph)]
                .iter()
                .any(|text| text.to_lowercase().contains(&needle))
    };
    findings
        .all()
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            filter.show_dismissed || !findings.verdict(*i).is_some_and(TriageStatus::is_dismissed)
        })
        .filter(|(_, f)| filter.severity.is_none_or(|s| f.severity == s) && matches(f))
        .filter(|(_, f)| !filter.only_reachable || reach.reaches_finding(f))
        .filter(|(_, f)| !filter.hide_history || f.tool != history_findings::TOOL)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::reach::Reach;
    use crate::index::{build_graph, Extractor, SourceFile};

    fn graph() -> CodeGraph {
        let source =
            "void copy(char *d, const char *s, int n) { strcpy(d, s); memcpy(d, s, n); }\n\
                      void run(char *c) { system(c); }\n";
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        build_graph(vec![SourceFile {
            path: "x.c".into(),
            contents: source.into(),
            facts,
        }])
    }

    fn filter(query: &str) -> Filter {
        Filter {
            query: query.into(),
            ..Filter::default()
        }
    }

    #[test]
    fn findings_input_reaches_are_tagged() {
        use egui_kittest::kittest::Queryable;

        let source = "void copy(char *d, const char *s) { strcpy(d, s); }\n\
                      void run(char *c) { system(c); }\n\
                      void serve(int fd) { char b[64]; read(fd, b, 64); run(b); }\n";
        let graph = crate::index::graph_of_files(&[("x.c", source)]);
        let findings = Findings::for_graph(&graph, &Default::default());
        let reach = Reach::of(&graph);
        let harness = crate::app::fonts::test_harness().build_ui(|ui| {
            let mut panel = FindingsPanel {
                is_open: true,
                filter: Filter::default(),
            };
            panel.show(ui, &findings, &graph, &reach, Layout::default(), None);
        });
        assert_eq!(harness.query_all_by_label("input").count(), 1);
        assert!(harness
            .query_by_label("Only what input can reach (1)")
            .is_some());
    }

    #[test]
    fn a_compared_range_can_narrow_the_list_to_changed_code() {
        use crate::history::changes::{ChangedFile, Changes, Hunk};
        let source = "void copy(char *d, const char *s) { strcpy(d, s); }\n\
                      void run(char *c) {\n  system(c);\n}\n";
        let graph = crate::index::graph_of_files(&[("x.c", source)]);
        let findings = Findings::for_graph(&graph, &Default::default());
        let reach = Reach::of(&graph);
        // run()'s opening line changed; its system() call didn't.
        let changes = Changes {
            base: "v1".into(),
            files: vec![ChangedFile {
                file: "x.c".into(),
                renamed_from: None,
                is_new: false,
                hunks: vec![Hunk {
                    first_line: 2,
                    line_count: 1,
                    removed: 1,
                }],
                added: 1,
                removed: 1,
            }],
            ..Changes::default()
        };
        let range = RangeView::new(changes, &graph);
        let all = listed(&findings, &Filter::default(), &graph, &reach);
        assert_eq!(
            rules(&in_changed_code(all.clone(), Some(&range))),
            ["system"]
        );
        assert_eq!(rules(&in_changed_code(all, None)), ["strcpy", "system"]);
    }

    #[test]
    fn git_history_marks_can_be_left_out_of_the_list() {
        let graph = graph();
        let mark = Finding {
            tool: history_findings::TOOL.into(),
            rule: "fixed in aaaa1111".into(),
            severity: Severity::Low,
            message: String::new(),
            fix: None,
            cwe: None,
            file: "x.c".into(),
            line: 1,
            columns: None,
            symbol: None,
        };
        let findings = Findings::for_graph(&graph, &Default::default())
            .with_tool_results(history_findings::TOOL, vec![mark]);
        let reach = Reach::of(&graph);
        let count = |filter: &Filter| listed(&findings, filter, &graph, &reach).len();
        let everything = count(&Filter::default());
        let without = Filter {
            hide_history: true,
            ..Filter::default()
        };
        assert_eq!(count(&without), everything - 1);
    }

    #[test]
    fn can_list_only_what_input_reaches() {
        let source = "void copy(char *d, const char *s) { strcpy(d, s); }\n\
                      void run(char *c) { system(c); }\n\
                      void serve(int fd) { char b[64]; read(fd, b, 64); run(b); }\n";
        let graph = crate::index::graph_of_files(&[("x.c", source)]);
        let findings = Findings::for_graph(&graph, &Default::default());
        let reach = Reach::of(&graph);
        let only = Filter {
            only_reachable: true,
            ..Filter::default()
        };
        assert_eq!(rules(&listed(&findings, &only, &graph, &reach)), ["system"]);
        assert_eq!(
            rules(&listed(&findings, &Filter::default(), &graph, &reach)),
            ["strcpy", "system"]
        );
    }

    fn rules(listed: &[(usize, &Finding)]) -> Vec<String> {
        listed.iter().map(|(_, f)| f.rule.clone()).collect()
    }

    #[test]
    fn filters_by_severity_and_text() {
        let graph = graph();
        let findings = Findings::for_graph(&graph, &Default::default());
        assert_eq!(
            rules(&listed(
                &findings,
                &Filter::default(),
                &graph,
                &Reach::default()
            )),
            ["strcpy", "system", "memcpy"]
        );
        assert_eq!(
            rules(&listed(
                &findings,
                &Filter {
                    severity: Some(Severity::Low),
                    ..Filter::default()
                },
                &graph,
                &Reach::default()
            )),
            ["memcpy"]
        );
        // By function name, and by rule, ignoring case.
        assert_eq!(
            rules(&listed(
                &findings,
                &filter("RUN"),
                &graph,
                &Reach::default()
            )),
            ["system"]
        );
        assert_eq!(
            rules(&listed(
                &findings,
                &filter("cpy"),
                &graph,
                &Reach::default()
            )),
            ["strcpy", "memcpy"]
        );
    }

    #[test]
    fn dismissed_findings_are_hidden_unless_asked_for() {
        use crate::notes::triage::{Triage, TriageEntry};
        let graph = graph();
        let mut triage = Triage::default();
        triage
            .set(TriageEntry {
                rule: "system".into(),
                file: "x.c".into(),
                line: 2,
                column: 20,
                anchor: String::new(),
                status: TriageStatus::Reviewed,
                note: String::new(),
                updated: 1,
            })
            .expect("set");
        let findings =
            Findings::for_graph(&graph, &Default::default()).with_triage(&triage, &graph, true);
        assert_eq!(
            rules(&listed(
                &findings,
                &Filter::default(),
                &graph,
                &Reach::default()
            )),
            ["strcpy", "memcpy"]
        );
        assert_eq!(
            rules(&listed(
                &findings,
                &Filter {
                    show_dismissed: true,
                    ..Filter::default()
                },
                &graph,
                &Reach::default()
            )),
            ["strcpy", "system", "memcpy"]
        );
    }

    #[test]
    fn clicking_a_row_picks_its_finding() {
        use egui_kittest::kittest::Queryable;

        struct State {
            panel: FindingsPanel,
            findings: Findings,
            graph: CodeGraph,
            picked: Option<(String, u32)>,
        }
        let graph = graph();
        let mut panel = FindingsPanel::default();
        panel.set_open(true);
        let state = State {
            panel,
            findings: Findings::for_graph(&graph, &Default::default()),
            graph,
            picked: None,
        };
        let mut harness = crate::app::fonts::test_harness().build_ui_state(
            |ui, state: &mut State| {
                let picked = state.panel.show(
                    ui,
                    &state.findings,
                    &state.graph,
                    &Reach::default(),
                    Layout::default(),
                    None,
                );
                if let Some(f) = picked {
                    state.picked = Some((f.rule.clone(), f.line));
                }
            },
            state,
        );
        harness.run();
        harness.get_by_label("system  x.c:2  run").click();
        harness.run();
        assert_eq!(harness.state().picked, Some(("system".into(), 2)));
    }
}
