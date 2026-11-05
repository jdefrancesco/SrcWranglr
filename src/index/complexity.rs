//! McCabe cyclomatic complexity of a function definition: 1, plus one for
//! each place the code can branch (`if`, loops, `case`, `catch`, `?:`,
//! `&&`, `||`). Lambdas defined inside a function count toward it.

use tree_sitter::Node;

/// Statements and expressions that add one path each.
const BRANCH_KINDS: &[&str] = &[
    "if_statement",
    "for_statement",
    "for_range_loop",
    "while_statement",
    "do_statement",
    "catch_clause",
    "conditional_expression",
];
/// Short-circuit operators: each adds a path through a condition.
const LOGICAL_OPERATORS: &[&str] = &["&&", "||", "and", "or"];

/// The cyclomatic complexity of `function` (any syntax node; normally a
/// `function_definition`). Walks iteratively, so deep nesting is safe.
pub fn cyclomatic(function: Node) -> u32 {
    let mut cursor = function.walk();
    let mut complexity = 1u32;
    loop {
        complexity = complexity.saturating_add(branch_points(cursor.node()));
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return complexity;
            }
        }
    }
}

fn branch_points(node: Node) -> u32 {
    let kind = node.kind();
    let is_branch = BRANCH_KINDS.contains(&kind)
        // `default:` has no value and adds no path of its own.
        || (kind == "case_statement" && node.child_by_field_name("value").is_some())
        || (kind == "binary_expression"
            && node
                .child_by_field_name("operator")
                .is_some_and(|op| LOGICAL_OPERATORS.contains(&op.kind())));
    u32::from(is_branch)
}

/// How many source lines `node` covers.
pub fn line_count(node: Node) -> u32 {
    let rows = node.end_position().row - node.start_position().row + 1;
    u32::try_from(rows).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::*;

    /// Complexity of the first function defined in `source`.
    fn complexity_of(source: &str) -> u32 {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser.parse(source, None).expect("parse");
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root
            .children(&mut cursor)
            .find(|n| n.kind() == "function_definition")
            .expect("a function");
        cyclomatic(function)
    }

    #[test]
    fn straight_line_code_has_complexity_one() {
        assert_eq!(
            complexity_of("int f(int a) { int b = a + 1; return b * 2; }"),
            1
        );
    }

    #[test]
    fn each_branch_and_loop_adds_one() {
        let source = "int f(int a) {
            if (a) { a++; } else if (a > 2) { a--; }
            for (int i = 0; i < a; i++) {}
            while (a) { a--; }
            do { a++; } while (a < 3);
            for (int x : v) {}
            return a ? 1 : 0;
        }";
        // if, else-if, for, while, do, range-for, ?: = 7
        assert_eq!(complexity_of(source), 8);
    }

    #[test]
    fn cases_count_but_default_does_not() {
        let source = "int f(int a) {
            switch (a) { case 1: return 1; case 2: case 3: return 2; default: return 0; }
        }";
        assert_eq!(complexity_of(source), 4);
    }

    #[test]
    fn short_circuit_operators_count_but_bitwise_ones_do_not() {
        let source = "int f(int a, int b) { if (a && b || !a) return a & b | 1; return 0; }";
        // if, &&, || = 3
        assert_eq!(complexity_of(source), 4);
    }

    #[test]
    fn catch_clauses_count() {
        let source = "void f() { try { g(); } catch (const A&) {} catch (...) {} }";
        assert_eq!(complexity_of(source), 3);
    }

    #[test]
    fn survives_deep_nesting() {
        let depth = 5_000;
        let source = format!(
            "int f(int a) {{ return {}a{}; }}",
            "(a && ".repeat(depth),
            ")".repeat(depth)
        );
        assert!(complexity_of(&source) > 1);
    }

    #[test]
    fn counts_lines_of_a_node() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        let tree = parser
            .parse("int f()\n{\n  return 1;\n}\n", None)
            .expect("parse");
        let function = tree.root_node().child(0).expect("function");
        assert_eq!(line_count(function), 4);
    }
}
