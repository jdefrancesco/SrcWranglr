//! The project's findings: risky calls found while indexing, plus any
//! results imported from external tools, with a per-file line index for
//! the code view's gutter.

use std::collections::{BTreeMap, HashMap};

use crate::analysis::dangerous::DangerousList;
use crate::analysis::risky::{self, risky_calls};
use crate::analysis::{Finding, Severity};
use crate::model::CodeGraph;
use crate::notes::triage::{FindingKey, Triage, TriageStatus};

#[derive(Debug, Default)]
pub struct Findings {
    /// Sorted by severity, then file and line.
    all: Vec<Finding>,
    /// Indices into `all`, per file.
    by_file: HashMap<String, Vec<usize>>,
    /// Your verdict on each finding, with its note (same order as `all`).
    verdicts: Vec<Option<(TriageStatus, String)>>,
    /// Whether reviewed / false-positive verdicts hide findings.
    are_dismissals_applied: bool,
}

impl Findings {
    /// The findings SrcWranglr itself makes for `graph`, with `list`'s
    /// functions flagged too.
    pub fn for_graph(graph: &CodeGraph, list: &DangerousList) -> Self {
        Self::from_findings(risky_calls(graph, list))
    }

    fn from_findings(mut all: Vec<Finding>) -> Self {
        all.sort_by(|a, b| {
            (a.severity, &a.file, a.line, &a.rule).cmp(&(b.severity, &b.file, b.line, &b.rule))
        });
        let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, finding) in all.iter().enumerate() {
            by_file.entry(finding.file.clone()).or_default().push(index);
        }
        let verdicts = vec![None; all.len()];
        Self {
            all,
            by_file,
            verdicts,
            are_dismissals_applied: true,
        }
    }

    /// These findings with your verdicts from `triage` attached. With
    /// `apply_dismissals` false (verdicts you haven't trusted), reviewed
    /// and false-positive findings are still highlighted.
    pub fn with_triage(self, triage: &Triage, graph: &CodeGraph, apply_dismissals: bool) -> Self {
        let lines = LineIndex::new(graph, self.all.iter().map(|f| f.file.as_str()));
        let keys: Vec<FindingKey> = self
            .all
            .iter()
            .map(|f| FindingKey {
                rule: &f.rule,
                file: &f.file,
                line: f.line,
                column: f.columns.map_or(0, |(start, _)| start),
                text: lines.text(&f.file, f.line),
            })
            .collect();
        let verdicts = triage
            .match_findings(&keys, |file, line| lines.text(file, line))
            .into_iter()
            .map(|e| e.map(|e| (e.status, e.note.clone())))
            .collect();
        Self {
            verdicts,
            are_dismissals_applied: apply_dismissals,
            ..self
        }
    }

    /// Your verdict on the finding at `index` of `all`.
    pub fn verdict(&self, index: usize) -> Option<TriageStatus> {
        self.verdict_and_note(index).map(|(status, _)| status)
    }

    /// Your verdict on the finding at `index`, with its note.
    pub fn verdict_and_note(&self, index: usize) -> Option<(TriageStatus, String)> {
        self.verdicts.get(index).cloned().flatten()
    }

    fn is_dismissed(&self, index: usize) -> bool {
        self.are_dismissals_applied && self.verdict(index).is_some_and(TriageStatus::is_dismissed)
    }

    /// These findings with SrcWranglr's own (rules and your list) made
    /// again for `graph` and `list`; external tools' results are kept.
    pub fn with_own_results(&self, graph: &CodeGraph, list: &DangerousList) -> Self {
        let tools = self
            .all
            .iter()
            .filter(|f| f.tool != risky::TOOL && f.tool != risky::LIST_TOOL)
            .cloned();
        Self::from_findings(tools.chain(risky_calls(graph, list)).collect())
    }

    /// These findings with `tool`'s replaced by `found`.
    pub fn with_tool_results(&self, tool: &str, found: Vec<Finding>) -> Self {
        let kept = self.all.iter().filter(|f| f.tool != tool).cloned();
        Self::from_findings(kept.chain(found).collect())
    }

    pub fn all(&self) -> &[Finding] {
        &self.all
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.all.iter().filter(|f| f.severity == severity).count()
    }

    /// Findings in `file` that know their snippet's columns, with their
    /// index in `all`.
    pub fn snippets_in(&self, file: &str) -> impl Iterator<Item = (usize, &Finding)> {
        self.by_file
            .get(file)
            .into_iter()
            .flatten()
            .filter(|i| !self.is_dismissed(**i))
            .map(|i| (*i, &self.all[*i]))
            .filter(|(_, f)| f.columns.is_some())
    }

    /// The most severe finding on each line of `file`.
    pub fn worst_by_line(&self, file: &str) -> BTreeMap<u32, &Finding> {
        let mut lines: BTreeMap<u32, &Finding> = BTreeMap::new();
        let shown = self.by_file.get(file).into_iter().flatten();
        for finding in shown
            .filter(|i| !self.is_dismissed(**i))
            .map(|i| &self.all[*i])
        {
            lines
                .entry(finding.line)
                .and_modify(|worst| {
                    if finding.severity < worst.severity {
                        *worst = finding;
                    }
                })
                .or_insert(finding);
        }
        lines
    }
}

/// The lines of the files some findings are in, split once.
struct LineIndex<'g> {
    files: HashMap<&'g str, Vec<&'g str>>,
}

