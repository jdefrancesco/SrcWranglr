//! Finding look-alike functions and blocks.

use std::collections::HashSet;

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

fn shapes(files: &[(&str, &str)]) -> (CodeGraph, Shapes) {
    let graph = graph_of_files(files);
    let shapes = shapes_of(&graph, &AtomicBool::new(false)).expect("shapes");
    (graph, shapes)
}

/// A parser, written twice with other names, numbers, strings, comments
/// and layout: the same shape.
const ONE: &str = "int parse_header(const unsigned char *buf, int len, char *out) {\n\
    int n = buf[0];\n\
    if (n > len) { return -1; }\n\
    for (int i = 0; i < n; i++) {\n\
        out[i] = buf[i + 1];\n\
    }\n\
    log_msg(\"header %d\", n);\n\
    return n;\n\
}\n";

const TWO: &str = "/* copied from parse.c */\n\
int read_field(const unsigned char *data, int size, char *dst)\n\
{\n\
    int count = data[2];   // first byte\n\
    if (count > size) {\n\
        return -7;\n\
    }\n\
    for (int k = 0; k < count; k++) { dst[k] = data[k + 3]; }\n\
    trace(\"field %d\", count);\n\
    return count;\n\
}\n";

/// The same with one more check: alike, not the same.
const THREE: &str = "int parse_item(const unsigned char *buf, int len, char *out) {\n\
    int n = buf[0];\n\
    if (n > len) { return -1; }\n\
    if (n == 0) { return 0; }\n\
    for (int i = 0; i < n; i++) {\n\
        out[i] = buf[i + 1];\n\
    }\n\
    log_msg(\"item %d\", n);\n\
    return n;\n\
}\n";

const OTHER: &str = "double average(const double *values, unsigned count) {\n\
    double total = 0.0;\n\
    unsigned seen = 0;\n\
    while (seen < count) { total += values[seen]; seen = seen + 1; }\n\
    if (count == 0) { return 0.0; } else { return total / count; }\n\
}\n";

#[test]
fn renamed_and_reformatted_copies_have_the_same_shape() {
    let (graph, shapes) = shapes(&[("a.c", ONE), ("b.c", TWO), ("c.c", OTHER)]);
    let (found, _) = groups(&shapes);
    let functions: Vec<&Group> = found
        .iter()
        .filter(|g| g.kind == UnitKind::Function)
        .collect();
    assert_eq!(functions.len(), 1, "{found:?}");
    let names: HashSet<SymbolId> = symbols_in(&shapes, functions[0]);
    assert_eq!(
        names,
        HashSet::from([id(&graph, "parse_header"), id(&graph, "read_field")])
    );
    assert!(
        found.iter().all(|g| g.kind == UnitKind::Function),
        "their loops are inside look-alike functions already: {found:?}"
    );
}

#[test]
fn a_copied_block_in_different_functions_is_found_on_its_own() {
    let loop_once = "void a(int *p, int n) { setup(); for (int i = 0; i < n; i++) { if (p[i] > 9) \
        { p[i] = p[i] - 9; } else { p[i] = p[i] + 1; } total += p[i] * 2; } }\n";
    let loop_again = "int b(int *q, int m, int z) { int r = z; while (r) { r--; } for (int j = 0; \
        j < m; j++) { if (q[j] > 4) { q[j] = q[j] - 4; } else { q[j] = q[j] + 7; } sum += q[j] \
        * 3; } return r; }\n";
    let (_, shapes) = shapes(&[("a.c", loop_once), ("b.c", loop_again)]);
    let (found, _) = groups(&shapes);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, UnitKind::Block);
    let lines: Vec<&str> = found[0]
        .units
        .iter()
        .map(|&i| shapes.units[i].file.as_str())
        .collect();
    assert_eq!(lines, ["a.c", "b.c"]);
}

#[test]
fn short_functions_are_not_compared() {
    let tiny = "int get(int *p) { return *p; }\nint got(int *q) { return *q; }\n";
    let (_, shapes) = shapes(&[("t.c", tiny)]);
    assert!(shapes.units.is_empty());
    assert!(groups(&shapes).0.is_empty());
}

#[test]
fn like_ranks_exact_copies_then_near_ones_and_leaves_out_the_rest() {
    let (graph, shapes) = shapes(&[("a.c", ONE), ("b.c", TWO), ("c.c", OTHER), ("d.c", THREE)]);
    let found = like(&shapes, id(&graph, "parse_header"));
    let named: Vec<(SymbolId, bool)> = found
        .iter()
        .map(|&(i, score)| (shapes.units[i].symbol.expect("indexed"), score == 1.0))
        .collect();
    assert_eq!(
        named,
        [
            (id(&graph, "read_field"), true),
            (id(&graph, "parse_item"), false)
        ]
    );
    assert!(found[1].1 >= MIN_LIKENESS && found[1].1 < 1.0, "{found:?}");
    assert!(like(&shapes, 999_999).is_empty());
}

#[test]
fn a_stop_ends_the_reading() {
    let graph = graph_of_files(&[("a.c", ONE)]);
    assert!(shapes_of(&graph, &AtomicBool::new(true)).is_none());
}

#[test]
fn deep_nesting_is_read_without_recursion() {
    let depth = 3_000;
    let source = format!(
        "int deep(int a) {{ return {}a{}; }}\n",
        "(a + ".repeat(depth),
        ")".repeat(depth)
    );
    let (_, shapes) = shapes(&[("d.c", &source)]);
    assert_eq!(shapes.units.len(), 1);
}

#[test]
fn a_long_else_if_chain_is_read_quickly_and_capped() {
    let mut body = String::from("int chain(int v) {\n");
    for i in 0..5_000 {
        body.push_str(&format!(
            "  if (v == {i}) {{ v = v + {i} * 3; call(v, {i}); }} else\n"
        ));
    }
    body.push_str("  { v = 0; }\n  return v;\n}\n");
    let started = std::time::Instant::now();
    let (_, shapes) = shapes(&[("c.c", &body)]);
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    let blocks = shapes
        .units
        .iter()
        .filter(|u| u.kind == UnitKind::Block)
        .count();
    assert!(blocks <= MAX_BLOCK_DEPTH + 1, "{blocks} blocks");
}

#[test]
fn a_block_knows_the_unit_it_is_inside() {
    let (_, shapes) = shapes(&[("a.c", ONE)]);
    let function = shapes
        .units
        .iter()
        .position(|u| u.kind == UnitKind::Function)
        .expect("the function");
    for unit in shapes.units.iter().filter(|u| u.kind == UnitKind::Block) {
        assert_eq!(unit.parent, Some(function));
    }
}
