//! Cross-file resolution: merge per-file declarations into one symbol table
//! and turn name-based references into typed edges.
//!
//! Resolution order for a reference made from `a::B::m` to `x`:
//! 1. enclosing scopes, innermost first: `a::B::x` (then `a::B`'s base
//!    classes), `a::x`, `x`
//! 2. for `Lookup::ScopedOrUnique` only: the single symbol whose qualified
//!    name ends in `x`, if exactly one exists. Ambiguous names stay unlinked.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use super::extract::{Decl, FileFacts, Lookup, Reference};
use crate::model::{
    CodeGraph, Edge, EdgeKind, ExternalCall, FunctionMetrics, Occurrence, OccurrenceRole, Symbol,
    SymbolId, SymbolKind,
};

/// A reference whose source and target symbols are both known.
struct Resolved<'f> {
    reference: &'f Reference,
    from: SymbolId,
    to: SymbolId,
}

/// One parsed file, ready for resolution.
pub struct SourceFile {
    /// Project-relative path, used as the display name and code-view key.
    pub path: String,
    pub contents: String,
    pub facts: FileFacts,
}

pub fn build_graph(files: Vec<SourceFile>) -> CodeGraph {
    let merged = merge_decls(&files);
    let symbols = assign_symbols(&merged);
    let by_qname: HashMap<&str, SymbolId> = symbols
        .values()
        .map(|s| (s.qualified_name.as_str(), s.id))
        .collect();

    let refs: Vec<&Reference> = files.iter().flat_map(|f| &f.facts.refs).collect();
    let bases = resolve_bases(&refs, &symbols, &by_qname);
    let mut resolver = MemoResolver::new(Resolver::new(&symbols, &by_qname, &bases));
    let resolved: Vec<Vec<Resolved>> = files
        .iter()
        .map(|f| resolve_file(&f.facts, &by_qname, &mut resolver))
        .collect();

    let containment = symbols.values().filter_map(|s| {
        let parent = by_qname.get(parent_of(&s.qualified_name)?)?;
        Some(Edge {
            from: *parent,
            to: s.id,
            kind: EdgeKind::Contains,
        })
    });
    let references = resolved.iter().flatten().map(|r| Edge {
        from: r.from,
        to: r.to,
        kind: r.reference.kind,
    });
    let edges = dedupe_edges(containment.chain(references), &symbols);
    let occurrences = files
        .iter()
        .zip(&resolved)
        .filter_map(|(file, refs)| {
            let list = file_occurrences(&file.facts, refs, &by_qname);
            (!list.is_empty()).then(|| (file.path.clone(), list))
        })
        .collect();

    let external_calls = files
        .iter()
        .filter_map(|file| {
            let calls = external_calls_in(&file.facts, &by_qname, &mut resolver);
            (!calls.is_empty()).then(|| (file.path.clone(), calls))
        })
        .collect();

    let functions = function_metrics(&files, &by_qname);
    let file_contents = files.into_iter().map(|f| (f.path, f.contents)).collect();
    CodeGraph {
        symbols,
        edges,
        file_contents,
        occurrences,
        external_calls,
        functions,
    }
}

/// Each function definition's metrics, in file order.
fn function_metrics(
    files: &[SourceFile],
    by_qname: &HashMap<&str, SymbolId>,
) -> Vec<FunctionMetrics> {
    files
        .iter()
        .flat_map(|file| {
            file.facts.functions.iter().filter_map(|f| {
                Some(FunctionMetrics {
                    symbol: *by_qname.get(f.qualified_name.as_str())?,
                    file: file.path.clone(),
                    line: f.line,
                    lines: f.lines,
                    complexity: f.complexity,
                })
            })
        })
        .collect()
}

fn resolve_file<'f>(
    facts: &'f FileFacts,
    by_qname: &HashMap<&str, SymbolId>,
    resolver: &mut MemoResolver<'_, 'f>,
) -> Vec<Resolved<'f>> {
    facts
        .refs
        .iter()
        .filter_map(|reference| {
            let from = *by_qname.get(&*reference.from)?;
            let to = resolver.resolve(reference)?;
            Some(Resolved {
                reference,
                from,
                to,
            })
        })
        .collect()
}

