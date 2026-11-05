//! Tests for the declaration and reference extractor.

use super::*;

fn extract(source: &str) -> FileFacts {
    Extractor::new()
        .expect("grammar")
        .extract(source)
        .expect("parse")
}

fn decl<'a>(facts: &'a FileFacts, qualified: &str) -> &'a Decl {
    facts
        .decls
        .iter()
        .find(|d| d.qualified_name == qualified)
        .unwrap_or_else(|| panic!("no decl {qualified} in {:#?}", facts.decls))
}

fn has_ref(facts: &FileFacts, from: &str, target: &str, kind: EdgeKind) -> bool {
    facts
        .refs
        .iter()
        .any(|r| &*r.from == from && r.target == target && r.kind == kind)
}

const CIRCLE: &str = "class Circle : public Shape {\n\
    public:\n\
        explicit Circle(double r) : radius(r) {}\n\
        virtual ~Circle() = default;\n\
        double area() const override { return 3.14 * radius * radius; }\n\
        virtual double perim() const = 0;\n\
    private:\n\
        double radius;\n\
        Point center;\n\
    };\n";

#[test]
fn extracts_class_with_base_and_members() {
    let facts = extract(CIRCLE);

    let circle = decl(&facts, "Circle");
    assert_eq!(circle.kind, SymbolKind::Class);
    assert_eq!(circle.line, 1);
    assert!(circle.is_definition);
    assert!(has_ref(&facts, "Circle", "Shape", EdgeKind::Inherits));

    assert_eq!(decl(&facts, "Circle::area").kind, SymbolKind::Method);
    assert!(decl(&facts, "Circle::area").is_definition);
    assert!(!decl(&facts, "Circle::perim").is_definition);
    assert_eq!(decl(&facts, "Circle::Circle").kind, SymbolKind::Method);
    assert_eq!(decl(&facts, "Circle::~Circle").kind, SymbolKind::Method);
    assert_eq!(decl(&facts, "Circle::radius").kind, SymbolKind::Field);
    assert_eq!(decl(&facts, "Circle::radius").line, 8);
}

#[test]
fn records_member_uses_and_field_types() {
    let facts = extract(CIRCLE);
    assert!(has_ref(&facts, "Circle::area", "radius", EdgeKind::Uses));
    assert!(has_ref(&facts, "Circle::Circle", "radius", EdgeKind::Uses));
    assert!(has_ref(&facts, "Circle", "Point", EdgeKind::Uses));
    // Parameters are locals, not references.
    assert!(!facts.refs.iter().any(|r| r.target == "r"));
}

#[test]
fn extracts_namespaces_structs_and_typedef_structs() {
    let facts = extract(
        "namespace geo { struct P { int x; }; }\n\
         typedef struct { int w; } Size;\n\
         namespace { struct Hidden {}; }\n",
    );
    assert_eq!(decl(&facts, "geo").kind, SymbolKind::Namespace);
    assert_eq!(decl(&facts, "geo::P").kind, SymbolKind::Struct);
    assert_eq!(decl(&facts, "geo::P::x").kind, SymbolKind::Field);
    assert_eq!(decl(&facts, "Size").kind, SymbolKind::Struct);
    assert_eq!(decl(&facts, "Size::w").kind, SymbolKind::Field);
    // Anonymous namespace contents live in the enclosing scope.
    assert_eq!(decl(&facts, "Hidden").kind, SymbolKind::Struct);
}

#[test]
fn resolves_calls_through_local_variable_types() {
    let facts = extract(
        "double total(const Circle& c, Square* s) {\n\
             Circle k(1.0);\n\
             geo::helper(k);\n\
             log_it();\n\
             return c.area() + s->area() + k.perim();\n\
         }\n",
    );
    assert_eq!(decl(&facts, "total").kind, SymbolKind::Function);
    assert!(has_ref(&facts, "total", "Circle::area", EdgeKind::Calls));
    assert!(has_ref(&facts, "total", "Square::area", EdgeKind::Calls));
    assert!(has_ref(&facts, "total", "Circle::perim", EdgeKind::Calls));
    assert!(has_ref(&facts, "total", "geo::helper", EdgeKind::Calls));
    assert!(has_ref(&facts, "total", "log_it", EdgeKind::Calls));
    assert!(has_ref(&facts, "total", "Circle", EdgeKind::Uses));
    assert!(!facts
        .refs
        .iter()
        .any(|r| r.target == "k" || r.target == "c"));
}

#[test]
fn unknown_receiver_calls_allow_unique_name_fallback() {
    let facts = extract("void f() { get().render(); }\n");
    let call = facts
        .refs
        .iter()
        .find(|r| r.target == "render")
        .expect("render ref");
    assert_eq!(call.kind, EdgeKind::Calls);
    assert_eq!(call.lookup, Lookup::ScopedOrUnique);
}

