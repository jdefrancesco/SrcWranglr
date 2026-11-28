//! A light, per-function map from local and parameter names to the few facts
//! the integer/bounds checks need: whether a variable is unsigned and whether
//! it is a pointer. Built by scanning the function's parameters and the
//! declarations in its own body — no header, typedef, or cross-function
//! knowledge, which is the honest limit of what one function's text can tell us.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::analysis::parse::{named_children, text};

/// What little we know about one variable.
#[derive(Default, Clone, Copy)]
pub(super) struct VarType {
    /// Declared with an unsigned type (`unsigned …`, `size_t`, `uintN_t`, …).
    pub unsigned: bool,
    /// Declared as a pointer.
    pub pointer: bool,
    /// Element count of a fixed-size local array (`char buf[10]` → 10). Never
    /// set for parameters, whose bracketed size is only a hint.
    pub array_len: Option<u64>,
}

/// The variables in scope in one function, by name.
pub(super) struct Types {
    vars: HashMap<String, VarType>,
}

impl Types {
    /// Read a function definition's parameters and body declarations.
    pub(super) fn from_function(function: Node, source: &str) -> Self {
        let mut vars = HashMap::new();
        if let Some(declarator) = function.child_by_field_name("declarator") {
            collect_params(declarator, source, &mut vars);
        }
        if let Some(body) = function.child_by_field_name("body") {
            collect_decls(body, source, &mut vars);
        }
        Types { vars }
    }

    /// What is known about `name`, if it was declared in this function.
    pub(super) fn get(&self, name: &str) -> Option<VarType> {
        self.vars.get(name).copied()
    }
}

/// Add every `parameter_declaration` reachable in the declarator.
fn collect_params(node: Node, source: &str, vars: &mut HashMap<String, VarType>) {
    if node.kind() == "parameter_declaration" {
        insert(
            node,
            node.child_by_field_name("declarator"),
            source,
            vars,
            true,
        );
        return;
    }
    for child in named_children(node) {
        collect_params(child, source, vars);
    }
}

/// Add every body declaration, not descending into nested functions.
fn collect_decls(node: Node, source: &str, vars: &mut HashMap<String, VarType>) {
    match node.kind() {
        "function_definition" | "lambda_expression" => return,
        "declaration" => {
            for declarator in named_children(node) {
                insert(node, Some(declarator), source, vars, false);
            }
        }
        _ => {}
    }
    for child in named_children(node) {
        collect_decls(child, source, vars);
    }
}

/// Record the variable `declarator` introduces under `declaration`'s type.
fn insert(
    declaration: Node,
    declarator: Option<Node>,
    source: &str,
    vars: &mut HashMap<String, VarType>,
    is_param: bool,
) {
    let Some(declarator) = declarator else {
        return;
    };
    let Some((name, pointer, array_len)) = unwrap_declarator(declarator, source) else {
        return;
    };
    // An array parameter decays to a pointer: its bracketed size is a hint and
    // the real buffer belongs to the caller, so it is not a length we can trust.
    let (pointer, array_len) = if is_param && array_len.is_some() {
        (true, None)
    } else {
        (pointer, array_len)
    };
    let unsigned = declaration
        .child_by_field_name("type")
        .is_some_and(|ty| is_unsigned_type(ty, source));
    vars.insert(
        name.to_string(),
        VarType {
            unsigned,
            pointer,
            array_len,
        },
    );
}

/// The declared name, whether it is a pointer, and any fixed array length,
/// through declarator wrappers.
fn unwrap_declarator<'s>(node: Node, source: &'s str) -> Option<(&'s str, bool, Option<u64>)> {
    match node.kind() {
        "identifier" => Some((text(source, node), false, None)),
        "init_declarator" => unwrap_declarator(node.child_by_field_name("declarator")?, source),
        "pointer_declarator" => {
            let (name, _, _) = unwrap_declarator(node.child_by_field_name("declarator")?, source)?;
            Some((name, true, None))
        }
        "array_declarator" => {
            let (name, pointer, _) =
                unwrap_declarator(node.child_by_field_name("declarator")?, source)?;
            let len = node
                .child_by_field_name("size")
                .and_then(|size| super::literal_u64(size, source));
            Some((name, pointer, len))
        }
        "parenthesized_declarator" => {
            unwrap_declarator(named_children(node).into_iter().next()?, source)
        }
        _ => None,
    }
}

/// Whether a type node names an unsigned integer type.
fn is_unsigned_type(type_node: Node, source: &str) -> bool {
    let name = text(source, type_node);
    match type_node.kind() {
        // `unsigned`, `unsigned int`, `unsigned long`, …
        "sized_type_specifier" => name.contains("unsigned"),
        // This grammar classes known typedefs like `size_t`/`uint32_t` as
        // primitive types, so both kinds consult the unsigned-typedef list.
        "primitive_type" | "type_identifier" => name == "unsigned" || is_unsigned_typedef(name),
        _ => false,
    }
}

/// Common typedefs that are unambiguously unsigned.
fn is_unsigned_typedef(name: &str) -> bool {
    matches!(
        name,
        "size_t"
            | "uintptr_t"
            | "uintmax_t"
            | "uint8_t"
            | "uint16_t"
            | "uint32_t"
            | "uint64_t"
            | "uint"
            | "uchar"
            | "ushort"
            | "ulong"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u_int"
            | "u_char"
            | "u_short"
            | "u_long"
    )
}
