//! Data model for the indexed codebase.
//!
//! A set of symbols (functions, classes, fields, ...) and typed edges between
//! them (calls, inherits, uses, contains), as produced by `crate::index`,
//! plus every place in the source where a symbol's name appears.

use std::collections::HashMap;

pub type SymbolId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Class,
    Struct,
    Function,
    Method,
    Field,
    Namespace,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Field => "field",
            SymbolKind::Namespace => "namespace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    Calls,
    Inherits,
    Uses,
    Contains,
}

impl EdgeKind {
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Calls => "call",
            EdgeKind::Inherits => "base class",
            EdgeKind::Uses => "use",
            EdgeKind::Contains => "member",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
}

/// A name's position on one source line: 1-based `line`, and byte columns
/// `start..end` within that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

#[cfg(test)]
impl Span {
    pub fn contains_column(&self, column: u32) -> bool {
        (self.start..self.end).contains(&column)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceRole {
    /// The symbol's own name where it is declared or defined.
    Declaration,
    /// A use of the symbol from inside another symbol.
    Reference(EdgeKind),
}

/// One appearance of a symbol's name in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub span: Span,
    pub symbol: SymbolId,
    pub role: OccurrenceRole,
    /// The symbol whose code contains this reference (`None` for declarations).
    pub context: Option<SymbolId>,
}

/// A call, written `f(...)` or `ns::f(...)`, to a function the project
/// doesn't define: the C library, the OS, third-party APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCall {
    /// The project symbol making the call.
    pub caller: SymbolId,
    /// The called name as written, e.g. `strcpy` or `std::memcpy`.
    pub name: String,
    /// Where the name is written.
    pub span: Span,
    /// The call from its name to its closing parenthesis if on one line,
    /// else the name.
    pub extent: Span,
    /// Each argument's source text, cut short if long.
    pub args: Vec<String>,
}

/// A string literal written in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLiteral {
    /// As written: prefix, quotes and escapes included; adjacent literals
    /// joined by a space; cut short (with `…`) if long.
    pub text: String,
    /// Where it starts; to the end of that line if it goes on.
    pub span: Span,
    /// The function it is in; `None` at file scope.
    pub function: Option<SymbolId>,
}

/// A comment worth a look: one telling an analyzer not to report, or a
/// TODO-style tag left in the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMark {
    pub kind: CommentMarkKind,
    /// Where the directive or tag is written.
    pub span: Span,
    /// The comment from the directive or tag to the end of its line,
    /// cut short (with `…`) if long.
    pub text: String,
    /// The function it is in; `None` at file scope.
    pub function: Option<SymbolId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CommentMarkKind {
    /// Tells this analyzer not to report here.
    Silences(Analyzer),
    Tag(SourceTag),
}

/// An analyzer whose findings a comment in the code can silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Analyzer {
    Semgrep,
    Flawfinder,
    Cppcheck,
    ClangTidy,
    Coverity,
    CodeQl,
    SonarQube,
    PvsStudio,
    Splint,
}

impl Analyzer {
    pub fn label(self) -> &'static str {
        match self {
            Analyzer::Semgrep => "semgrep",
            Analyzer::Flawfinder => "flawfinder",
            Analyzer::Cppcheck => "cppcheck",
            Analyzer::ClangTidy => "clang-tidy",
            Analyzer::Coverity => "Coverity",
            Analyzer::CodeQl => "CodeQL",
            Analyzer::SonarQube => "SonarQube",
            Analyzer::PvsStudio => "PVS-Studio",
            Analyzer::Splint => "Splint",
        }
    }
}

/// A tag marking work left or a known problem: `TODO`, `FIXME`, ...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceTag {
    Fixme,
    Bug,
    Xxx,
    Hack,
    Todo,
}

impl SourceTag {
    pub const ALL: [SourceTag; 5] = [
        SourceTag::Fixme,
        SourceTag::Bug,
        SourceTag::Xxx,
        SourceTag::Hack,
        SourceTag::Todo,
    ];

    /// As written in the code.
    pub fn label(self) -> &'static str {
        match self {
            SourceTag::Fixme => "FIXME",
            SourceTag::Bug => "BUG",
            SourceTag::Xxx => "XXX",
            SourceTag::Hack => "HACK",
            SourceTag::Todo => "TODO",
        }
    }
}

/// Size and branching of one function definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetrics {
    pub symbol: SymbolId,
    pub file: String,
    /// 1-based line where the definition starts.
    pub line: u32,
    /// Lines the definition covers, from its return type to its `}`.
    pub lines: u32,
    /// McCabe cyclomatic complexity: 1 plus one per branch point.
    pub complexity: u32,
    pub signature: Signature,
}

/// Parameter types longer than this are left empty (unknown).
pub const MAX_TYPE_CHARS: usize = 256;

/// How a function definition is declared, as written in the source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signature {
    /// Everything before the body, comments removed and whitespace
    /// collapsed, e.g. `static int parse(const uint8_t *buf, size_t len)`.
    /// Empty if it was too long to keep.
    pub prototype: String,
    pub params: Vec<Param>,
    /// Declared `static`: only callable from its own file (or, for a
    /// member function, callable without an object).
    pub is_static: bool,
    /// Takes `...` (or a C++ parameter pack).
    pub is_variadic: bool,
    /// Defined inside `template <…>`.
    pub is_template: bool,
}