impl<'g> LineIndex<'g> {
    fn new<'f>(graph: &'g CodeGraph, files: impl Iterator<Item = &'f str>) -> Self {
        let mut index: HashMap<&'g str, Vec<&'g str>> = HashMap::new();
        for file in files {
            if let Some((name, text)) = graph.file_contents.get_key_value(file) {
                index
                    .entry(name.as_str())
                    .or_insert_with(|| text.lines().collect());
            }
        }
        Self { files: index }
    }

    /// Line `line` (1-based) of `file`, "" if unknown.
    fn text(&self, file: &str, line: u32) -> &'g str {
        let index = usize::try_from(line.saturating_sub(1)).unwrap_or(usize::MAX);
        self.files
            .get(file)
            .and_then(|lines| lines.get(index))
            .copied()
            .unwrap_or("")
    }
}

/// The text of 1-based `line` of `file` ("" if unknown).
pub fn line_text<'g>(graph: &'g CodeGraph, file: &str, line: u32) -> &'g str {
    let index = usize::try_from(line.saturating_sub(1)).unwrap_or(usize::MAX);
    graph
        .file_contents
        .get(file)
        .and_then(|text| text.lines().nth(index))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(tool: &str, rule: &str, severity: Severity, file: &str, line: u32) -> Finding {
        Finding {
            tool: tool.into(),
            rule: rule.into(),
            severity,
            message: format!("{rule} is risky"),
            fix: None,
            cwe: None,
            file: file.into(),
            line,
            columns: None,
            symbol: None,
        }
    }

    fn sample() -> Findings {
        Findings::from_findings(vec![
            finding("t", "memcpy", Severity::Low, "a.c", 3),
            finding("t", "strcpy", Severity::High, "b.c", 9),
            finding("t", "strncpy", Severity::Medium, "a.c", 3),
            finding("t", "system", Severity::High, "a.c", 7),
        ])
    }

    #[test]
    fn findings_are_sorted_most_severe_first() {
        let findings = sample();
        let rules: Vec<&str> = findings.all().iter().map(|f| f.rule.as_str()).collect();
        assert_eq!(rules, ["system", "strcpy", "strncpy", "memcpy"]);
        assert_eq!(findings.count(Severity::High), 2);
    }

    #[test]
    fn each_line_shows_its_most_severe_finding() {
        let findings = sample();
        let lines: Vec<(u32, &str)> = findings
            .worst_by_line("a.c")
            .into_iter()
            .map(|(line, f)| (line, f.rule.as_str()))
            .collect();
        assert_eq!(lines, [(3, "strncpy"), (7, "system")]);
        assert!(findings.worst_by_line("none.c").is_empty());
    }

    #[test]
    fn tool_results_replace_that_tools_earlier_results_only() {
        let findings = sample()
            .with_tool_results(
                "semgrep",
                vec![finding("semgrep", "r1", Severity::Low, "c.c", 1)],
            )
            .with_tool_results(
                "semgrep",
                vec![finding("semgrep", "r2", Severity::Low, "c.c", 2)],
            );
        let rules: Vec<&str> = findings.all().iter().map(|f| f.rule.as_str()).collect();
        assert_eq!(rules, ["system", "strcpy", "strncpy", "memcpy", "r2"]);
    }

    #[test]
    fn own_results_are_redone_and_tool_results_kept() {
        let graph = crate::demo::demo_codebase().expect("demo");
        let list = DangerousList::parse(
            "[[function]]\nname = \"total_area\"\nseverity = \"low\"\nreason = \"r\"\n",
        )
        .expect("list");
        let findings = Findings::for_graph(&graph, &DangerousList::default())
            .with_tool_results(
                "semgrep",
                vec![finding("semgrep", "x", Severity::Low, "a.c", 1)],
            )
            .with_own_results(&graph, &list);
        let rules: Vec<&str> = findings.all().iter().map(|f| f.rule.as_str()).collect();
        // Both low: then by file (a.c before main.cpp).
        assert_eq!(rules, ["x", "total_area"]);
    }

    #[test]
    fn dismissed_findings_are_not_highlighted_but_still_listed() {
        let source = "void f(char *d, char *s) {\nstrcpy(d, s);\n}\n";
        let facts = crate::index::Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        let graph = crate::index::build_graph(vec![crate::index::SourceFile {
            path: "a.c".into(),
            contents: source.into(),
            facts,
        }]);
        let mut triage = Triage::default();
        triage
            .set(crate::notes::triage::TriageEntry {
                rule: "strcpy".into(),
                file: "a.c".into(),
                line: 2,
                column: 0,
                anchor: "strcpy(d, s);".into(),
                status: TriageStatus::FalsePositive,
                note: String::new(),
                updated: 1,
            })
            .expect("set");
        let found = Findings::for_graph(&graph, &DangerousList::default());
        // Untrusted verdicts are shown but don't hide anything.
        let untrusted = Findings::for_graph(&graph, &DangerousList::default())
            .with_triage(&triage, &graph, false);
        assert_eq!(untrusted.verdict(0), Some(TriageStatus::FalsePositive));
        assert_eq!(untrusted.snippets_in("a.c").count(), 1);
        let findings = found.with_triage(&triage, &graph, true);
        assert_eq!(findings.all().len(), 1);
        assert_eq!(findings.verdict(0), Some(TriageStatus::FalsePositive));
        assert_eq!(findings.snippets_in("a.c").count(), 0);
        assert!(findings.worst_by_line("a.c").is_empty());
    }

    #[test]
    fn the_graph_findings_are_its_risky_calls() {
        let graph = crate::demo::demo_codebase().expect("demo");
        assert!(Findings::for_graph(&graph, &Default::default())
            .all()
            .is_empty());
    }
}
