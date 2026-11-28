//! A signed integer compared against an unsigned value, where the signed side
//! is attacker-influenced (CWE-195). C converts the signed operand to unsigned
//! for the comparison, so a negative value becomes huge and slips past a bound
//! — the classic signedness bounds-check defeat. To stay high-signal this fires
//! only when the signed operand is confirmed tainted: a backward trace from it
//! reaches outside input. A benign mixed comparison on untainted data (a loop
//! counter against a `size_t`, say) is left alone, which is why this check is
//! paired with the trace engine rather than run on types alone.

use tree_sitter::Node;

use super::types::Types;
use super::{columns_of, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;
use crate::model::CodeGraph;

/// Flag tainted signed-vs-unsigned comparisons in one function body.
pub(super) fn check(
    graph: &CodeGraph,
    file: &str,
    body: Node,
    source: &str,
    types: &Types,
    flaws: &mut Vec<Flaw>,
) {
    match body.kind() {
        "function_definition" | "lambda_expression" => return,
        "binary_expression" => {
            if let Some(flaw) = mismatch(graph, file, body, source, types) {
                flaws.push(flaw);
            }
        }
        _ => {}
    }
    for child in named_children(body) {
        check(graph, file, child, source, types, flaws);
    }
}

fn mismatch(
    graph: &CodeGraph,
    file: &str,
    node: Node,
    source: &str,
    types: &Types,
) -> Option<Flaw> {
    if !matches!(operator(node, source), "<" | ">" | "<=" | ">=") {
        return None;
    }
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let signed = signed_operand(left, right, source, types)?;
    is_tainted(graph, file, signed).then(|| flaw(node, signed, source))
}

/// The signed-integer operand of a signed-vs-unsigned comparison, or `None`.
fn signed_operand<'t>(
    left: Node<'t>,
    right: Node<'t>,
    source: &str,
    types: &Types,
) -> Option<Node<'t>> {
    if is_signed(left, source, types) && is_unsigned(right, source, types) {
        Some(left)
    } else if is_unsigned(left, source, types) && is_signed(right, source, types) {
        Some(right)
    } else {
        None
    }
}

fn is_signed(node: Node, source: &str, types: &Types) -> bool {
    node.kind() == "identifier"
        && types
            .get(text(source, node))
            .is_some_and(|ty| ty.signed_int && !ty.pointer)
}

fn is_unsigned(node: Node, source: &str, types: &Types) -> bool {
    node.kind() == "identifier"
        && types
            .get(text(source, node))
            .is_some_and(|ty| ty.unsigned && !ty.pointer)
}

/// Whether a backward trace of `operand` reaches outside input.
fn is_tainted(graph: &CodeGraph, file: &str, operand: Node) -> bool {
    let line = operand.start_position().row as u32 + 1;
    let column = operand.start_position().column as u32;
    crate::analysis::trace::trace_variable(graph, file, line, column)
        .is_some_and(|trace| trace.paths.iter().any(|path| path.endpoint.is_input()))
}

fn flaw(node: Node, signed: Node, source: &str) -> Flaw {
    let name = text(source, signed);
    Flaw {
        rule: "signed-unsigned-compare",
        cwe: 195,
        severity: Severity::High,
        message: format!(
            "`{name}` is signed but compared with an unsigned value, so it is converted to \
             unsigned here; `{name}` is attacker-influenced, so a negative value becomes huge and \
             slips past this bound."
        ),
        fix: Some(format!(
            "reject `{name} < 0` before this check, or give both sides the same signedness"
        )),
        line: line_of(node),
        columns: columns_of(node),
    }
}
