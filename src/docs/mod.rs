//! Documentation for a function: its man page when it comes from a
//! library, or the comment above it (read as Doxygen) when the project
//! defines it.

pub mod comments;
pub mod doxygen;
pub mod man;

use crate::model::{CodeGraph, OccurrenceRole, SymbolId};
use comments::{comment_above, DocComment};

/// The comment documenting the project symbol `id`, and where it is: the
/// first Doxygen comment above its definition or a declaration, else the
/// first plain one.
pub fn symbol_comment(graph: &CodeGraph, id: SymbolId) -> Option<(String, DocComment)> {
    let symbol = graph.symbols.get(&id)?;
    let declared = graph
        .locations_of(id)
        .into_iter()
        .filter(|l| l.occurrence.role == OccurrenceRole::Declaration)
        .map(|l| (l.file, l.occurrence.span.line));
    let places = std::iter::once((symbol.file.clone(), symbol.line)).chain(declared);
    let mut plain = None;
    for (file, line) in places {
        let Some(source) = graph.file_contents.get(&file) else {
            continue;
        };
        let Some(comment) = comment_above(source, line) else {
            continue;
        };
        if comment.is_doxygen {
            return Some((file, comment));
        }
        plain.get_or_insert((file, comment));
    }
    plain
}

/// The brief of `id`'s doc comment, for a tooltip.
pub fn symbol_brief(graph: &CodeGraph, id: SymbolId) -> Option<String> {
    let (_, comment) = symbol_comment(graph, id)?;
    let name = &graph.symbols.get(&id)?.name;
    let brief = doc_of(&comment.text, name).brief;
    (!brief.is_empty()).then_some(brief)
}

/// The parts of the comment `text` documenting `name`. A first paragraph
/// that only repeats the name (`parse_header()`, a heading many projects
/// write) isn't its brief: the next paragraph is.
pub fn doc_of(text: &str, name: &str) -> doxygen::Doc {
    let doc = doxygen::parse(text);
    let heading = doc
        .brief
        .trim()
        .trim_end_matches(':')
        .trim_end_matches("()");
    if heading != name || doc.details.is_empty() {
        return doc;
    }
    let mut details = doc.details;
    let brief = details.remove(0);
    doxygen::Doc {
        brief,
        details,
        ..doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    fn id(graph: &CodeGraph, name: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.name == name)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn a_header_s_doxygen_comment_wins_over_a_plain_one_at_the_definition() {
        let header = "/** @brief Parses a header. @param p text */\nint parse(const char *p);\n";
        let source =
            "#include \"p.h\"\n/* the parser */\nint parse(const char *p) {\n return 0;\n}\n";
        let graph = graph_of_files(&[("p.h", header), ("p.c", source)]);
        let parse = id(&graph, "parse");
        let (file, comment) = symbol_comment(&graph, parse).expect("a comment");
        assert_eq!(file, "p.h");
        assert!(comment.is_doxygen);
        assert_eq!(
            symbol_brief(&graph, parse).as_deref(),
            Some("Parses a header.")
        );
    }

    #[test]
    fn a_heading_that_only_repeats_the_name_is_not_the_brief() {
        let doc = doc_of(
            "Curl_sasl_start()\n\nCalls the mechanism.\n\nMore.",
            "Curl_sasl_start",
        );
        assert_eq!(doc.brief, "Calls the mechanism.");
        assert_eq!(doc.details, ["More."]);
        let alone = doc_of("parse:", "parse");
        assert_eq!(alone.brief, "parse:");
        let other = doc_of("Parses it.\n\nMore.", "parse");
        assert_eq!(other.brief, "Parses it.");
    }

    #[test]
    fn a_plain_comment_is_used_when_there_is_no_doxygen_one() {
        let source = "/* Frees the buffer. */\nvoid release(void *p) {\n}\n";
        let graph = graph_of_files(&[("r.c", source)]);
        let release = id(&graph, "release");
        assert_eq!(
            symbol_brief(&graph, release).as_deref(),
            Some("Frees the buffer.")
        );
        let bare = graph_of_files(&[("b.c", "void bare(void) {\n}\n")]);
        assert_eq!(symbol_comment(&bare, id(&bare, "bare")), None);
    }
}
