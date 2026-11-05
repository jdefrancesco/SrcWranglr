//! The project's findings: risky calls found while indexing, plus any
//! results imported from external tools, with a per-file line index for
//! the code view's gutter.

use std::collections::{BTreeMap, HashMap};

use crate::analysis::risky::risky_calls;
use crate::analysis::{Finding, Severity};
use crate::model::CodeGraph;

#[derive(Debug, Default)]
pub struct Findings {
    /// Sorted by severity, then file and line.
    all: Vec<Finding>,
    /// Indices into `all`, per file.
    by_file: HashMap<String, Vec<usize>>,
}

impl Findings {
    /// The findings slurcetrail itself makes for `graph`.
    pub fn for_graph(graph: &CodeGraph) -> Self {
        Self::from_findings(risky_calls(graph))
    }

    fn from_findings(mut all: Vec<Finding>) -> Self {
        all.sort_by(|a, b| {
            (a.severity, &a.file, a.line, &a.rule).cmp(&(b.severity, &b.file, b.line, &b.rule))
        });
        let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, finding) in all.iter().enumerate() {
            by_file.entry(finding.file.clone()).or_default().push(index);
        }
        Self { all, by_file }
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

    /// The most severe finding on each line of `file`.
    pub fn worst_by_line(&self, file: &str) -> BTreeMap<u32, &Finding> {
        let mut lines: BTreeMap<u32, &Finding> = BTreeMap::new();
        for finding in self
            .by_file
            .get(file)
            .into_iter()
            .flatten()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(tool: &str, rule: &str, severity: Severity, file: &str, line: u32) -> Finding {
        Finding {
            tool: tool.into(),
            rule: rule.into(),
            severity,
            message: format!("{rule} is risky"),
            cwe: None,
            file: file.into(),
            line,
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
    fn the_graph_findings_are_its_risky_calls() {
        let graph = crate::demo::demo_codebase().expect("demo");
        assert!(Findings::for_graph(&graph).all().is_empty());
    }
}
