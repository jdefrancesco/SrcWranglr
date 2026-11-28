//! Node-local checks on one expression or statement: an assignment used as a
//! condition, an `&`/`|` precedence surprise, a self-comparison, and a
//! self-assignment. None of these need flow or type information — the mistake
//! is visible in the node itself — so each is cheap and near-free of false
//! positives.

use tree_sitter::Node;

use super::{columns_of, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// Run the condition checks that apply to `node`'s kind.
pub(super) fn check(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    match node.kind() {
        "if_statement" | "while_statement" => assignment_in_condition(node, source, flaws),
        "binary_expression" => {
            precedence(node, source, flaws);
            self_comparison(node, source, flaws);
        }
        "assignment_expression" => self_assignment(node, source, flaws),
        _ => {}
    }
}

/// `if (x = y)` — an assignment where `==` was almost certainly meant. The
/// idiom `if ((x = y))` wraps the assignment in a second pair of parentheses
/// to say it is deliberate, so only a bare assignment is flagged.
fn assignment_in_condition(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    let Some(value) = node
        .child_by_field_name("condition")
        .and_then(|clause| clause.child_by_field_name("value"))
    else {
        return;
    };
    if value.kind() != "assignment_expression" || operator(value, source) != "=" {
        return;
    }
    flaws.push(Flaw {
        rule: "assignment-in-condition",
        cwe: 481,
        severity: Severity::Medium,
        message: format!(
            "`{}` assigns inside the condition; `=` where `==` was likely meant.",
            text(source, value).trim()
        ),
        fix: Some(
            "compare with `==`, or wrap the assignment in a second pair of `()` to show it is \
             intended"
                .to_string(),
        ),
        line: line_of(value),
        columns: columns_of(value),
    });
}

/// `x & MASK == v` — `&`, `|` and `^` bind looser than the comparison, so this
/// groups as `x & (MASK == v)`, which is almost never what was meant.
fn precedence(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    let outer = operator(node, source);
    if !matches!(outer, "&" | "|" | "^") {
        return;
    }
    let operands = [
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ];
    for operand in operands.into_iter().flatten() {
        if operand.kind() != "binary_expression" {
            continue;
        }
        let inner = operator(operand, source);
        if matches!(inner, "==" | "!=" | "<" | ">" | "<=" | ">=") {
            flaws.push(Flaw {
                rule: "operator-precedence",
                cwe: 783,
                severity: Severity::Medium,
                message: format!(
                    "`{}` groups as `… {outer} (… {inner} …)` — `{outer}` binds looser than \
                     `{inner}`. Parenthesize what you meant.",
                    text(source, node).trim()
                ),
                fix: Some(format!("add parentheses around the `{outer}` operands")),
                line: line_of(node),
                columns: columns_of(node),
            });
            return;
        }
    }
}

/// `a == a`, `a < a` and the like: a value compared with itself, so the result
/// is constant. `!=` is left out — `x != x` is the idiomatic NaN test.
fn self_comparison(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    let op = operator(node, source);
    if !matches!(op, "==" | "<" | ">" | "<=" | ">=") {
        return;
    }
    if !same_variable(node, source) {
        return;
    }
    let always_true = matches!(op, "==" | "<=" | ">=");
    flaws.push(Flaw {
        rule: "self-comparison",
        cwe: if always_true { 571 } else { 570 },
        severity: Severity::Medium,
        message: format!(
            "`{}` compares a value with itself — always {}. A wrong operand is the usual cause.",
            text(source, node).trim(),
            if always_true { "true" } else { "false" }
        ),
        fix: None,
        line: line_of(node),
        columns: columns_of(node),
    });
}

/// `x = x`: an assignment with no effect, usually a copy-paste slip or a
/// missing field or index on one side.
fn self_assignment(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    if operator(node, source) != "=" || !same_variable(node, source) {
        return;
    }
    let left = node.child_by_field_name("left");
    let name = left.map(|l| text(source, l).trim()).unwrap_or_default();
    flaws.push(Flaw {
        rule: "self-assignment",
        cwe: 480,
        severity: Severity::Low,
        message: format!(
            "`{}` assigns `{name}` to itself and has no effect — likely a copy-paste slip or a \
             missing field or index.",
            text(source, node).trim()
        ),
        fix: None,
        line: line_of(node),
        columns: columns_of(node),
    });
}

/// Whether `node`'s `left` and `right` are the same side-effect-free variable
/// (an identifier, field access or subscript spelled identically).
fn same_variable(node: Node, source: &str) -> bool {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return false;
    };
    is_variable(left)
        && is_variable(right)
        && text(source, left).trim() == text(source, right).trim()
        && !has_side_effect(left)
}

fn is_variable(node: Node) -> bool {
    matches!(
        node.kind(),
        "identifier" | "field_expression" | "subscript_expression"
    )
}

/// Whether evaluating `node` could have a side effect — a call, assignment or
/// `++`/`--` — in which case comparing it with "itself" is not obviously a bug.
fn has_side_effect(node: Node) -> bool {
    matches!(
        node.kind(),
        "call_expression" | "assignment_expression" | "update_expression"
    ) || named_children(node).into_iter().any(has_side_effect)
}
