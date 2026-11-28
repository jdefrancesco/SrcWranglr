//! Within one function body, an allocation whose NULL return is dereferenced
//! before anything checks it: `p = malloc(...)` followed by `*p`, `p->x` or
//! `p[i]` with no `if (p)` / `if (!p)` / `p == NULL` guard, assert, or
//! reassignment in between. The reading of "checked" is deliberately generous
//! — any mention of the pointer in a condition, an equality test, or a
//! check-like call clears the suspicion — so the check stays quiet on code
//! that does guard its allocations, and only the first dereference is flagged.

use std::collections::HashSet;

use tree_sitter::Node;

use super::{columns_of, leaf_identifier, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

const ALLOCATORS: [&str; 8] = [
    "malloc",
    "calloc",
    "realloc",
    "reallocarray",
    "strdup",
    "strndup",
    "aligned_alloc",
    "memalign",
];

/// Check one function body for an unchecked allocation that is dereferenced.
pub(super) fn check(body: Node, source: &str, flaws: &mut Vec<Flaw>) {
    let mut tracked: Vec<(String, u32)> = Vec::new();
    visit(body, source, &mut tracked, flaws);
}

/// Pre-order walk in source order. A node is processed before its children, so
/// a guard in an `if` condition clears the pointer before the dereference in
/// the branch body is reached. Nested functions have their own flow.
fn visit(node: Node, source: &str, tracked: &mut Vec<(String, u32)>, flaws: &mut Vec<Flaw>) {
    // A nested function has its own flow; a `sizeof`/`alignof` operand is never
    // evaluated, so `*p` inside `sizeof(*p)` is not a real dereference.
    if matches!(
        node.kind(),
        "function_definition" | "lambda_expression" | "sizeof_expression" | "alignof_expression"
    ) {
        return;
    }
    process(node, source, tracked, flaws);
    for child in named_children(node) {
        visit(child, source, tracked, flaws);
    }
}

fn process(node: Node, source: &str, tracked: &mut Vec<(String, u32)>, flaws: &mut Vec<Flaw>) {
    match node.kind() {
        "init_declarator" => {
            if let Some(name) = allocated_name(node, source) {
                set(tracked, name, line_of(node));
            }
        }
        "assignment_expression" => reassignment(node, source, tracked),
        "if_statement" | "while_statement" | "for_statement" | "conditional_expression" => {
            if let Some(condition) = node.child_by_field_name("condition") {
                clear_mentioned(tracked, condition, source);
            }
        }
        "binary_expression" if matches!(operator(node, source), "==" | "!=") => {
            clear_mentioned(tracked, node, source);
        }
        "call_expression" => guard_call(node, source, tracked),
        "pointer_expression" if operator(node, source) == "*" => {
            dereference(node, source, tracked, flaws);
        }
        "field_expression" | "subscript_expression" => dereference(node, source, tracked, flaws),
        _ => {}
    }
}

/// `p = malloc(...)` tracks `p`; `p = something_else` forgets it.
fn reassignment(node: Node, source: &str, tracked: &mut Vec<(String, u32)>) {
    if operator(node, source) != "=" {
        return;
    }
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let name = text(source, left);
    match node.child_by_field_name("right") {
        Some(value) if is_allocator(value, source) => set(tracked, name.to_string(), line_of(node)),
        _ => {
            clear(tracked, name);
        }
    }
}

/// A dereference of a tracked pointer: flag it, and forget the pointer so the
/// same allocation is reported once.
fn dereference(node: Node, source: &str, tracked: &mut Vec<(String, u32)>, flaws: &mut Vec<Flaw>) {
    let Some(argument) = node.child_by_field_name("argument") else {
        return;
    };
    if argument.kind() != "identifier" {
        return;
    }
    let name = text(source, argument);
    if let Some(allocated_at) = clear(tracked, name) {
        flaws.push(Flaw {
            rule: "unchecked-allocation",
            cwe: 476,
            severity: Severity::Medium,
            message: format!(
                "`{name}` is dereferenced here, but the allocation at line {allocated_at} is \
                 never checked for NULL — a failed allocation crashes here.",
            ),
            fix: Some(format!(
                "check `{name}` against NULL after the allocation before using it"
            )),
            line: line_of(node),
            columns: columns_of(node),
        });
    }
}

/// A call to a check-like function (`assert`, `CHECK`, `BUG_ON`, ...) clears
/// any tracked pointer among its arguments.
fn guard_call(node: Node, source: &str, tracked: &mut Vec<(String, u32)>) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    if !looks_like_check(text(source, function)) {
        return;
    }
    if let Some(arguments) = node.child_by_field_name("arguments") {
        clear_mentioned(tracked, arguments, source);
    }
}

fn looks_like_check(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "assert", "check", "ensure", "verify", "require", "bug", "panic", "fatal", "abort", "die",
        "null", "expect",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}

/// The pointer `p` in `T *p = malloc(...)`, if the initializer is an allocator.
fn allocated_name(node: Node, source: &str) -> Option<String> {
    let value = node.child_by_field_name("value")?;
    if !is_allocator(value, source) {
        return None;
    }
    let declarator = node.child_by_field_name("declarator")?;
    Some(leaf_identifier(declarator, source)?.to_string())
}

/// Whether `node`, past any cast or parentheses, is a call to a known allocator.
fn is_allocator(node: Node, source: &str) -> bool {
    let inner = unwrap(node);
    inner.kind() == "call_expression"
        && inner
            .child_by_field_name("function")
            .is_some_and(|f| ALLOCATORS.contains(&text(source, f)))
}

fn unwrap(node: Node) -> Node {
    let mut node = node;
    while matches!(node.kind(), "cast_expression" | "parenthesized_expression") {
        match node.child_by_field_name("value") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

fn set(tracked: &mut Vec<(String, u32)>, name: String, line: u32) {
    clear(tracked, &name);
    tracked.push((name, line));
}

/// Forget `name`, returning the line it was allocated on if it was tracked.
fn clear(tracked: &mut Vec<(String, u32)>, name: &str) -> Option<u32> {
    let index = tracked.iter().position(|(n, _)| n == name)?;
    Some(tracked.remove(index).1)
}

/// Forget every tracked pointer named anywhere inside `node`.
fn clear_mentioned(tracked: &mut Vec<(String, u32)>, node: Node, source: &str) {
    let names = identifiers(node, source);
    tracked.retain(|(name, _)| !names.contains(name.as_str()));
}

fn identifiers<'s>(node: Node, source: &'s str) -> HashSet<&'s str> {
    let mut set = HashSet::new();
    collect_identifiers(node, source, &mut set);
    set
}

fn collect_identifiers<'s>(node: Node, source: &'s str, set: &mut HashSet<&'s str>) {
    if node.kind() == "identifier" {
        set.insert(text(source, node));
    }
    for child in named_children(node) {
        collect_identifiers(child, source, set);
    }
}
