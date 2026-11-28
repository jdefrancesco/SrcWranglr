//! `sizeof` applied to a pointer variable and used as a byte count: `memset(p,
//! 0, sizeof(p))`, `memcpy(dst, src, sizeof(dst))`, `malloc(sizeof(p))` and the
//! like (CWE-467). `sizeof(p)` is the size of the pointer — 8 bytes on a 64-bit
//! target — not of the object it points at, so the call touches or reserves far
//! fewer bytes than intended. Only a plain pointer identifier is flagged: the
//! correct `sizeof(*p)` and `sizeof(buf)` (a real array) read differently and
//! stay quiet.

use tree_sitter::Node;

use super::types::Types;
use super::{columns_of, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// Memory functions that take a byte count, and which argument that count is.
const SIZED_FUNCTIONS: [(&str, usize); 10] = [
    ("memcpy", 2),
    ("memmove", 2),
    ("memset", 2),
    ("strncpy", 2),
    ("strncat", 2),
    ("bcopy", 2),
    ("bzero", 1),
    ("malloc", 0),
    ("realloc", 1),
    ("calloc", 1),
];

/// Flag `sizeof(pointer)` used where a byte count is expected.
pub(super) fn check(body: Node, source: &str, types: &Types, flaws: &mut Vec<Flaw>) {
    match body.kind() {
        "function_definition" | "lambda_expression" => return,
        "call_expression" => {
            if let Some(flaw) = misuse(body, source, types) {
                flaws.push(flaw);
            }
        }
        _ => {}
    }
    for child in named_children(body) {
        check(child, source, types, flaws);
    }
}

fn misuse(call: Node, source: &str, types: &Types) -> Option<Flaw> {
    let function = call.child_by_field_name("function")?;
    let name = text(source, function);
    let (_, index) = SIZED_FUNCTIONS.iter().find(|(n, _)| *n == name)?;
    let arguments = call.child_by_field_name("arguments")?;
    let size = named_children(arguments).into_iter().nth(*index)?;
    let pointer = sizeof_pointer_operand(size, source, types)?;
    Some(flaw(name, pointer, size))
}

fn flaw(function: &str, pointer: &str, size: Node) -> Flaw {
    Flaw {
        rule: "sizeof-pointer",
        cwe: 467,
        severity: Severity::Medium,
        message: format!(
            "`sizeof({pointer})` measures the pointer itself (8 bytes on a 64-bit target), not \
             the object it points to, so `{function}` handles far fewer bytes than intended."
        ),
        fix: Some(format!(
            "pass the size of the pointed-to object, `sizeof(*{pointer})`, or the real buffer length"
        )),
        line: line_of(size),
        columns: columns_of(size),
    }
}

/// The name behind a size argument of the shape `sizeof(p)` where `p` is a known
/// pointer, past any wrapping parentheses or cast.
fn sizeof_pointer_operand<'s>(node: Node, source: &'s str, types: &Types) -> Option<&'s str> {
    let mut node = node;
    while matches!(node.kind(), "parenthesized_expression" | "cast_expression") {
        node = node.child_by_field_name("value")?;
    }
    if node.kind() != "sizeof_expression" {
        return None;
    }
    // The operand carries no field name here, so take the sole named child.
    let operand = named_children(node).into_iter().next()?;
    let name = operand_name(operand, source)?;
    types.get(name).filter(|ty| ty.pointer).map(|_| name)
}

/// The bare name a `sizeof` operand refers to, if any. The C++ grammar parses a
/// single parenthesized name (`sizeof(p)`) as a type descriptor, so this follows
/// `parenthesized_expression`, `type_descriptor`, identifiers and type names
/// alike; `sizeof(*p)`, `sizeof(int)` and compound expressions yield `None` (the
/// first is a pointer dereference, the rest are not names of a tracked local).
fn operand_name<'s>(operand: Node, source: &'s str) -> Option<&'s str> {
    let mut node = operand;
    loop {
        match node.kind() {
            "identifier" | "type_identifier" => return Some(text(source, node)),
            "parenthesized_expression" => node = named_children(node).into_iter().next()?,
            "type_descriptor" => node = node.child_by_field_name("type")?,
            _ => return None,
        }
    }
}
