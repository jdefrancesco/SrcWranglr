//! Where a value goes from where it is used, read from the syntax: up
//! through what passes it on (arithmetic, a cast, `*`, `&`, a method called
//! on it) to the variable it is stored in, the call it is handed to, or the
//! `return` it leaves by. A condition, a comparison or an index uses it
//! without passing it on.

use tree_sitter::Node;

use super::defs::arguments;
use super::flows::access_path;
use crate::index::syntax::{declarator_name, field_children, text};

/// Walking up from a use gives up past this many parents.
const MAX_UP: usize = 64;
/// More places a name is written in one function than this aren't read.
const MAX_MATCHES: usize = 4096;
/// Operators whose result is made from their operands' values.
const CARRYING: &[&str] = &["+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^"];
/// What a declarator is wrapped in on the way up to its declaration.
const DECLARATOR_WRAPPERS: &[&str] = &[
    "pointer_declarator",
    "array_declarator",
    "function_declarator",
    "init_declarator",
];
const DECLARATIONS: &[&str] = &[
    "declaration",
    "parameter_declaration",
    "optional_parameter_declaration",
    "field_declaration",
    "for_range_loop",
];

/// Where a value ends up from a use.
#[derive(Debug, Clone, Copy)]
pub(super) enum Goes<'t> {
    /// Stored by `write` into `target` (a variable, field or element), whose
    /// new value counts from byte `from`.
    Stored {
        write: Node<'t>,
        target: Node<'t>,
        from: usize,
    },
    /// Handed to `call` as its argument `index`.
    Argument { call: Node<'t>, index: usize },
    /// Returned from the function it is in.
    Returned,
    /// Used where it goes no further.
    Used,
}

/// Where the value at `node` goes, and the expression whose value it is
/// there (`argv[1]` for `argv`).
pub(super) fn goes<'t>(node: Node<'t>, source: &str) -> (Goes<'t>, Node<'t>) {
    let mut child = node;
    for _ in 0..MAX_UP {
        let Some(parent) = child.parent() else {
            break;
        };
        if let Some(end) = ends_at(parent, child) {
            return (end, child);
        }
        child = match method_call(parent, child) {
            Some(call) => call,
            None if passes_on(parent, child, source) => parent,
            None => break,
        };
    }
    (Goes::Used, child)
}

/// What `parent` does with `child` when that is where the value ends up.
fn ends_at<'t>(parent: Node<'t>, child: Node<'t>) -> Option<Goes<'t>> {
    let is = |field: &str| parent.child_by_field_name(field) == Some(child);
    let declared = |p: Node<'t>| {
        p.child_by_field_name("declarator")
            .and_then(declarator_name)
    };
    let stored = |target: Node<'t>, from: usize| Goes::Stored {
        write: parent,
        target,
        from,
    };
    match parent.kind() {
        "argument_list" => {
            let call = parent.parent().filter(|c| c.kind() == "call_expression")?;
            let index = arguments(call).iter().position(|a| *a == child)?;
            Some(Goes::Argument { call, index })
        }
        "assignment_expression" if is("right") => Some(stored(
            parent.child_by_field_name("left")?,
            parent.end_byte(),
        )),
        // `T x = v`, and `if (T x = v)`.
        "init_declarator" | "declaration" if is("value") => {
            Some(stored(declared(parent)?, parent.end_byte()))
        }
        "for_range_loop" if is("right") => {
            let declarator = parent.child_by_field_name("declarator")?;
            Some(stored(declarator_name(declarator)?, declarator.end_byte()))
        }
        "return_statement" => Some(Goes::Returned),
        _ => None,
    }
}

/// The call when `parent` calls a method of `child`: what it returns comes
/// from the object (`s.c_str()`).
fn method_call<'t>(parent: Node<'t>, child: Node<'t>) -> Option<Node<'t>> {
    let is_object = parent.kind() == "field_expression"
        && parent.child_by_field_name("argument") == Some(child);
    let call = parent.parent().filter(|c| c.kind() == "call_expression")?;
    (is_object && call.child_by_field_name("function") == Some(parent)).then_some(call)
}

/// Whether `parent`'s value is made from `child`'s.
fn passes_on(parent: Node, child: Node, source: &str) -> bool {
    let is = |field: &str| parent.child_by_field_name(field) == Some(child);
    let operator = || {
        parent
            .child_by_field_name("operator")
            .map_or("", |o| text(o, source))
    };
    match parent.kind() {
        "parenthesized_expression"
        | "cast_expression"
        | "pointer_expression"
        | "update_expression"
        | "initializer_list"
        | "compound_literal_expression" => true,
        "initializer_pair" => is("value"),
        // An element or field of it, not an index into something else.
        "subscript_expression" | "field_expression" => is("argument"),
        "binary_expression" => CARRYING.contains(&operator()),
        "unary_expression" => matches!(operator(), "-" | "+" | "~"),
        "conditional_expression" => !is("condition"),
        "comma_expression" => is("right"),
        _ => false,
    }
}

