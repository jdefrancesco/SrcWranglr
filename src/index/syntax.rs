//! Small, pure helpers for reading tree-sitter C/C++ syntax nodes.

use tree_sitter::Node;

use super::extract::MAX_DECLARATOR_DEPTH;
use crate::model::Span;

pub(super) fn text<'s>(node: Node, src: &'s str) -> &'s str {
    src.get(node.byte_range()).unwrap_or("")
}

/// Name as written, without whitespace or template arguments:
/// `Box < T > :: get` → `Box::get`.
pub(super) fn clean_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut template_depth = 0usize;
    for c in raw.chars() {
        match c {
            '<' => template_depth += 1,
            '>' => template_depth = template_depth.saturating_sub(1),
            c if template_depth == 0 && !c.is_whitespace() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Name of a declared entity. Unlike `clean_name`, operator names keep their
/// `<`/`>` (`operator<<` must not be read as a template argument list), and
/// template arguments are stripped structurally from qualifiers only.
pub(super) fn declared_name(node: Node, src: &str) -> String {
    let compact = |n: Node| text(n, src).split_whitespace().collect::<String>();
    match node.kind() {
        "operator_name" | "destructor_name" => compact(node),
        "template_function" => node
            .child_by_field_name("name")
            .map_or_else(String::new, |n| declared_name(n, src)),
        "qualified_identifier" => {
            let scope = node
                .child_by_field_name("scope")
                .map(|n| clean_name(text(n, src)));
            let name = node
                .child_by_field_name("name")
                .map(|n| declared_name(n, src));
            match (scope, name) {
                (Some(scope), Some(name)) if !scope.is_empty() => format!("{scope}::{name}"),
                (_, Some(name)) => name,
                _ => clean_name(text(node, src)),
            }
        }
        _ => clean_name(text(node, src)),
    }
}

/// `void (*cb)(int)` declares a pointer *variable*, not a function.
pub(super) fn is_function_pointer(func_declarator: Node) -> bool {
    func_declarator
        .child_by_field_name("declarator")
        .is_some_and(|inner| inner.kind() == "parenthesized_declarator")
}

pub(super) fn segments(name: &str) -> impl Iterator<Item = String> + '_ {
    name.split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(super) fn join(scope: &[String], name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", scope.join("::"))
    }
}

pub(super) fn line_of(node: Node) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}

/// Where `node` sits, or `None` if it spans several lines.
pub(super) fn span_of(node: Node) -> Option<Span> {
    let (start, end) = (node.start_position(), node.end_position());
    if start.row != end.row {
        return None;
    }
    Some(Span {
        line: u32::try_from(start.row + 1).ok()?,
        start: u32::try_from(start.column).ok()?,
        end: u32::try_from(end.column).ok()?,
    })
}

/// The last name inside a qualified or templated name: `a::Box<T>::run` → `run`.
/// That is the part made clickable in the code view.
pub(super) fn name_leaf(node: Node) -> Node {
    let mut current = node;
    for _ in 0..MAX_DECLARATOR_DEPTH {
        let is_wrapper = matches!(
            current.kind(),
            "qualified_identifier" | "template_function" | "template_type" | "template_method"
        );
        match current.child_by_field_name("name").filter(|_| is_wrapper) {
            Some(name) => current = name,
            None => break,
        }
    }
    current
}

/// Span of the clickable part of a name node.
pub(super) fn name_span(node: Node) -> Option<Span> {
    span_of(name_leaf(node))
}

pub(super) fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

pub(super) fn field_children<'t>(node: Node<'t>, field: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor).collect()
}

/// Unwraps `pointer_declarator`, `reference_declarator`, ... down to the
/// `function_declarator`, if this declarator declares a function.
pub(super) fn find_function_declarator(node: Node) -> Option<Node> {
    let mut current = node;
    for _ in 0..MAX_DECLARATOR_DEPTH {
        match current.kind() {
            "function_declarator" => return Some(current),
            "pointer_declarator"
            | "reference_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator" => {
                current = current
                    .child_by_field_name("declarator")
                    .or_else(|| current.named_child(0))?;
            }
            _ => return None,
        }
    }
    None
}

/// The name node inside a declarator (`*x[3]` → `x`, `Foo::bar` → itself).
pub(super) fn declarator_name(node: Node) -> Option<Node> {
    let mut current = node;
    for _ in 0..MAX_DECLARATOR_DEPTH {
        match current.kind() {
            "identifier"
            | "field_identifier"
            | "qualified_identifier"
            | "destructor_name"
            | "operator_name"
            | "type_identifier" => return Some(current),
            "template_function" => return current.child_by_field_name("name"),
            _ => {
                current = current
                    .child_by_field_name("declarator")
                    .or_else(|| current.named_child(0))?;
            }
        }
    }
    None
}

/// Name of a user-defined type node, or `None` for builtins like `int`.
pub(super) fn type_name(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" | "qualified_identifier" => Some(clean_name(text(node, src))),
        "template_type" => node
            .child_by_field_name("name")
            .map(|n| clean_name(text(n, src))),
        _ => None,
    }
    .filter(|name| !name.is_empty())
}

pub(super) fn is_class_like(kind: &str) -> bool {
    matches!(
        kind,
        "class_specifier" | "struct_specifier" | "union_specifier"
    )
}
