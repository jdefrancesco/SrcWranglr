//! Within one function body, a local scalar declared without an initializer and
//! then *read* before anything gives it a value: undefined behavior, and a
//! classic information-leak primitive (CWE-457). The reasoning is deliberately
//! conservative and source-order based — a variable's first mention after its
//! declaration decides it: a write (`x = …`), an address-of (`&x`), an `++x`,
//! or a `sizeof` all settle it as defined, and only a plain read as the first
//! mention is flagged. `static`/`extern`/`volatile` declarations are skipped
//! (they are not uninitialized), as is any function containing `goto`, whose
//! control flow can't be read off source order.

use tree_sitter::Node;

use super::{columns_of, leaf_identifier, operator, Flaw};
use crate::analysis::parse::{line_of, named_children, text};
use crate::analysis::Severity;

/// How one mention of a tracked variable uses it.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Use {
    /// The name being declared — not a use.
    Decl,
    /// Operand of `sizeof`/`_Alignof` — not evaluated, so not a use.
    Ignore,
    /// Written whole (`x = …`), address taken (`&x`), or stepped (`x++`): from
    /// here on it holds a value. Treated as safe even on a branch.
    Settled,
    /// Read as a value while still possibly uninitialized.
    Read,
}

/// A local tracked for this function: its name and the byte at which its
/// declaration ends, so only later mentions are weighed.
struct Local<'s> {
    name: &'s str,
    decl_end: usize,
}

/// Flag each uninitialized local whose first mention after declaration is a read.
pub(super) fn check(body: Node, source: &str, flaws: &mut Vec<Flaw>) {
    if contains_goto(body) {
        return;
    }
    let locals = declared_uninit(body, source);
    if locals.is_empty() {
        return;
    }
    let mut ids = Vec::new();
    super::identifier_nodes(body, &mut ids);
    for local in &locals {
        if let Some(read) = first_read(&ids, source, local) {
            flaws.push(flaw(local.name, read));
        }
    }
}

/// The line and columns of the first read of `local`, or `None` if it is
/// settled (written/escaped) or never mentioned before any read.
fn first_read(ids: &[Node], source: &str, local: &Local) -> Option<(u32, Option<(u32, u32)>)> {
    for id in ids {
        if id.start_byte() < local.decl_end || text(source, *id) != local.name {
            continue;
        }
        match classify(*id, source) {
            Use::Decl | Use::Ignore => continue,
            Use::Settled => return None,
            Use::Read => return Some((line_of(*id), columns_of(*id))),
        }
    }
    None
}

fn flaw(name: &str, read: (u32, Option<(u32, u32)>)) -> Flaw {
    Flaw {
        rule: "uninitialized-read",
        cwe: 457,
        severity: Severity::Medium,
        message: format!(
            "`{name}` is read here before it is given a value — it holds whatever was left on \
             the stack, which is undefined behavior and can disclose memory."
        ),
        fix: Some(format!(
            "initialize `{name}` where it is declared, or assign it before this read"
        )),
        line: read.0,
        columns: read.1,
    }
}

/// Decide how an occurrence of a tracked name is being used.
fn classify(id: Node, source: &str) -> Use {
    if in_sizeof(id) {
        return Use::Ignore;
    }
    if is_declared_name(id) {
        return Use::Decl;
    }
    let Some(parent) = id.parent() else {
        return Use::Read;
    };
    match parent.kind() {
        "assignment_expression" if parent.child_by_field_name("left") == Some(id) => {
            // `x = …` settles; a compound `x += …` reads `x` first.
            if operator(parent, source) == "=" {
                Use::Settled
            } else {
                Use::Read
            }
        }
        // `&x` — the address escapes, so a callee may fill it in. (Address-of is
        // a `pointer_expression` in this grammar; `*p` shares that kind but, with
        // the `*` operator, reads the pointer, so it falls through to `Read`.)
        "pointer_expression" | "unary_expression" if operator(parent, source) == "&" => {
            Use::Settled
        }
        // `x++` / `--x` — conservatively treated as settling the variable.
        "update_expression" => Use::Settled,
        _ => Use::Read,
    }
}

/// Whether `id` is the name being introduced by a declaration or parameter,
/// through any `*`/`[]`/`()` declarator wrappers.
fn is_declared_name(id: Node) -> bool {
    let mut node = id;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "pointer_declarator"
            | "array_declarator"
            | "function_declarator"
            | "parenthesized_declarator" => node = parent,
            "init_declarator" | "declaration" | "parameter_declaration" => {
                return parent.child_by_field_name("declarator") == Some(node);
            }
            _ => return false,
        }
    }
    false
}

/// Whether `id` sits inside a `sizeof`/`_Alignof` operand (never evaluated),
/// looking no further up than the enclosing statement or declaration.
fn in_sizeof(id: Node) -> bool {
    let mut node = id;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "sizeof_expression" | "alignof_expression" => return true,
            kind if kind.ends_with("statement")
                || kind == "declaration"
                || kind == "init_declarator" =>
            {
                return false
            }
            _ => node = parent,
        }
    }
    false
}

/// Every local declared without an initializer that we are willing to track.
fn declared_uninit<'s>(body: Node, source: &'s str) -> Vec<Local<'s>> {
    let mut out = Vec::new();
    collect_decls(body, source, &mut out);
    out
}

fn collect_decls<'s>(node: Node, source: &'s str, out: &mut Vec<Local<'s>>) {
    match node.kind() {
        "function_definition" | "lambda_expression" => return,
        "declaration" if !skippable(node, source) => {
            for declarator in named_children(node) {
                if let Some(name) = uninit_scalar_name(node, declarator, source) {
                    out.push(Local {
                        name,
                        decl_end: declarator.end_byte(),
                    });
                }
            }
        }
        _ => {}
    }
    for child in named_children(node) {
        collect_decls(child, source, out);
    }
}

/// A declaration we leave alone: `static`/`extern`/`register`/`typedef` storage
/// (not stack-uninitialized) or a `volatile` qualifier (reads may be intended).
fn skippable(declaration: Node, source: &str) -> bool {
    named_children(declaration).into_iter().any(|child| {
        child.kind() == "storage_class_specifier"
            || (child.kind() == "type_qualifier" && text(source, child) == "volatile")
    })
}

/// The name of a scalar local introduced by `declarator` with no initializer,
/// or `None` for initialized, aggregate, array, or function declarators.
fn uninit_scalar_name<'s>(declaration: Node, declarator: Node, source: &'s str) -> Option<&'s str> {
    match declarator.kind() {
        // A plain name is a scalar only if its type is a built-in scalar; a
        // bare `struct S s;` / typedef may be an aggregate we can't reason about.
        "identifier" => scalar_type(declaration).then(|| text(source, declarator)),
        // A pointer local is scalar whatever it points at.
        "pointer_declarator" => leaf_identifier(declarator, source),
        _ => None,
    }
}

/// Whether the declaration's type is a built-in scalar (`int`, `unsigned long`…).
fn scalar_type(declaration: Node) -> bool {
    declaration
        .child_by_field_name("type")
        .is_some_and(|ty| matches!(ty.kind(), "primitive_type" | "sized_type_specifier"))
}

/// Whether the body contains a `goto`, whose jumps defeat source-order reasoning.
fn contains_goto(node: Node) -> bool {
    if node.kind() == "goto_statement" {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| contains_goto(child))
}