/// Plain function calls in one file that resolve to no project symbol,
/// sorted by position.
fn external_calls_in<'f>(
    facts: &'f FileFacts,
    by_qname: &HashMap<&str, SymbolId>,
    resolver: &mut MemoResolver<'_, 'f>,
) -> Vec<ExternalCall> {
    let mut calls: Vec<ExternalCall> = facts
        .refs
        .iter()
        .filter_map(|r| {
            let site = r.free_call.as_ref()?;
            let caller = *by_qname.get(&*r.from)?;
            let span = r.span?;
            resolver.resolve(r).is_none().then(|| ExternalCall {
                caller,
                name: r.target.clone(),
                span,
                extent: site.extent.unwrap_or(span),
                args: site.args.clone(),
            })
        })
        .collect();
    calls.sort_by_key(|c| c.span);
    calls
}

/// Declared names and resolved references in one file, sorted by position.
/// Unlike edges, self-references (recursion) and uses of an enclosing class
/// are kept: they are still clickable names in the source.
fn file_occurrences(
    facts: &FileFacts,
    refs: &[Resolved],
    by_qname: &HashMap<&str, SymbolId>,
) -> Vec<Occurrence> {
    let declarations = facts.decls.iter().filter_map(|d| {
        Some(Occurrence {
            span: d.name_span?,
            symbol: *by_qname.get(d.qualified_name.as_str())?,
            role: OccurrenceRole::Declaration,
            context: None,
        })
    });
    let references = refs.iter().filter_map(|r| {
        Some(Occurrence {
            span: r.reference.span?,
            symbol: r.to,
            role: OccurrenceRole::Reference(r.reference.kind),
            context: Some(r.from),
        })
    });
    let mut list: Vec<Occurrence> = declarations.chain(references).collect();
    list.sort_by_key(|o| (o.span, o.symbol));
    list.dedup_by_key(|o| (o.span, o.symbol));
    list
}

fn parent_of(qualified: &str) -> Option<&str> {
    qualified.rsplit_once("::").map(|(parent, _)| parent)
}

fn last_segment(qualified: &str) -> &str {
    qualified
        .rsplit_once("::")
        .map_or(qualified, |(_, name)| name)
}

/// One entry per qualified name. Definitions win over declarations; among
/// equals, the first file (in input order) wins.
fn merge_decls(files: &[SourceFile]) -> BTreeMap<&str, (&Decl, &str)> {
    let mut merged: BTreeMap<&str, (&Decl, &str)> = BTreeMap::new();
    for file in files {
        for decl in &file.facts.decls {
            let entry = merged
                .entry(decl.qualified_name.as_str())
                .or_insert((decl, &file.path));
            if decl.is_definition && !entry.0.is_definition {
                *entry = (decl, &file.path);
            }
        }
    }
    merged
}

/// Ids follow sorted qualified-name order, so they're stable across runs.
/// Functions whose parent is a class/struct (out-of-line definitions such as
/// `void Foo::bar() {}`) become methods.
fn assign_symbols(merged: &BTreeMap<&str, (&Decl, &str)>) -> HashMap<SymbolId, Symbol> {
    let is_type = |name: &str| {
        merged
            .get(name)
            .is_some_and(|(d, _)| matches!(d.kind, SymbolKind::Class | SymbolKind::Struct))
    };
    (0..)
        .zip(merged.iter())
        .map(|(id, (qualified, (decl, file)))| {
            let is_member_function =
                decl.kind == SymbolKind::Function && parent_of(qualified).is_some_and(is_type);
            let kind = if is_member_function {
                SymbolKind::Method
            } else {
                decl.kind
            };
            let symbol = Symbol {
                id,
                name: last_segment(qualified).to_string(),
                qualified_name: qualified.to_string(),
                kind,
                file: file.to_string(),
                line: decl.line,
            };
            (id, symbol)
        })
        .collect()
}

/// Class → resolved direct base classes. Bases resolve via scopes and
/// unique names only (no base-class lookup, which needs this very map).
fn resolve_bases(
    refs: &[&Reference],
    symbols: &HashMap<SymbolId, Symbol>,
    by_qname: &HashMap<&str, SymbolId>,
) -> HashMap<SymbolId, Vec<SymbolId>> {
    let no_bases = HashMap::new();
    let resolver = Resolver::new(symbols, by_qname, &no_bases);
    let mut bases: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
    for r in refs.iter().filter(|r| r.kind == EdgeKind::Inherits) {
        let (Some(from), Some(base)) = (by_qname.get(&*r.from), resolver.resolve(r)) else {
            continue;
        };
        if *from != base {
            bases.entry(*from).or_default().push(base);
        }
    }
    bases
}