#[test]
fn extracts_out_of_line_definitions_and_prototypes() {
    let facts = extract(
        "int Square::area2() const { return side; }\n\
         int proto(int a);\n\
         template <typename T> T Box<T>::get() const { return value; }\n",
    );
    let area2 = decl(&facts, "Square::area2");
    assert!(area2.is_definition);
    assert!(has_ref(&facts, "Square::area2", "side", EdgeKind::Uses));

    let proto = decl(&facts, "proto");
    assert_eq!(proto.kind, SymbolKind::Function);
    assert!(!proto.is_definition);

    assert!(decl(&facts, "Box::get").is_definition);
}

#[test]
fn this_member_access_is_scoped() {
    let facts = extract("void A::f() { this->count++; this->g(); }\n");
    let uses = facts
        .refs
        .iter()
        .find(|r| r.target == "count")
        .expect("count ref");
    assert_eq!((uses.kind, uses.lookup), (EdgeKind::Uses, Lookup::Scoped));
    let call = facts.refs.iter().find(|r| r.target == "g").expect("g ref");
    assert_eq!((call.kind, call.lookup), (EdgeKind::Calls, Lookup::Scoped));
}

#[test]
fn keeps_operator_names_distinct() {
    let facts = extract(
        "struct A { bool operator<(const A&) const; bool operator>(const A&) const;\n\
         A& operator<<(int); bool operator==(const A&) const; };\n\
         bool A::operator<(const A& o) const { return true; }\n",
    );
    for op in [
        "A::operator<",
        "A::operator>",
        "A::operator<<",
        "A::operator==",
    ] {
        decl(&facts, op);
    }
    assert!(
        decl(&facts, "A::operator<").is_definition
            || facts
                .decls
                .iter()
                .any(|d| d.qualified_name == "A::operator<" && d.is_definition)
    );
    assert!(!facts
        .decls
        .iter()
        .any(|d| d.qualified_name == "A::operator"));
}

#[test]
fn range_for_variables_are_locals() {
    let facts = extract("struct T { int it; void g() { for (auto it : v) { use(it); } } };\n");
    assert!(!has_ref(&facts, "T::g", "it", EdgeKind::Uses));
}

#[test]
fn function_pointer_fields_are_fields() {
    let facts = extract("struct S { void (*cb)(int); };\n");
    assert_eq!(decl(&facts, "S::cb").kind, SymbolKind::Field);
}

fn spans_of(facts: &FileFacts, target: &str) -> Vec<(u32, u32, u32)> {
    facts
        .refs
        .iter()
        .filter(|r| r.target == target)
        .filter_map(|r| r.span)
        .map(|s| (s.line, s.start, s.end))
        .collect()
}

fn name_span(facts: &FileFacts, qualified: &str) -> (u32, u32, u32) {
    let span = decl(facts, qualified).name_span.expect("name span");
    (span.line, span.start, span.end)
}

#[test]
fn repeated_calls_are_separate_occurrences() {
    let facts = extract("void f() {\n  g();\n  g();\n}\n");
    assert_eq!(spans_of(&facts, "g"), [(2, 2, 3), (3, 2, 3)]);
}

#[test]
fn records_spans_of_member_calls_types_and_qualified_names() {
    let facts = extract("void f(Box b) {\n  b.open();\n  geo::helper();\n}\n");
    assert_eq!(spans_of(&facts, "Box::open"), [(2, 4, 8)]);
    assert_eq!(spans_of(&facts, "geo::helper"), [(3, 7, 13)]);
    assert_eq!(spans_of(&facts, "Box"), [(1, 7, 10)]);
}

#[test]
fn records_spans_of_bases_fields_and_initializers() {
    let facts = extract(CIRCLE);
    assert_eq!(spans_of(&facts, "Shape"), [(1, 22, 27)]);
    assert_eq!(spans_of(&facts, "Point"), [(9, 0, 5)]);
    let radius = spans_of(&facts, "radius");
    assert!(radius.contains(&(3, 28, 34)), "{radius:?}");
}

#[test]
fn records_declaration_name_spans() {
    let facts = extract("class Circle {\n  int radius;\n};\nint Circle::area() { return 0; }\n");
    assert_eq!(name_span(&facts, "Circle"), (1, 6, 12));
    assert_eq!(name_span(&facts, "Circle::radius"), (2, 6, 12));
    assert_eq!(name_span(&facts, "Circle::area"), (4, 12, 16));
}

#[test]
fn survives_broken_and_non_ascii_input() {
    let facts = extract("class { int ;; }}} void naïve(é) { ünï(); /* ünïcode */\n");
    // Just needs to not panic and return something.
    assert!(facts.decls.len() < 100);
}

#[test]
fn survives_deeply_nested_expressions() {
    let deep = format!(
        "int f() {{ return {}1{}; }}\n",
        "(".repeat(5000),
        ")".repeat(5000)
    );
    let facts = extract(&deep);
    assert!(decl(&facts, "f").is_definition);
}
