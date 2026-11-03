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
use crate::model::{CodeGraph, Edge, EdgeKind, Symbol, SymbolId, SymbolKind};

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
    let resolver = Resolver::new(&symbols, &by_qname, &bases);

    let containment = symbols.values().filter_map(|s| {
        let parent = by_qname.get(parent_of(&s.qualified_name)?)?;
        Some(Edge {
            from: *parent,
            to: s.id,
            kind: EdgeKind::Contains,
        })
    });
    let references = refs.iter().filter_map(|r| {
        let from = *by_qname.get(r.from.as_str())?;
        let to = resolver.resolve(r)?;
        Some(Edge {
            from,
            to,
            kind: r.kind,
        })
    });
    let edges = dedupe_edges(containment.chain(references), &symbols);

    let file_contents = files.into_iter().map(|f| (f.path, f.contents)).collect();
    CodeGraph {
        symbols,
        edges,
        file_contents,
    }
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
        let (Some(from), Some(base)) = (by_qname.get(r.from.as_str()), resolver.resolve(r)) else {
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

    #[test]
    fn empty_input_gives_empty_graph() {
        let graph = build_graph(Vec::new());
        assert!(graph.symbols.is_empty() && graph.edges.is_empty());
    }
}
