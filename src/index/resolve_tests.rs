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
    let source = "void f(char *d, char *fmt) {\n  sprintf(d, fmt,\n    1);\n  puts(\"a, b\");\n}\n";
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
