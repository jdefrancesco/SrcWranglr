//! An allocation whose size is a multiplication that can overflow: `malloc(n *
//! m)`, `malloc(n * sizeof(T))`, `realloc(p, a * b)` and the like (CWE-190). On
//! overflow the product wraps to a small value, the allocation succeeds short,
//! and the code then writes the full, un-wrapped amount — a heap overflow. To
//! stay high-signal the check fires only when *neither* factor is a literal, so
//! the exploit-shaped `count * size` is caught while the ubiquitous `n * 4`
//! stays quiet.

use tree_sitter::Node;

use super::{columns_of, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// Allocators that take a raw byte size, and which argument that size is.
const SIZED_ALLOCATORS: [(&str, usize); 4] = [
    ("malloc", 0),
    ("alloca", 0),
    ("realloc", 1),
    ("aligned_alloc", 1),
];

/// Flag an overflow-prone multiplication in an allocation size, node-locally.
pub(super) fn check(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    if node.kind() != "call_expression" {
        return;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let name = text(source, function);
    let Some(&(_, index)) = SIZED_ALLOCATORS.iter().find(|(n, _)| *n == name) else {
        return;
    };
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return;
    };
    let Some(size) = named_children(arguments).into_iter().nth(index) else {
        return;
    };
    if let Some(product) = risky_multiply(size, source) {
        flaws.push(flaw(name, product));
    }
}

fn flaw(name: &str, product: Node) -> Flaw {
    Flaw {
        rule: "allocation-size-overflow",
        cwe: 190,
        severity: Severity::Medium,
        message: format!(
            "the size passed to `{name}` multiplies values that can overflow — the product wraps \
             to a small allocation while the code goes on to write the full amount."
        ),
        fix: Some(
            "check the factors against SIZE_MAX before multiplying, or use calloc()/\
             reallocarray(), which detect the overflow"
                .to_string(),
        ),
        line: line_of(product),
        columns: columns_of(product),
    }
}

/// The first `*` multiplication in `node` whose two operands are both
/// non-literal (anywhere in the subtree), or `None`.
fn risky_multiply<'t>(node: Node<'t>, source: &str) -> Option<Node<'t>> {
    if node.kind() == "binary_expression" && operator(node, source) == "*" {
        let left = node.child_by_field_name("left");
        let right = node.child_by_field_name("right");
        if let (Some(left), Some(right)) = (left, right) {
            if !is_literal(left) && !is_literal(right) {
                return Some(node);
            }
        }
    }
    named_children(node)
        .into_iter()
        .find_map(|child| risky_multiply(child, source))
}

/// Whether `node`, past any parentheses or cast, is a numeric literal.
fn is_literal(node: Node) -> bool {
    let mut node = node;
    while matches!(node.kind(), "parenthesized_expression" | "cast_expression") {
        match node.child_by_field_name("value") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node.kind() == "number_literal"
}
