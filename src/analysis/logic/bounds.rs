//! A fixed-size local array indexed past its end (CWE-193 / out-of-bounds
//! access). Two shapes are flagged, both only when the array's declared length
//! is known and the offending index is a literal, so nothing is guessed: a
//! constant subscript at or beyond the length (`char b[10]; b[10] = 0;`), and a
//! `for` loop whose literal bound carries the index one past the end
//! (`for (i = 0; i <= 10; i++) b[i] = 0;`). Array parameters are not tracked —
//! their size belongs to the caller — and a variable index with no literal
//! bound is left alone.

use tree_sitter::Node;

use super::types::Types;
use super::{columns_of, literal_u64, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// Flag out-of-bounds indexing of fixed-size local arrays.
pub(super) fn check(body: Node, source: &str, types: &Types, flaws: &mut Vec<Flaw>) {
    match body.kind() {
        "function_definition" | "lambda_expression" => return,
        "subscript_expression" => {
            if let Some(flaw) = constant_index(body, source, types) {
                flaws.push(flaw);
            }
        }
        "for_statement" => {
            if let Some(flaw) = loop_overrun(body, source, types) {
                flaws.push(flaw);
            }
        }
        _ => {}
    }
    for child in named_children(body) {
        check(child, source, types, flaws);
    }
}

/// `arr[C]` where `C` is a constant at or beyond `arr`'s length.
fn constant_index(subscript: Node, source: &str, types: &Types) -> Option<Flaw> {
    let (array, len) = array_and_len(subscript, source, types)?;
    let index = literal_u64(single_index(subscript)?, source)?;
    if index < len {
        return None;
    }
    let message = format!(
        "`{array}` holds {len} elements (valid indices 0..{}), so index {index} is out of bounds.",
        len.saturating_sub(1)
    );
    let fix = format!("index `{array}` within 0..{}", len.saturating_sub(1));
    Some(flaw(message, fix, subscript))
}

/// A `for` loop whose literal bound drives a subscript of a known array one or
/// more elements past its end.
fn loop_overrun(for_statement: Node, source: &str, types: &Types) -> Option<Flaw> {
    let condition = for_statement.child_by_field_name("condition")?;
    if condition.kind() != "binary_expression" {
        return None;
    }
    let left = condition.child_by_field_name("left")?;
    let right = condition.child_by_field_name("right")?;
    if left.kind() != "identifier" {
        return None;
    }
    let loop_var = text(source, left);
    let bound = literal_u64(right, source)?;
    let max_index = match operator(condition, source) {
        "<=" => bound,
        "<" => bound.checked_sub(1)?,
        _ => return None,
    };
    let body = for_statement.child_by_field_name("body")?;
    overrun_subscript(body, source, types, loop_var, max_index)
}

/// The first `arr[loop_var]` in `node` whose array is shorter than `max_index`.
fn overrun_subscript(
    node: Node,
    source: &str,
    types: &Types,
    loop_var: &str,
    max_index: u64,
) -> Option<Flaw> {
    if matches!(node.kind(), "function_definition" | "lambda_expression") {
        return None;
    }
    if node.kind() == "subscript_expression" {
        if let Some((array, len)) = array_and_len(node, source, types) {
            let indexes_with_loop_var = single_index(node).is_some_and(|index| {
                index.kind() == "identifier" && text(source, index) == loop_var
            });
            if indexes_with_loop_var && max_index >= len {
                let message = format!(
                    "this loop lets the index reach {max_index}, but `{array}` holds {len} \
                     elements (valid indices 0..{}), so the last pass writes past the end.",
                    len.saturating_sub(1)
                );
                let fix = format!("stop the loop at {} (use `<` with the length)", len);
                return Some(flaw(message, fix, node));
            }
        }
    }
    named_children(node)
        .into_iter()
        .find_map(|child| overrun_subscript(child, source, types, loop_var, max_index))
}

/// The indexed array's name and declared length, if it is a known fixed array.
fn array_and_len<'s>(subscript: Node, source: &'s str, types: &Types) -> Option<(&'s str, u64)> {
    let argument = subscript.child_by_field_name("argument")?;
    if argument.kind() != "identifier" {
        return None;
    }
    let name = text(source, argument);
    Some((name, types.get(name)?.array_len?))
}

/// The sole index node of a subscript, or `None` for multi-dimensional access.
fn single_index(subscript: Node) -> Option<Node> {
    let indices = subscript.child_by_field_name("indices")?;
    let mut children = named_children(indices).into_iter();
    let first = children.next()?;
    children.next().is_none().then_some(first)
}

fn flaw(message: String, fix: String, subscript: Node) -> Flaw {
    Flaw {
        rule: "array-bounds",
        cwe: out_of_bounds_cwe(subscript),
        severity: Severity::Medium,
        message,
        fix: Some(fix),
        line: line_of(subscript),
        columns: columns_of(subscript),
    }
}

/// CWE-787 when the subscript is written through, CWE-125 when it is only read.
fn out_of_bounds_cwe(subscript: Node) -> u32 {
    let written = subscript.parent().is_some_and(|parent| {
        parent.kind() == "assignment_expression"
            && parent.child_by_field_name("left") == Some(subscript)
    });
    if written {
        787
    } else {
        125
    }
}
