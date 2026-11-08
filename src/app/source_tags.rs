//! The TODO-style tags written in the project's comments (`TODO`, `FIXME`,
//! `XXX`, `HACK`, `BUG`), for the TODO pane's "In the source" list.

use super::visible::visible;
use crate::model::{CodeGraph, CommentMarkKind, SourceTag, Span, SymbolId};

/// One tag written in a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTagEntry {
    pub tag: SourceTag,
    pub file: String,
    /// Where the tag is written.
    pub span: Span,
    /// The comment from the tag to the end of its line.
    pub text: String,
    pub function: Option<SymbolId>,
    /// The function's qualified name, if it is in one.
    pub function_name: Option<String>,
    /// Text and file as the pane shows them, lowercased, for the filter.
    search: String,
}

/// The project's tags, and the files with more marked comments than were
/// kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceTags {
    /// By file, then line.
    pub entries: Vec<SourceTagEntry>,
    pub cut_short: Vec<String>,
}

/// Every tag in `graph`'s comments.
pub fn source_tags(graph: &CodeGraph) -> SourceTags {
    let mut files: Vec<&String> = graph.comment_marks.keys().collect();
    files.sort();
    let entries = files
        .into_iter()
        .flat_map(|file| {
            graph.comment_marks[file].iter().filter_map(move |mark| {
                let CommentMarkKind::Tag(tag) = mark.kind else {
                    return None;
                };
                let function_name = mark
                    .function
                    .and_then(|id| graph.symbols.get(&id))
                    .map(|symbol| symbol.qualified_name.clone());
                Some(SourceTagEntry {
                    tag,
                    file: file.clone(),
                    span: mark.span,
                    text: mark.text.clone(),
                    function: mark.function,
                    function_name,
                    search: format!("{} {}", visible(&mark.text), visible(file)).to_lowercase(),
                })
            })
        })
        .collect();
    SourceTags {
        entries,
        cut_short: graph.tags_cut_short.clone(),
    }
}

/// Which of `entries` (by index) match `query` (ignoring case, in their
/// text or file) and are `only` that tag, if given; in their order.
pub fn listed(entries: &[SourceTagEntry], query: &str, only: Option<SourceTag>) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| only.is_none_or(|tag| e.tag == tag))
        .filter(|(_, e)| needle.is_empty() || e.search.contains(&needle))
        .map(|(index, _)| index)
        .collect()
}

/// How many of `entries` are `tag`.
pub fn count(entries: &[SourceTagEntry], tag: SourceTag) -> usize {
    entries.iter().filter(|e| e.tag == tag).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    fn graph() -> CodeGraph {
        graph_of_files(&[
            (
                "b.c",
                "int parse(void) {\n  // FIXME: trusts the length\n  return 0; // NOLINT\n}\n",
            ),
            ("a.c", "// TODO: free on error\n/* XXX racy */\n"),
        ])
    }

    #[test]
    fn lists_tags_by_file_and_line_but_not_silencing_comments() {
        let tags = source_tags(&graph());
        let shown: Vec<(&str, u32, SourceTag, &str)> = tags
            .entries
            .iter()
            .map(|e| (e.file.as_str(), e.span.line, e.tag, e.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("a.c", 1, SourceTag::Todo, "TODO: free on error"),
                ("a.c", 2, SourceTag::Xxx, "XXX racy"),
                ("b.c", 2, SourceTag::Fixme, "FIXME: trusts the length"),
            ]
        );
        assert_eq!(tags.entries[2].function_name.as_deref(), Some("parse"));
        assert_eq!(tags.entries[0].function_name, None);
        assert!(tags.cut_short.is_empty());
    }

    #[test]
    fn filters_by_text_file_and_tag() {
        let tags = source_tags(&graph());
        let all = &tags.entries;
        assert_eq!(listed(all, "", None), [0, 1, 2]);
        assert_eq!(listed(all, "  LENGTH ", None), [2]);
        assert_eq!(listed(all, "b.c", None), [2]);
        assert_eq!(listed(all, "", Some(SourceTag::Xxx)), [1]);
        assert!(listed(all, "racy", Some(SourceTag::Todo)).is_empty());
        assert_eq!(count(all, SourceTag::Todo), 1);
        assert_eq!(count(all, SourceTag::Hack), 0);
    }
}
