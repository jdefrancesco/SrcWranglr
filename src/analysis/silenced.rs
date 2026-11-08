//! Comments in the code telling an analyzer not to report (`// nosemgrep`,
//! `// cppcheck-suppress`, `// NOLINT`, ...), as findings: someone saw a
//! warning there and silenced it, which is worth knowing why. SrcWranglr
//! runs semgrep, cppcheck and flawfinder with these comments ignored, so
//! the code can't hide anything from them that way.

use super::{Finding, Severity};
use crate::index::MAX_MARKS_PER_FILE;
use crate::model::{Analyzer, CodeGraph, CommentMarkKind};

/// What these findings say found them.
pub const TOOL: &str = "source comments";

/// Analyzers SrcWranglr runs itself, with their suppression comments
/// turned off.
const RUN_HERE: [Analyzer; 3] = [Analyzer::Semgrep, Analyzer::Cppcheck, Analyzer::Flawfinder];

/// A finding for each comment in `graph` silencing an analyzer.
pub fn silencing_comments(graph: &CodeGraph) -> Vec<Finding> {
    let mut found: Vec<Finding> = graph
        .comment_marks
        .iter()
        .flat_map(|(file, marks)| {
            marks.iter().filter_map(move |mark| match mark.kind {
                CommentMarkKind::Silences(analyzer) => Some(Finding {
                    tool: TOOL.into(),
                    rule: rule(analyzer),
                    severity: Severity::Low,
                    message: message(analyzer),
                    fix: Some(
                        "Read what the tool would have said here, and whether the reason for \
                         silencing it still holds."
                            .into(),
                    ),
                    cwe: None,
                    file: file.clone(),
                    line: mark.span.line,
                    columns: Some((mark.span.start, mark.span.end)),
                    symbol: mark.function,
                }),
                CommentMarkKind::Tag(_) => None,
            })
        })
        .collect();
    found.extend(
        graph
            .silencers_cut_short
            .iter()
            .map(|file| too_many_in(file)),
    );
    found.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    found
}

/// Says `file` had more silencing comments than were kept, so the list of
/// them is short there.
fn too_many_in(file: &str) -> Finding {
    Finding {
        tool: TOOL.into(),
        rule: "silences too much".into(),
        severity: Severity::Medium,
        message: format!(
            "This file has more comments silencing analyzers than SrcWranglr keeps \
             ({MAX_MARKS_PER_FILE}), so not all are listed. That many is unusual in itself."
        ),
        fix: Some("Search the file for nosemgrep, NOLINT, cppcheck-suppress and the like.".into()),
        cwe: None,
        file: file.to_string(),
        line: 1,
        columns: None,
        symbol: None,
    }
}

pub fn rule(analyzer: Analyzer) -> String {
    format!("silences {}", analyzer.label())
}

fn message(analyzer: Analyzer) -> String {
    let name = analyzer.label();
    let ours = if RUN_HERE.contains(&analyzer) {
        format!("SrcWranglr runs {name} with these comments ignored, so it still reports here.")
    } else {
        format!("If you run {name} yourself, it won't report what this comment silences.")
    };
    format!(
        "A comment tells {name} not to report this code. Someone saw a warning here and \
         chose to silence it. {ours}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    #[test]
    fn each_silencing_comment_is_a_finding_and_tags_are_not() {
        let graph = graph_of_files(&[(
            "a.c",
            "int f(char *d, const char *s) {\n\
               strcpy(d, s); // nosemgrep: c.lang.strcpy\n\
               // TODO: bound this\n\
               return 0; // NOLINT\n\
             }\n",
        )]);
        let found = silencing_comments(&graph);
        let rules: Vec<(&str, u32)> = found.iter().map(|f| (f.rule.as_str(), f.line)).collect();
        assert_eq!(rules, [("silences semgrep", 2), ("silences clang-tidy", 4)]);
        assert!(found.iter().all(|f| f.tool == TOOL && f.symbol.is_some()));
        assert!(found[0].message.contains("still reports here"));
        assert!(found[1].message.contains("If you run clang-tidy yourself"));
        let line = graph.file_contents["a.c"].lines().nth(1).expect("line 2");
        let (start, end) = found[0].columns.expect("columns");
        assert_eq!(&line[start as usize..end as usize], "nosemgrep");
    }

    #[test]
    fn a_file_with_too_many_to_keep_says_so() {
        let mut graph = graph_of_files(&[("a.c", "int x;\n")]);
        graph.silencers_cut_short = vec!["a.c".into()];
        let found = silencing_comments(&graph);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, "silences too much");
        assert_eq!(found[0].severity, Severity::Medium);
    }
}