/// Drops duplicates, self-loops, and references from a symbol to one of its
/// own enclosing scopes (e.g. a constructor "using" its class).
fn dedupe_edges(
    edges: impl Iterator<Item = Edge>,
    symbols: &HashMap<SymbolId, Symbol>,
) -> Vec<Edge> {
    let is_ancestor = |ancestor: SymbolId, of: SymbolId| {
        let (Some(a), Some(o)) = (symbols.get(&ancestor), symbols.get(&of)) else {
            return false;
        };
        o.qualified_name
            .strip_prefix(a.qualified_name.as_str())
            .is_some_and(|rest| rest.starts_with("::"))
    };
    let mut seen = HashSet::new();
    edges
        .filter(|e| e.from != e.to)
        .filter(|e| e.kind == EdgeKind::Contains || !is_ancestor(e.to, e.from))
        .filter(|e| seen.insert((e.from, e.to, e.kind)))
        .collect()
}

/// Caches lookups: the same name is usually referenced many times from the
/// same symbol, and resolution only depends on `(from, target, lookup)`.
struct MemoResolver<'a, 'f> {
    resolver: Resolver<'a>,
    cache: HashMap<(&'f str, &'f str, Lookup), Option<SymbolId>>,
}

impl<'a, 'f> MemoResolver<'a, 'f> {
    fn new(resolver: Resolver<'a>) -> Self {
        Self {
            resolver,
            cache: HashMap::new(),
        }
    }

    fn resolve(&mut self, r: &'f Reference) -> Option<SymbolId> {
        let key = (&*r.from, r.target.as_str(), r.lookup);
        *self
            .cache
            .entry(key)
            .or_insert_with(|| self.resolver.resolve(r))
    }
}

struct Resolver<'a> {
    symbols: &'a HashMap<SymbolId, Symbol>,
    by_qname: &'a HashMap<&'a str, SymbolId>,
    by_name: HashMap<&'a str, Vec<SymbolId>>,
    bases: &'a HashMap<SymbolId, Vec<SymbolId>>,
}

impl<'a> Resolver<'a> {
    fn new(
        symbols: &'a HashMap<SymbolId, Symbol>,
        by_qname: &'a HashMap<&'a str, SymbolId>,
        bases: &'a HashMap<SymbolId, Vec<SymbolId>>,
    ) -> Self {
        let mut by_name: HashMap<&str, Vec<SymbolId>> = HashMap::new();
        for symbol in symbols.values() {
            by_name
                .entry(symbol.name.as_str())
                .or_default()
                .push(symbol.id);
        }
        Self {
            symbols,
            by_qname,
            by_name,
            bases,
        }
    }

    fn resolve(&self, r: &Reference) -> Option<SymbolId> {
        self.resolve_scoped(&r.from, &r.target)
            .or_else(|| match r.lookup {
                Lookup::ScopedOrUnique => self.resolve_unique(&r.target),
                Lookup::Scoped => None,
            })
    }

    /// `target` looked up in each enclosing scope of `from`, innermost first,
    /// including members inherited by enclosing classes.
    fn resolve_scoped(&self, from: &str, target: &str) -> Option<SymbolId> {
        let mut scope = parent_of(from);
        while let Some(s) = scope {
            if let Some(id) = self.by_qname.get(format!("{s}::{target}").as_str()) {
                return Some(*id);
            }
            let inherited = self
                .by_qname
                .get(s)
                .and_then(|id| self.find_in_bases(*id, target));
            if inherited.is_some() {
                return inherited;
            }
            scope = parent_of(s);
        }
        // Global scope, then `Type::member` where the member is inherited.
        self.by_qname
            .get(target)
            .copied()
            .or_else(|| self.resolve_inherited_member(from, target))
    }

    /// `Derived::bump` → `Base::bump` when `bump` is declared in a base of `Derived`.
    fn resolve_inherited_member(&self, from: &str, target: &str) -> Option<SymbolId> {
        let (qualifier, member) = target.rsplit_once("::")?;
        let owner = self
            .resolve_scoped(from, qualifier)
            .or_else(|| self.resolve_unique(qualifier))?;
        self.find_in_bases(owner, member)
    }

    /// Breadth-first through base classes (guards against inheritance cycles).
    fn find_in_bases(&self, class: SymbolId, member: &str) -> Option<SymbolId> {
        let mut queue: VecDeque<SymbolId> = self
            .bases
            .get(&class)
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let mut visited = HashSet::from([class]);
        while let Some(base) = queue.pop_front() {
            if !visited.insert(base) {
                continue;
            }
            let base_name = &self.symbols.get(&base)?.qualified_name;
            if let Some(id) = self.by_qname.get(format!("{base_name}::{member}").as_str()) {
                return Some(*id);
            }
            queue.extend(self.bases.get(&base).into_iter().flatten());
        }
        None
    }

