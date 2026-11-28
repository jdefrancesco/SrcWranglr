//! Within one function body, a resource opened and then dropped: `fopen`,
//! `open`, `opendir`, `popen` and friends assigned to a local that is never
//! closed, returned, or handed anywhere before the function ends — or reopened
//! into the same variable with nothing in between (CWE-775). The reasoning is
//! deliberately conservative and source-order based: *any* later mention of the
//! handle (a call, a return, a reassignment, an address-of) clears the
//! suspicion, so only a handle that truly goes nowhere, or is overwritten
//! immediately, is flagged. It does not chase leaks on one branch of a
//! close-on-some-paths control flow — that boundary keeps false positives near
//! zero.

use tree_sitter::Node;

use super::{columns_of, operator, Flaw};
use crate::analysis::parse::{line_of, text};
use crate::analysis::Severity;

const OPENERS: [&str; 8] = [
    "fopen", "fdopen", "tmpfile", "popen", "open", "opendir", "creat", "socket",
];

/// Flag each opened handle whose next mention is nothing, or another open of the
/// same variable.
pub(super) fn check(body: Node, source: &str, flaws: &mut Vec<Flaw>) {
    let mut ids = Vec::new();
    super::identifier_nodes(body, &mut ids);
    for (position, id) in ids.iter().enumerate() {
        let Some(open_line) = opens_at(*id, source) else {
            continue;
        };
        let name = text(source, *id);
        let next = ids[position + 1..]
            .iter()
            .find(|later| text(source, **later) == name);
        let leaked = match next {
            // Nothing mentions the handle again: it is simply dropped.
            None => true,
            // Reopened into the same variable with nothing in between: the
            // earlier handle is lost. Any other use (a close, a read, a return)
            // clears it.
            Some(later) => opens_at(*later, source).is_some(),
        };
        if leaked {
            flaws.push(flaw(name, open_line, columns_of(*id)));
        }
    }
}

fn flaw(name: &str, open_line: u32, columns: Option<(u32, u32)>) -> Flaw {
    Flaw {
        rule: "resource-leak",
        cwe: 775,
        severity: Severity::Medium,
        message: format!(
            "`{name}` is opened here but never closed, returned, or handed off before the \
             function ends — the resource leaks every time this runs."
        ),
        fix: Some(format!(
            "close `{name}` on every path out of the function, or hand ownership to the caller"
        )),
        line: open_line,
        columns,
    }
}

/// If `id` names the target of `T *id = opener(…)` or `id = opener(…)`, the line
/// of that open; otherwise `None`.
fn opens_at(id: Node, source: &str) -> Option<u32> {
    let mut node = id;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "pointer_declarator" | "array_declarator" | "parenthesized_declarator" => node = parent,
            "init_declarator" if parent.child_by_field_name("declarator") == Some(node) => {
                let value = parent.child_by_field_name("value")?;
                return is_opener(value, source).then(|| line_of(parent));
            }
            "assignment_expression"
                if parent.child_by_field_name("left") == Some(node)
                    && operator(parent, source) == "=" =>
            {
                let value = parent.child_by_field_name("right")?;
                return is_opener(value, source).then(|| line_of(parent));
            }
            _ => return None,
        }
    }
    None
}

/// Whether `node`, past any parentheses, is a call to a known resource opener.
fn is_opener(node: Node, source: &str) -> bool {
    let mut node = node;
    while node.kind() == "parenthesized_expression" {
        match node.child_by_field_name("value") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .is_some_and(|f| OPENERS.contains(&text(source, f)))
}
