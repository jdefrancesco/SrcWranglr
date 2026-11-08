//! The project's strings, for the Strings pane: each distinct string once,
//! with every place it is used.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use super::visible::visible;
use crate::model::{CodeGraph, Span, SymbolId};

/// One place a string is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringUse {
    pub file: String,
    pub span: Span,
    /// The function it is in; `None` at file scope.
    pub function: Option<SymbolId>,
}

/// A distinct string and where it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringEntry {
    /// As written in the source.
    pub text: String,
    /// In file order.
    pub uses: Vec<StringUse>,
    /// The text as the pane shows it, lowercased, for the filter.
    search: String,
}

/// How the pane orders the strings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StringOrder {
    #[default]
    Text,
    MostUsed,
}

impl StringOrder {
    pub const ALL: [StringOrder; 2] = [StringOrder::Text, StringOrder::MostUsed];

    pub fn label(self) -> &'static str {
        match self {
            StringOrder::Text => "A–Z",
            StringOrder::MostUsed => "Most used",
        }
    }
}

/// Each distinct string in `graph`, in text order (ignoring case), with
/// its uses.
pub fn entries(graph: &CodeGraph) -> Vec<StringEntry> {
    let mut files: Vec<&String> = graph.strings.keys().collect();
    files.sort();
    let mut by_text: BTreeMap<&str, Vec<StringUse>> = BTreeMap::new();
    for file in files {
        for literal in &graph.strings[file] {
            by_text
                .entry(literal.text.as_str())
                .or_default()
                .push(StringUse {
                    file: file.clone(),
                    span: literal.span,
                    function: literal.function,
                });
        }
    }
    let mut all: Vec<StringEntry> = by_text
        .into_iter()
        .map(|(text, uses)| StringEntry {
            text: text.to_string(),
            uses,
            search: visible(text).to_lowercase(),
        })
        .collect();
    // Stable, from byte order: strings that differ only in case keep it.
    all.sort_by_cached_key(|entry| entry.text.to_lowercase());
    all
}

/// Which of `entries` (by index) match `query`, ignoring case, in `order`.
/// The query is matched against the text as the pane shows it.
pub fn listed(entries: &[StringEntry], query: &str, order: StringOrder) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let mut rows: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| needle.is_empty() || e.search.contains(&needle))
        .map(|(index, _)| index)
        .collect();
    // A stable sort of text-ordered rows: ties stay in text order.
    if order == StringOrder::MostUsed {
        rows.sort_by_key(|&index| Reverse(entries[index].uses.len()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    fn graph() -> CodeGraph {
        graph_of_files(&[
            (
                "b.c",
                "int log_it(void) { return puts(\"Error: %s\\n\"); }\n\
                 int other(void) { return puts(\"zebra\"); }\n",
            ),
            (
                "a.c",
                "const char *e = \"Error: %s\\n\";\n\
                 int main(void) { puts(\"Error: %s\\n\"); return puts(\"apple\"); }\n",
            ),
        ])
    }

    fn texts(entries: &[StringEntry], rows: &[usize]) -> Vec<String> {
        rows.iter().map(|&i| entries[i].text.clone()).collect()
    }

    #[test]
    fn each_string_is_listed_once_with_all_its_uses_in_file_order() {
        let all = entries(&graph());
        let shown: Vec<(&str, usize)> = all
            .iter()
            .map(|e| (e.text.as_str(), e.uses.len()))
            .collect();
        assert_eq!(
            shown,
            [("\"apple\"", 1), ("\"Error: %s\\n\"", 3), ("\"zebra\"", 1)]
        );
        let places: Vec<(&str, u32)> = all[1]
            .uses
            .iter()
            .map(|u| (u.file.as_str(), u.span.line))
            .collect();
        assert_eq!(places, [("a.c", 1), ("a.c", 2), ("b.c", 1)]);
        assert_eq!(all[1].uses[0].function, None);
        assert!(all[1].uses[1].function.is_some());
    }

    #[test]
    fn the_filter_ignores_case_and_most_used_comes_first_when_asked() {
        let all = entries(&graph());
        let every = listed(&all, "", StringOrder::Text);
        assert_eq!(every.len(), 3);
        assert_eq!(
            texts(&all, &listed(&all, "  ERROR ", StringOrder::Text)),
            ["\"Error: %s\\n\""]
        );
        assert!(listed(&all, "nothing like it", StringOrder::Text).is_empty());
        let by_use = texts(&all, &listed(&all, "", StringOrder::MostUsed));
        // Ties keep text order.
        assert_eq!(by_use, ["\"Error: %s\\n\"", "\"apple\"", "\"zebra\""]);
    }

    #[test]
    fn text_order_ignores_case() {
        let graph = graph_of_files(&[(
            "a.c",
            "const char *t[] = { \"beta\", \"ALPHA\", \"Gamma\", \"alpha\" };\n",
        )]);
        let all = entries(&graph);
        let order: Vec<&str> = all.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(order, ["\"ALPHA\"", "\"alpha\"", "\"beta\"", "\"Gamma\""]);
    }

    #[test]
    fn the_filter_matches_what_is_shown() {
        let graph = graph_of_files(&[("a.c", "const char *r = R\"(one\ttwo)\";\n")]);
        let all = entries(&graph);
        assert_eq!(listed(&all, "one‹TAB›two", StringOrder::Text), [0]);
        assert_eq!(listed(&all, "‹tab›", StringOrder::Text), [0]);
    }
}