    /// The only symbol named `target` (or ending in `::target`), if unique.
    fn resolve_unique(&self, target: &str) -> Option<SymbolId> {
        let suffix = format!("::{target}");
        let mut matches = self.by_name.get(last_segment(target))?.iter().filter(|id| {
            self.symbols
                .get(id)
                .is_some_and(|s| s.qualified_name == target || s.qualified_name.ends_with(&suffix))
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(*first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::extract::Extractor;

    fn graph_of(sources: &[(&str, &str)]) -> CodeGraph {
        let mut extractor = Extractor::new().expect("grammar");
        let files = sources
            .iter()
            .map(|(path, contents)| SourceFile {
                path: path.to_string(),
                contents: contents.to_string(),
                facts: extractor.extract(contents).expect("parse"),
            })
            .collect();
        build_graph(files)
    }

    fn id(graph: &CodeGraph, qualified: &str) -> SymbolId {
        graph
            .symbols
            .values()
            .find(|s| s.qualified_name == qualified)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("no symbol {qualified}"))
    }

    fn has_edge(graph: &CodeGraph, from: &str, to: &str, kind: EdgeKind) -> bool {
        let (from, to) = (id(graph, from), id(graph, to));
        graph
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.kind == kind)
    }

    const SHAPES: &[(&str, &str)] = &[
        (
            "shape.h",
            "class Shape { public: virtual double area() const = 0; };\n",
        ),
        (
            "circle.h",
            "class Circle : public Shape {\n\
             public:\n\
                 double area() const override { return radius * radius; }\n\
             private:\n\
                 double radius;\n\
             };\n",
        ),
        (
            "main.cpp",
            "double total(const Circle& c) { return c.area(); }\n\
             int main() { Circle c; return total(c); }\n",
        ),
    ];

    #[test]
    fn links_inheritance_containment_calls_and_uses() {
        let graph = graph_of(SHAPES);
        assert!(has_edge(&graph, "Circle", "Shape", EdgeKind::Inherits));
        assert!(has_edge(
            &graph,
            "Circle",
            "Circle::area",
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &graph,
            "Circle",
            "Circle::radius",
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &graph,
            "Circle::area",
            "Circle::radius",
            EdgeKind::Uses
        ));
        assert!(has_edge(&graph, "total", "Circle::area", EdgeKind::Calls));
        assert!(has_edge(&graph, "main", "total", EdgeKind::Calls));
        assert!(has_edge(&graph, "total", "Circle", EdgeKind::Uses));
    }

    #[test]
    fn records_files_lines_and_contents() {
        let graph = graph_of(SHAPES);
        let radius = &graph.symbols[&id(&graph, "Circle::radius")];
        assert_eq!((radius.file.as_str(), radius.line), ("circle.h", 5));
        assert_eq!(radius.name, "radius");
        assert!(graph.file_contents["main.cpp"].contains("int main()"));
    }

    #[test]
    fn merges_declaration_and_definition_preferring_definition() {
        let graph = graph_of(&[
            (
                "sq.h",
                "class Square {\n  int area2() const;\n  int side;\n};\n",
            ),
            ("sq.cpp", "\n\nint Square::area2() const { return side; }\n"),
        ]);
        let matches: Vec<&Symbol> = graph
            .symbols
            .values()
            .filter(|s| s.qualified_name == "Square::area2")
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].file.as_str(), matches[0].line), ("sq.cpp", 3));
        assert_eq!(matches[0].kind, SymbolKind::Method);
        assert!(has_edge(
            &graph,
            "Square::area2",
            "Square::side",
            EdgeKind::Uses
        ));
    }

    #[test]
    fn out_of_line_function_in_namespace_stays_a_function() {
        let graph = graph_of(&[(
            "a.cpp",
            "namespace util { void go(); }\nvoid util::go() {}\n",
        )]);
        assert_eq!(
            graph.symbols[&id(&graph, "util::go")].kind,
            SymbolKind::Function
        );
        assert!(has_edge(&graph, "util", "util::go", EdgeKind::Contains));
    }

    #[test]
    fn resolves_members_inherited_from_base_classes() {
        let graph = graph_of(&[(
            "a.h",
            "struct Base { int count; void bump(); };\n\
             struct Derived : Base { void f() { count++; bump(); } };\n",
        )]);
        assert!(has_edge(
            &graph,
            "Derived::f",
            "Base::count",
            EdgeKind::Uses
        ));
        assert!(has_edge(
            &graph,
            "Derived::f",
            "Base::bump",
            EdgeKind::Calls
        ));
    }

    #[test]
    fn resolves_inherited_members_through_typed_receivers() {
        let graph = graph_of(&[(
            "a.cpp",
            "struct Base { void bump(); };\nstruct Derived : Base {};\n\
             void f() { Derived d; d.bump(); }\n",
        )]);
        assert!(has_edge(&graph, "f", "Base::bump", EdgeKind::Calls));
    }

    #[test]
    fn prefers_innermost_scope() {
        let graph = graph_of(&[(
            "a.cpp",
            "void run() {}\nnamespace app { void run() {} void start() { run(); } }\n",
        )]);
        assert!(has_edge(&graph, "app::start", "app::run", EdgeKind::Calls));
        assert!(!has_edge(&graph, "app::start", "run", EdgeKind::Calls));
    }

    #[test]
    fn leaves_ambiguous_names_unlinked() {
        let graph = graph_of(&[(
            "a.cpp",
            "struct A { void draw(); };\nstruct B { void draw(); };\n\
             void f() { thing().draw(); }\n",
        )]);
        let f = id(&graph, "f");
        assert!(!graph
            .edges
            .iter()
            .any(|e| e.from == f && e.kind == EdgeKind::Calls));
    }

    #[test]
    fn links_unique_name_for_unknown_receiver() {
        let graph = graph_of(&[(
            "a.cpp",
            "struct View { void render(); };\nvoid f() { make().render(); }\n",
        )]);
        assert!(has_edge(&graph, "f", "View::render", EdgeKind::Calls));
    }

    #[test]
    fn skips_self_edges_and_duplicates() {
        let graph = graph_of(&[
            (
                "a.cpp",
                "int fact(int n) { return n ? n * fact(n - 1) : 1; }\nvoid g() { fact(1); }\n",
            ),
            ("b.cpp", "void g();\nvoid h() { g(); g(); }\n"),
        ]);
        let fact = id(&graph, "fact");
        assert!(!graph.edges.iter().any(|e| e.from == fact && e.to == fact));
        let mut seen = HashSet::new();
        assert!(graph
            .edges
            .iter()
            .all(|e| seen.insert((e.from, e.to, e.kind))));
    }

    #[test]
    fn assigns_ids_deterministically() {
        let a = graph_of(SHAPES);
        let b = graph_of(SHAPES);
        let names = |g: &CodeGraph| {
            let mut v: Vec<(SymbolId, String)> = g
                .symbols
                .values()
                .map(|s| (s.id, s.qualified_name.clone()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(&a), names(&b));
        assert_eq!(a.edges.len(), b.edges.len());
    }

    fn occurrence_at(graph: &CodeGraph, file: &str, line: u32, column: u32) -> Option<Occurrence> {
        graph
            .occurrences_on_line(file, line)
            .iter()
            .find(|o| o.span.contains_column(column))
            .copied()
    }

    #[test]
    fn records_every_call_site_as_an_occurrence() {
        let graph = graph_of(SHAPES);
        // "int main() { Circle c; return total(c); }" — `total` at column 30.
        let call = occurrence_at(&graph, "main.cpp", 2, 31).expect("call to total");
        assert_eq!(call.symbol, id(&graph, "total"));
        assert_eq!(call.role, OccurrenceRole::Reference(EdgeKind::Calls));
        assert_eq!(call.context, Some(id(&graph, "main")));
    }

    #[test]
    fn records_declarations_in_every_file_for_the_merged_symbol() {
        let graph = graph_of(&[
            ("sq.h", "class Square {\n  int area2() const;\n};\n"),
            ("sq.cpp", "int Square::area2() const { return 1; }\n"),
        ]);
        let area2 = id(&graph, "Square::area2");
        let files: Vec<String> = graph
            .locations_of(area2)
            .into_iter()
            .filter(|l| l.occurrence.role == OccurrenceRole::Declaration)
            .map(|l| l.file)
            .collect();
        assert_eq!(files, ["sq.cpp", "sq.h"]);
    }

    #[test]
    fn keeps_recursive_calls_as_occurrences_but_not_edges() {
        let graph = graph_of(&[("a.cpp", "int fact(int n) { return fact(n - 1); }\n")]);
        let fact = id(&graph, "fact");
        let recursive = occurrence_at(&graph, "a.cpp", 1, 26).expect("recursive call");
        assert_eq!((recursive.symbol, recursive.context), (fact, Some(fact)));
        assert!(!graph.edges.iter().any(|e| e.from == fact && e.to == fact));
    }

    #[test]
    fn unresolved_free_calls_are_kept_as_external_calls() {
        let source = "void helper() {}\n\
            struct Sock { void write(int); };\n\
            void f(char *d, const char *s, Sock obj) {\n\
            \x20 strcpy(d, s); helper(); obj.write(1); std::memcpy(d, s, 1);\n\
            }\n";
        let graph = graph_of(&[("a.c", source)]);
        let f = id(&graph, "f");
        let calls: Vec<(&str, SymbolId, u32, u32)> = graph.external_calls["a.c"]
            .iter()
            .map(|c| (c.name.as_str(), c.caller, c.span.line, c.span.start))
            .collect();
        // helper() resolves; obj.write() is a member call, never external.
        assert_eq!(calls, [("strcpy", f, 4, 2), ("std::memcpy", f, 4, 45)]);
    }

    #[test]
    fn external_calls_keep_their_arguments_and_extent() {
        let source =
            "void f(char *d, char *fmt) {\n  sprintf(d, fmt,\n    1);\n  puts(\"a, b\");\n}\n";
        let graph = graph_of(&[("a.c", source)]);
        let calls = &graph.external_calls["a.c"];
        let sprintf = &calls[0];
        assert_eq!(sprintf.args, ["d", "fmt", "1"]);
        // Spans lines: the extent is just the name.
        assert_eq!((sprintf.extent.start, sprintf.extent.end), (2, 9));
        let puts = &calls[1];
        assert_eq!(puts.args, ["\"a, b\""]);
        // On one line: the extent covers the whole call.
        assert_eq!(
            (puts.extent.line, puts.extent.start, puts.extent.end),
            (4, 2, 14)
        );
    }

    #[test]
    fn unresolved_names_have_no_occurrence() {
        let graph = graph_of(&[("a.cpp", "void f() { unknown(); }\n")]);
        assert!(occurrence_at(&graph, "a.cpp", 1, 12).is_none());
    }

    /// Every occurrence's span, cut out of its source line, is the symbol's name.
    fn assert_spans_match_names(graph: &CodeGraph) {
        let mut checked = 0;
        for (file, occurrences) in &graph.occurrences {
            let lines: Vec<&str> = graph.file_contents[file].split('\n').collect();
            for o in occurrences {
                let line = lines[o.span.line as usize - 1].trim_end_matches('\r');
                let text = line.get(o.span.start as usize..o.span.end as usize);
                assert_eq!(
                    text,
                    Some(graph.symbols[&o.symbol].name.as_str()),
                    "{file}:{o:?}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn spans_are_byte_columns_on_non_ascii_tabbed_crlf_lines() {
        let graph = graph_of(&[(
            "u.cpp",
            "// naïve → café\r\nint helper() { return 1; }\r\n\
             \tint f() { const char* s = \"ünï → \"; return helper() + helper(); }\r\n",
        )]);
        assert_spans_match_names(&graph);
        let call = occurrence_at(&graph, "u.cpp", 3, 50).expect("call after non-ASCII");
        assert_eq!(call.symbol, id(&graph, "helper"));
    }

    #[test]
    fn demo_spans_match_symbol_names() {
        assert_spans_match_names(&crate::demo::demo_codebase().expect("demo"));
    }

    #[test]
    fn records_each_function_definitions_complexity() {
        let graph = graph_of(&[(
            "a.c",
            "int pick(int a) {\n  if (a > 1 && a < 9) return 1;\n  return 0;\n}\nint one(void) { return 1; }\n",
        )]);
        let metrics: Vec<(&str, u32, u32, u32)> = graph
            .functions
            .iter()
            .map(|f| {
                (
                    graph.symbols[&f.symbol].name.as_str(),
                    f.line,
                    f.lines,
                    f.complexity,
                )
            })
            .collect();
        assert_eq!(metrics, vec![("pick", 1, 4, 3), ("one", 5, 1, 1)]);
        assert!(graph.functions.iter().all(|f| f.file == "a.c"));
    }

    #[test]
    fn empty_input_gives_empty_graph() {
        let graph = build_graph(Vec::new());
        assert!(graph.symbols.is_empty() && graph.edges.is_empty());
        assert!(graph.occurrences.is_empty());
    }
}
