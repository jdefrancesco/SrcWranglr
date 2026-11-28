//! A comparison of an unsigned value against zero whose result is fixed: `x <
//! 0` can never be true and `x >= 0` can never be false (CWE-570/571). The
//! first is a guard that silently does nothing; the second, used as a loop
//! condition like `for (size_t i = n; i >= 0; i--)`, never terminates. Only the
//! strict tautologies are flagged — `x <= 0` and `x > 0` genuinely depend on
//! whether `x` is zero, so they are left alone.

use tree_sitter::Node;

use super::types::Types;
use super::{columns_of, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// Flag fixed-result comparisons of an unsigned variable against zero.
pub(super) fn check(body: Node, source: &str, types: &Types, flaws: &mut Vec<Flaw>) {
    match body.kind() {
        "function_definition" | "lambda_expression" => return,
        "binary_expression" => {
            if let Some(flaw) = compare(body, source, types) {
                flaws.push(flaw);
            }
        }
        _ => {}
    }
    for child in named_children(body) {
        check(child, source, types, flaws);
    }
}

/// `Some(true)` if the comparison is always true, `Some(false)` if always
/// false, for the unsigned-vs-zero shapes we are sure about.
fn compare(node: Node, source: &str, types: &Types) -> Option<Flaw> {
    let op = operator(node, source);
    if !matches!(op, "<" | ">" | "<=" | ">=") {
        return None;
    }
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let always_true = if is_unsigned_var(left, source, types) && is_zero(right, source) {
        match op {
            "<" => false, // x < 0  — never
            ">=" => true, // x >= 0 — always
            _ => return None,
        }
    } else if is_zero(left, source) && is_unsigned_var(right, source, types) {
        match op {
            ">" => false, // 0 > x  — never
            "<=" => true, // 0 <= x — always
            _ => return None,
        }
    } else {
        return None;
    };
    Some(flaw(node, always_true, source))
}

fn flaw(node: Node, always_true: bool, source: &str) -> Flaw {
    let (cwe, detail) = if always_true {
        (
            571,
            "always true: an unsigned value is always >= 0, so this test never fails — and as a \
             loop condition it never terminates",
        )
    } else {
        (
            570,
            "always false: an unsigned value is never negative, so this guard never fires",
        )
    };
    Flaw {
        rule: "unsigned-comparison",
        cwe,
        severity: Severity::Medium,
        message: format!(
            "comparing `{}` against zero is {detail}.",
            text(source, node)
        ),
        fix: Some(
            "use a signed type if a negative value is really possible, or drop the dead comparison"
                .to_string(),
        ),
        line: line_of(node),
        columns: columns_of(node),
    }
}

/// An identifier known to be an unsigned, non-pointer local or parameter.
fn is_unsigned_var(node: Node, source: &str, types: &Types) -> bool {
    node.kind() == "identifier"
        && types
            .get(text(source, node))
            .is_some_and(|ty| ty.unsigned && !ty.pointer)
}

/// Whether `node`, past any parentheses, is the integer literal zero.
fn is_zero(node: Node, source: &str) -> bool {
    let mut node = node;
    while node.kind() == "parenthesized_expression" {
        match node.child_by_field_name("value") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    if node.kind() != "number_literal" {
        return false;
    }
    let text = text(source, node).trim_end_matches(['u', 'U', 'l', 'L']);
    text == "0" || text == "0x0" || text == "0X0"
}
