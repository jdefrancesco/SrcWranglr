//! The Findings panel: risky calls (and imported tool results), most
//! severe first, filterable by severity and text; click one to go there.

use egui::RichText;

use super::findings::Findings;
use crate::analysis::{Finding, Severity};
use crate::model::CodeGraph;
use crate::style::severity_color;

const DEFAULT_WIDTH: f32 = 340.0;

#[derive(Default)]
pub struct FindingsPanel {
    is_open: bool,
    /// Only this severity; `None` shows all.
    severity: Option<Severity>,
    query: String,
}

impl FindingsPanel {
    pub fn toggle(&mut self) {
        self.is_open = !self.is_open;
    }

    /// Draws the panel (if open). Returns the finding the user picked.
    pub fn show<'f>(
        &mut self,
        ui: &mut egui::Ui,
        findings: &'f Findings,
        graph: &CodeGraph,
    ) -> Option<&'f Finding> {
        if !self.is_open {
            return None;
        }
        egui::Panel::right("findings_panel")
            .resizable(true)
            .default_size(DEFAULT_WIDTH)
            .show(ui, |ui| self.contents(ui, findings, graph))
            .inner
    }

    fn contents<'f>(
        &mut self,
        ui: &mut egui::Ui,
        findings: &'f Findings,
        graph: &CodeGraph,
    ) -> Option<&'f Finding> {
        ui.heading(format!("Findings ({})", findings.all().len()));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.severity, None, "All");
            for severity in Severity::ALL {
                let label = format!("{} ({})", severity.label(), findings.count(severity));
                ui.selectable_value(&mut self.severity, Some(severity), label);
            }
        });
        let search = egui::TextEdit::singleline(&mut self.query)
            .hint_text("Filter by function, file, or rule…")
            .desired_width(f32::INFINITY);
        ui.add(search);
        ui.separator();
        let rows = listed(findings, self.severity, &self.query, graph);
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
                    .filter(|f| finding_row(ui, f, graph))
                    .last()
                    .copied()
            })
            .inner
    }
}

/// One finding on one line; hover for the explanation. Returns whether
/// it was clicked.
fn finding_row(ui: &mut egui::Ui, finding: &Finding, graph: &CodeGraph) -> bool {
    ui.horizontal(|ui| {
        let severity = RichText::new(finding.severity.label())
            .color(severity_color(finding.severity))
            .monospace();
        ui.label(severity);
        let text = format!(
            "{}  {}:{}  {}",
            finding.rule,
            finding.file,
            finding.line,
            symbol_name(finding, graph)
        );
        let cwe = finding
            .cwe
            .map(|c| format!(" (CWE-{c})"))
            .unwrap_or_default();
        let hover = format!("{}{cwe}\nfound by {}", finding.message, finding.tool);
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

/// The findings of `severity` (all if `None`) whose rule, file, or
/// function contains `query` (ignoring case), in severity order.
pub fn listed<'f>(
    findings: &'f Findings,
    severity: Option<Severity>,
    query: &str,
    graph: &CodeGraph,
) -> Vec<&'f Finding> {
    let needle = query.trim().to_lowercase();
    findings
        .all()
        .iter()
        .filter(|f| severity.is_none_or(|s| f.severity == s))
        .filter(|f| {
            needle.is_empty()
                || [f.rule.as_str(), f.file.as_str(), symbol_name(f, graph)]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{build_graph, Extractor, SourceFile};

    fn graph() -> CodeGraph {
        let source = "void copy(char *d, const char *s) { strcpy(d, s); memcpy(d, s, 1); }\n\
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

    fn rules(listed: &[&Finding]) -> Vec<String> {
        listed.iter().map(|f| f.rule.clone()).collect()
    }

    #[test]
    fn filters_by_severity_and_text() {
        let graph = graph();
        let findings = Findings::for_graph(&graph);
        assert_eq!(
            rules(&listed(&findings, None, "", &graph)),
            ["strcpy", "system", "memcpy"]
        );
        assert_eq!(
            rules(&listed(&findings, Some(Severity::Low), "", &graph)),
            ["memcpy"]
        );
        // By function name, and by rule, ignoring case.
        assert_eq!(rules(&listed(&findings, None, "RUN", &graph)), ["system"]);
        assert_eq!(
            rules(&listed(&findings, None, "cpy", &graph)),
            ["strcpy", "memcpy"]
        );
    }

    #[test]
    fn clicking_a_row_picks_its_finding() {
        use egui_kittest::kittest::Queryable;
        use egui_kittest::Harness;

        struct State {
            panel: FindingsPanel,
            findings: Findings,
            graph: CodeGraph,
            picked: Option<(String, u32)>,
        }
        let graph = graph();
        let mut panel = FindingsPanel::default();
        panel.toggle();
        let state = State {
            panel,
            findings: Findings::for_graph(&graph),
            graph,
            picked: None,
        };
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                let picked = state.panel.show(ui, &state.findings, &state.graph);
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
