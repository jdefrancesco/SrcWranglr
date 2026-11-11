//! Cross references, as in IDA: every place a symbol is referenced (or a
//! library function is called), with what kind of reference it is and the
//! function it is in.

use crate::analysis::risky::bare_library_name;
use crate::code_view::Referenced;
use crate::model::{CodeGraph, OccurrenceRole, Span, SymbolId};

/// References listed at most.
pub const MAX_XREFS: usize = 200_000;

/// One reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xref {
    /// "call", "use", "declaration", …
    pub kind: &'static str,
    pub file: String,
    pub span: Span,
    /// The function (or other symbol) whose code holds it.
    pub function: Option<SymbolId>,
}

/// Every reference to `target`, by file and place; and whether there were
/// more than are listed.
pub fn xrefs(graph: &CodeGraph, target: &Referenced) -> (Vec<Xref>, bool) {
    let mut found: Vec<Xref> = match target {
        Referenced::Symbol(id) => graph
            .locations_of(*id)
            .into_iter()
            .map(|location| Xref {
                kind: match location.occurrence.role {
                    OccurrenceRole::Declaration => "declaration",
                    OccurrenceRole::Reference(kind) => kind.label(),
                },
                file: location.file,
                span: location.occurrence.span,
                function: location.occurrence.context,
            })
            .collect(),
        Referenced::Library(name) => graph
            .external_calls
            .iter()
            .flat_map(|(file, calls)| calls.iter().map(move |call| (file, call)))
            .filter(|(_, call)| bare_library_name(&call.name) == Some(name.as_str()))
            .map(|(file, call)| Xref {
                kind: "call",
                file: file.clone(),
                span: call.span,
                function: Some(call.caller),
            })
            .collect(),
    };
    found.sort_by(|a, b| (&a.file, a.span).cmp(&(&b.file, b.span)));
    let is_cut_short = found.len() > MAX_XREFS;
    found.truncate(MAX_XREFS);
    (found, is_cut_short)
}

/// What `target` is called, for the window's title.
pub fn name_of(graph: &CodeGraph, target: &Referenced) -> String {
    match target {
        Referenced::Symbol(id) => graph
            .symbols
            .get(id)
            .map_or_else(|| "?".to_string(), |s| s.qualified_name.clone()),
        Referenced::Library(name) => format!("{name}()"),
    }
}

/// What a name on `line` of `file` refers to: the project's symbol of that
/// name there, else the library function.
pub fn referenced_by_name(graph: &CodeGraph, file: &str, line: u32, name: &str) -> Referenced {
    let symbol = graph
        .occurrences_on_line(file, line)
        .iter()
        .filter_map(|o| graph.symbols.get(&o.symbol))
        .find(|s| s.name == name || s.qualified_name == name);
    match symbol {
        Some(symbol) => Referenced::Symbol(symbol.id),
        None => Referenced::Library(bare_library_name(name).unwrap_or(name).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::graph_of_files;

    const SOURCE: &str = "\
int helper(int x) { return x; }
void a(char *d, char *s) { memcpy(d, s, helper(1)); }
void b(char *d, char *s) { helper(2); memcpy(d, s, 4); }
";

    fn graph() -> CodeGraph {
        graph_of_files(&[("x.c", SOURCE)])
    }

    #[test]
    fn a_symbols_references_say_what_they_are_and_where() {
        let graph = graph();
        let helper = referenced_by_name(&graph, "x.c", 2, "helper");
        let (found, is_cut_short) = xrefs(&graph, &helper);
        assert!(!is_cut_short);
        let kinds: Vec<(&str, u32)> = found.iter().map(|x| (x.kind, x.span.line)).collect();
        assert_eq!(kinds, [("declaration", 1), ("call", 2), ("call", 3)]);
        let a = graph
            .symbols
            .values()
            .find(|s| s.name == "a")
            .expect("a")
            .id;
        assert_eq!(found[1].function, Some(a));
        assert_eq!(name_of(&graph, &helper), "helper");
    }

    #[test]
    fn a_library_functions_references_are_its_calls() {
        let graph = graph();
        let memcpy = referenced_by_name(&graph, "x.c", 2, "memcpy");
        assert_eq!(memcpy, Referenced::Library("memcpy".into()));
        let lines: Vec<u32> = xrefs(&graph, &memcpy)
            .0
            .iter()
            .map(|x| x.span.line)
            .collect();
        assert_eq!(lines, [2, 3]);
        assert_eq!(name_of(&graph, &memcpy), "memcpy()");
    }
}
