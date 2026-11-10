//! An exception drafted from a line of code, with the metavariables of
//! the rule it is for, so `len` in the check is the `$C` the rule's
//! `memcpy($A, $B, $C)` binds:
//!
//! - a check (`if (len > sizeof(buf)) return -1;`) leaves out what comes
//!   after it: `if (<... $C > sizeof($A) ...>) ...`;
//! - a call to the same function (`memcpy(buf, src, sizeof(buf))`) is the
//!   fixed form: matches that look like it are left out;
//! - any other statement (`len = MIN(len, sizeof(buf));`) leaves out what
//!   comes after it.

use tree_sitter::Node;

use super::{
    code_on, first_call_in, first_callee, parse, row_of, same_shape, Metavariables, Names,
    MAX_CODE_BYTES, MAX_NODES_VISITED, MAX_PATTERN_BYTES,
};
use crate::analysis::semgrep_rule::{Exception, ExceptionKind};

/// Statement kinds an exception is drafted from whole.
const STATEMENTS: [&str; 9] = [
    "if_statement",
    "expression_statement",
    "declaration",
    "return_statement",
    "while_statement",
    "for_statement",
    "do_statement",
    "switch_statement",
    "goto_statement",
];

/// An exception from line `line` of `source` (the text of `file`) to a
/// rule with `names`, whose first call is to `callee`.
pub fn exception_from(
    source: &str,
    file: &str,
    line: u32,
    names: &Names,
    callee: Option<&str>,
) -> Result<Exception, String> {
    let tree = parse(source, file)?;
    let row = row_of(line)?;
    let root = tree.root_node();
    let node = statement_on(root, row)
        .or_else(|| code_on(root, row))
        .ok_or("No call or statement starts on this line.")?;
    if node.byte_range().len() > MAX_CODE_BYTES {
        return Err("The code on this line is too long for an exception.".into());
    }
    let mut metavariables = Metavariables::seeded(names);
    let exception = match node.kind() {
        "if_statement" => checked(node, source, &mut metavariables)?,
        _ => {
            let call = first_call_in(node)
                .filter(|_| callee.is_some() && first_callee(node, source).as_deref() == callee);
            match call {
                Some(call) => Exception {
                    kind: ExceptionKind::LooksLike,
                    pattern: same_shape(call, source, &mut metavariables).0,
                },
                None => Exception {
                    kind: ExceptionKind::After,
                    pattern: same_shape(node, source, &mut metavariables).0,
                },
            }
        }
    };
    match exception.pattern.len() > MAX_PATTERN_BYTES {
        true => Err("The code on this line is too long for an exception.".into()),
        false => Ok(exception),
    }
}

/// After the check `if_statement` makes, whatever it does then.
fn checked(
    if_statement: Node,
    source: &str,
    names: &mut Metavariables,
) -> Result<Exception, String> {
    let condition = if_statement
        .child_by_field_name("condition")
        .ok_or("This `if` has no condition to draft from.")?;
    // The condition's value, without the `if`'s own parentheses (a
    // grammar without a value field leaves them on the condition).
    let (shape, _) = match condition.child_by_field_name("value") {
        Some(value) => same_shape(value, source, names),
        None => same_shape(condition, source, names),
    };
    let is_bare = condition.child_by_field_name("value").is_some();
    let shape = match is_bare {
        true => shape.trim(),
        false => shape
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(&shape)
            .trim(),
    };
    Ok(Exception {
        kind: ExceptionKind::After,
        pattern: format!("if (<... {shape} ...>) ..."),
    })
}

/// The first statement starting on row `row`, outermost first.
fn statement_on(root: Node, row: usize) -> Option<Node> {
    let mut cursor = root.walk();
    for _ in 0..MAX_NODES_VISITED {
        let node = cursor.node();
        let (start, end) = (node.start_position().row, node.end_position().row);
        if start == row && STATEMENTS.contains(&node.kind()) {
            return Some(node);
        }
        let holds_row = start <= row && row <= end;
        if holds_row && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return None;
            }
        }
    }
    None
}
