//! Lines fix commits last changed, as findings: they get a mark in the
//! source's gutter, a place in the Findings list, and a verdict like any
//! other finding.

use super::{date, History};
use crate::analysis::{Finding, Severity};

/// What these findings say found them.
pub const TOOL: &str = "git history";
/// Runs of lines marked at most: security fixes first, then newest.
pub const MAX_MARKED: usize = 5_000;

/// A finding at the first line of each run of lines a fix commit last
/// changed.
pub fn findings(history: &History) -> Vec<Finding> {
    let mut owned: Vec<&super::OwnedLines> = history.owned.iter().collect();
    // Commits are newest first, so a lower index is newer.
    owned.sort_by_key(|lines| {
        let strength = history.fix_commits[lines.commit].strength;
        (strength, lines.commit)
    });
    owned
        .into_iter()
        .take(MAX_MARKED)
        .map(|lines| {
            let commit = &history.fix_commits[lines.commit];
            let span = match lines.line_count {
                1 => format!("Line {} was", lines.first_line),
                _ => format!("Lines {}–{} were", lines.first_line, lines.last_line()),
            };
            Finding {
                tool: TOOL.into(),
                rule: format!("fixed in {}", commit.short),
                // Leads, not bugs: they stay below what the code itself
                // shows; the History pane tells security fixes apart.
                severity: Severity::Low,
                message: format!(
                    "{span} last changed by {} on {} by {}: \"{}\" ({}). Code fixed once is a \
                     good place to look for the same bug nearby, or for a fix that missed a case.",
                    commit.short,
                    date(commit.time),
                    commit.author,
                    commit.summary,
                    commit.keywords.join(", ")
                ),
                fix: None,
                cwe: None,
                file: lines.file.clone(),
                line: lines.first_line,
                columns: None,
                symbol: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{FixCommit, OwnedLines, Strength};

    fn commit(short: &str, strength: Strength) -> FixCommit {
        FixCommit {
            id: format!("{short}00"),
            short: short.into(),
            summary: format!("summary of {short}"),
            author: "Tess".into(),
            time: 1_700_000_000,
            keywords: vec!["fix".into()],
            strength,
        }
    }

    fn lines(file: &str, first_line: u32, line_count: u32, commit: usize) -> OwnedLines {
        OwnedLines {
            file: file.into(),
            first_line,
            line_count,
            commit,
        }
    }

    #[test]
    fn each_run_of_lines_is_a_finding_security_fixes_first() {
        let history = History {
            fix_commits: vec![
                commit("aaaa", Strength::Fix),
                commit("bbbb", Strength::Security),
            ],
            owned: vec![lines("a.c", 3, 1, 0), lines("b.c", 10, 4, 1)],
            ..History::default()
        };
        let found = findings(&history);
        let summary: Vec<(&str, u32, &str, Severity)> = found
            .iter()
            .map(|f| (f.file.as_str(), f.line, f.rule.as_str(), f.severity))
            .collect();
        assert_eq!(
            summary,
            [
                ("b.c", 10, "fixed in bbbb", Severity::Low),
                ("a.c", 3, "fixed in aaaa", Severity::Low),
            ]
        );
        assert!(found[0]
            .message
            .starts_with("Lines 10–13 were last changed by bbbb on 2023-11-14"));
        assert!(found[1].message.starts_with("Line 3 was"));
        assert!(found.iter().all(|f| f.tool == TOOL));
    }
}