/// One parameter of a function definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// The type as written without the name or default value, comments
    /// removed and whitespace collapsed: `const uint8_t *`, `size_t`.
    pub type_text: String,
    /// The parameter's name, if it has one.
    pub name: Option<String>,
}

/// An occurrence together with the file it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub occurrence: Occurrence,
}

#[derive(Debug, Clone, Default)]
pub struct CodeGraph {
    pub symbols: HashMap<SymbolId, Symbol>,
    pub edges: Vec<Edge>,
    /// Source text per file, keyed by project-relative path, for the code view.
    pub file_contents: HashMap<String, String>,
    /// Occurrences per file, sorted by span.
    pub occurrences: HashMap<String, Vec<Occurrence>>,
    /// Calls to functions outside the project, per file, sorted by span.
    pub external_calls: HashMap<String, Vec<ExternalCall>>,
    /// Every function definition's size and complexity.
    pub functions: Vec<FunctionMetrics>,
    /// String literals per file (files with any), in file order.
    pub strings: HashMap<String, Vec<StringLiteral>>,
    /// Files with more strings than were kept.
    pub strings_cut_short: Vec<String>,
    /// Comments silencing analyzers and TODO-style tags, per file (files
    /// with any), in file order.
    pub comment_marks: HashMap<String, Vec<CommentMark>>,
    /// Files with more TODO-style tags than were kept.
    pub tags_cut_short: Vec<String>,
    /// Files with more comments silencing analyzers than were kept.
    pub silencers_cut_short: Vec<String>,
}

impl CodeGraph {
    pub fn edges_for(&self, id: SymbolId) -> impl Iterator<Item = &Edge> {
        self.edges
            .iter()
            .filter(move |e| e.from == id || e.to == id)
    }

    /// Occurrences on one line of `file`, in column order.
    pub fn occurrences_on_line(&self, file: &str, line: u32) -> &[Occurrence] {
        let Some(all) = self.occurrences.get(file) else {
            return &[];
        };
        let first = all.partition_point(|o| o.span.line < line);
        let past = all.partition_point(|o| o.span.line <= line);
        &all[first..past]
    }

    /// Calls out of the project on one line of `file`, in column order.
    pub fn external_calls_on_line(&self, file: &str, line: u32) -> &[ExternalCall] {
        let Some(all) = self.external_calls.get(file) else {
            return &[];
        };
        let first = all.partition_point(|c| c.span.line < line);
        let past = all.partition_point(|c| c.span.line <= line);
        &all[first..past]
    }

    /// Every place `id` appears, ordered by file, then position.
    pub fn locations_of(&self, id: SymbolId) -> Vec<Location> {
        let mut locations: Vec<Location> = self
            .occurrences
            .iter()
            .flat_map(|(file, list)| {
                list.iter().filter(|o| o.symbol == id).map(|o| Location {
                    file: file.clone(),
                    occurrence: *o,
                })
            })
            .collect();
        locations.sort_by(|a, b| {
            (a.file.as_str(), a.occurrence.span).cmp(&(b.file.as_str(), b.occurrence.span))
        });
        locations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occurrence(line: u32, start: u32, symbol: SymbolId) -> Occurrence {
        Occurrence {
            span: Span {
                line,
                start,
                end: start + 3,
            },
            symbol,
            role: OccurrenceRole::Reference(EdgeKind::Calls),
            context: Some(99),
        }
    }

    fn graph_with_occurrences() -> CodeGraph {
        let occurrences = HashMap::from([
            (
                "b.cc".to_string(),
                vec![
                    occurrence(1, 0, 7),
                    occurrence(4, 2, 7),
                    occurrence(4, 9, 8),
                ],
            ),
            ("a.cc".to_string(), vec![occurrence(10, 5, 7)]),
        ]);
        CodeGraph {
            occurrences,
            ..CodeGraph::default()
        }
    }

    #[test]
    fn edges_for_covers_both_directions() {
        let edge = |from, to| Edge {
            from,
            to,
            kind: EdgeKind::Calls,
        };
        let graph = CodeGraph {
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3)],
            ..CodeGraph::default()
        };
        let touching: Vec<(SymbolId, SymbolId)> =
            graph.edges_for(1).map(|e| (e.from, e.to)).collect();
        assert_eq!(touching, [(0, 1), (1, 2)]);
    }

    #[test]
    fn occurrences_on_line_returns_only_that_line() {
        let graph = graph_with_occurrences();
        let on_four: Vec<u32> = graph
            .occurrences_on_line("b.cc", 4)
            .iter()
            .map(|o| o.span.start)
            .collect();
        assert_eq!(on_four, [2, 9]);
        assert!(graph.occurrences_on_line("b.cc", 2).is_empty());
        assert!(graph.occurrences_on_line("missing.cc", 1).is_empty());
    }

    #[test]
    fn locations_of_are_sorted_by_file_then_position() {
        let graph = graph_with_occurrences();
        let found: Vec<(String, u32)> = graph
            .locations_of(7)
            .into_iter()
            .map(|l| (l.file, l.occurrence.span.line))
            .collect();
        assert_eq!(
            found,
            [("a.cc".into(), 10), ("b.cc".into(), 1), ("b.cc".into(), 4)]
        );
        assert!(graph.locations_of(42).is_empty());
    }

    #[test]
    fn span_contains_its_start_but_not_its_end() {
        let span = Span {
            line: 1,
            start: 4,
            end: 7,
        };
        assert!(span.contains_column(4) && span.contains_column(6));
        assert!(!span.contains_column(7) && !span.contains_column(3));
    }
}
