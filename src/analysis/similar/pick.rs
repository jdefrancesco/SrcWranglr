//! The code on several lines: the statement (or function) they hold
//! exactly, else the statements that start on them in the innermost block
//! holding them all.

use tree_sitter::Node;

use super::{statement_code, MAX_NODES_VISITED};

/// Statements in one sequence, at most.
pub(super) const MAX_STATEMENTS: usize = 500;
/// Nodes whose children are a sequence of statements or declarations.
const LISTS: [&str; 5] = [
    "compound_statement",
    "translation_unit",
    "declaration_list",
    "field_declaration_list",
    "case_statement",
];

pub(super) enum Picked<'t> {
    One(Node<'t>),
    Sequence(Vec<Node<'t>>),
}

/// The code on rows `first..=last` of the tree at `root`.
pub(super) fn picked(root: Node, first: usize, last: usize) -> Option<Picked> {
    let (first, last) = (first.min(last), first.max(last));
    let mut container = root;
    for _ in 0..MAX_NODES_VISITED {
        match holding_child(container, first, last) {
            Some(child) if starts_and_ends(child, first, last) => return Some(one(child)),
            Some(child) => container = child,
            None => break,
        }
    }
    if !LISTS.contains(&container.kind()) {
        return (container != root).then(|| one(code_around(container)));
    }
    let mut cursor = container.walk();
    let statements: Vec<Node> = container
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .filter(|c| (first..=last).contains(&c.start_position().row))
        .take(MAX_STATEMENTS + 1)
        .collect();
    match statements.as_slice() {
        [] => None,
        [statement] => Some(one(*statement)),
        _ => Some(Picked::Sequence(statements)),
    }
}

/// `node` alone, matched like a line's statement.
fn one(node: Node) -> Picked {
    Picked::One(statement_code(node).unwrap_or(node))
}

fn starts_and_ends(node: Node, first: usize, last: usize) -> bool {
    node.start_position().row == first && node.end_position().row == last
}

/// The first child of `node` (not a comment) holding all of the rows.
fn holding_child(node: Node, first: usize, last: usize) -> Option<Node> {
    let mut cursor = node.walk();
    let child = node.named_children(&mut cursor).find(|c| {
        c.kind() != "comment" && c.start_position().row <= first && c.end_position().row >= last
    });
    child
}

/// `node`, or the nearest statement or expression around it (rows in the
/// middle of a call's arguments mean the call).
fn code_around(node: Node) -> Node {
    let mut code = node;
    for _ in 0..MAX_NODES_VISITED {
        let kind = code.kind();
        let is_code = kind.ends_with("_statement")
            || kind.ends_with("_expression")
            || matches!(kind, "declaration" | "function_definition");
        match code.parent() {
            Some(parent) if !is_code => code = parent,
            _ => break,
        }
    }
    code
}

/// The statement holding `node`: the node under the nearest block.
pub(super) fn statement_of(node: Node) -> Node {
    let mut statement = node;
    for _ in 0..MAX_NODES_VISITED {
        match statement.parent() {
            Some(parent) if !LISTS.contains(&parent.kind()) => statement = parent,
            _ => break,
        }
    }
    statement
}