/// A use of a variable's value: where its name is, and the access written
/// there (the name, or `name->a.b`) with the fields it goes through.
pub(super) struct Use<'t> {
    pub name: Node<'t>,
    pub access: Node<'t>,
    pub fields: Vec<String>,
}

/// Where `function` uses the value of the variable `name`, in source
/// order: not where it is declared or written to. And whether the name
/// is written so often that some were left out.
pub(super) fn uses_of<'t>(function: Node<'t>, source: &str, name: &str) -> (Vec<Use<'t>>, bool) {
    let Some(body) = function.child_by_field_name("body") else {
        return (Vec::new(), false);
    };
    let Some(text_of_body) = source.get(body.byte_range()) else {
        return (Vec::new(), false);
    };
    let matches: Vec<usize> = text_of_body
        .match_indices(name)
        .map(|(i, _)| i)
        .take(MAX_MATCHES + 1)
        .collect();
    let is_partial = matches.len() > MAX_MATCHES;
    let uses = matches
        .into_iter()
        .take(MAX_MATCHES)
        .filter_map(|i| {
            let at = body.start_byte() + i;
            let node = body.descendant_for_byte_range(at, at + name.len())?;
            let is_name = node.kind() == "identifier"
                && node.start_byte() == at
                && text(node, source) == name;
            (is_name && is_read(node)).then(|| use_at(node, source))
        })
        .collect();
    (uses, is_partial)
}

/// The use whose name is `name`: the whole field path it starts.
fn use_at<'t>(name: Node<'t>, source: &str) -> Use<'t> {
    let mut access = name;
    while let Some(parent) = access.parent().filter(|p| {
        p.kind() == "field_expression" && p.child_by_field_name("argument") == Some(access)
    }) {
        access = parent;
    }
    let (_, fields) = access_path(access, source);
    Use {
        name,
        access,
        fields,
    }
}

/// Whether the identifier `node` reads the variable: not its declaration,
/// a write to it, or a call's name.
fn is_read(node: Node) -> bool {
    let parent = node.parent();
    let is_called = parent.is_some_and(|p| {
        p.kind() == "call_expression" && p.child_by_field_name("function") == Some(node)
    });
    !is_called && !is_declared_at(node) && super::walk::set_by(node).is_none()
}

/// Whether `node` is the name a declaration declares.
fn is_declared_at(node: Node) -> bool {
    let mut child = node;
    for _ in 0..MAX_UP {
        let Some(parent) = child.parent() else {
            return false;
        };
        let is_declarator = field_children(parent, "declarator").contains(&child);
        match parent.kind() {
            kind if DECLARATIONS.contains(&kind) => return is_declarator,
            "reference_declarator" | "parenthesized_declarator" => {}
            kind if DECLARATOR_WRAPPERS.contains(&kind) && is_declarator => {}
            _ => return false,
        }
        child = parent;
    }
    false
}

/// Whether a write to `target` (in a function whose parameters are
/// `parameters`) changes what the parameter at the returned index points
/// to, so the caller sees it: `p->f = …`, `*p = …`, `p[i] = …`, or any
/// write to a reference. `is_into_pointee`: a library call copies into
/// what `target` points to.
pub(super) fn through_parameter(
    parameters: &[Option<Node>],
    source: &str,
    target: Node,
    is_into_pointee: bool,
) -> Option<usize> {
    let (base, _) = access_path(target, source);
    let name = text(base, source);
    let index = parameters
        .iter()
        .position(|p| p.is_some_and(|p| text(p, source) == name))?;
    let is_reference = parameters[index].is_some_and(is_reference_parameter);
    (is_into_pointee || is_reference || is_indirect(target, base, source)).then_some(index)
}

/// Whether the parameter named `name` is declared as a reference.
fn is_reference_parameter(name: Node) -> bool {
    let mut current = name;
    for _ in 0..MAX_UP {
        let Some(parent) = current.parent() else {
            return false;
        };
        match parent.kind() {
            "reference_declarator" => return true,
            kind if DECLARATIONS.contains(&kind) => return false,
            _ => current = parent,
        }
    }
    false
}

/// Whether `target` writes through a pointer on the way down to `base`.
fn is_indirect(target: Node, base: Node, source: &str) -> bool {
    let mut current = target;
    for _ in 0..MAX_UP {
        if current == base {
            return false;
        }
        let operator = current
            .child_by_field_name("operator")
            .map_or("", |o| text(o, source));
        let is_through = match current.kind() {
            "subscript_expression" => true,
            "pointer_expression" => operator == "*",
            "field_expression" => operator == "->",
            _ => false,
        };
        if is_through {
            return true;
        }
        let next = current
            .child_by_field_name("argument")
            .or_else(|| current.child_by_field_name("value"))
            .or_else(|| current.named_child(0));
        match next {
            Some(next) => current = next,
            None => return false,
        }
    }
    false
}
