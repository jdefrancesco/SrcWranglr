//! Which writes in a function can count for a use, by where they sit: the
//! declaration a name means at each point (C/C++ block scope, so a
//! variable in a closed block is another variable), branches that exclude
//! each other, and the loop around a use (whose later writes reach it on
//! the next pass).

use std::collections::HashMap;

use tree_sitter::Node;

use crate::index::syntax::text;

/// Walking up from a node gives up past this many parents.
const MAX_ANCESTORS: usize = 256;

/// Nodes that open a scope: a declaration in one is seen until it ends.
const SCOPES: &[&str] = &[
    "compound_statement",
    "for_statement",
    "for_range_loop",
    "if_statement",
    "while_statement",
    "switch_statement",
    "catch_clause",
    "lambda_expression",
    "function_definition",
];
const LOOPS: &[&str] = &[
    "for_statement",
    "for_range_loop",
    "while_statement",
    "do_statement",
];

/// A local declaration of the traced name.
#[derive(Debug, Clone, Copy)]
pub(super) struct Declaration {
    /// Where its name is written: what identifies it.
    pub at: usize,
    /// The scope it is seen in.
    scope: (usize, usize),
}

impl Declaration {
    /// The declaration whose name is the node `name`.
    pub(super) fn of(name: Node) -> Self {
        let scope = scope_of(name).map_or((0, usize::MAX), |s| (s.start_byte(), s.end_byte()));
        Self {
            at: name.start_byte(),
            scope,
        }
    }
}

/// The declaration a name written at byte `at` means: the innermost one
/// seen there, declared at or before it. `None`: a parameter, or a name
/// from outside the function.
pub(super) fn binding(declarations: &[Declaration], at: usize) -> Option<usize> {
    declarations
        .iter()
        .filter(|d| d.at <= at && d.scope.0 <= at && at < d.scope.1)
        .max_by_key(|d| (d.scope.0, d.at))
        .map(|d| d.at)
}

fn scope_of(node: Node) -> Option<Node> {
    let mut current = node.parent()?;
    for _ in 0..MAX_ANCESTORS {
        if SCOPES.contains(&current.kind()) {
            return Some(current);
        }
        current = current.parent()?;
    }
    None
}

/// Whether `node` is in one arm of an `if` (or `?:`) and byte `at` in the
/// other, so it never runs before `at` does — unless a loop around them
/// both runs it on an earlier pass.
pub(super) fn is_exclusive(node: Node, at: usize, function: Node) -> bool {
    let mut child = node;
    let mut is_split = false;
    for _ in 0..MAX_ANCESTORS {
        let Some(parent) = child.parent() else {
            break;
        };
        if parent == function {
            break;
        }
        if LOOPS.contains(&parent.kind()) && contains(parent, at) {
            return false;
        }
        if matches!(parent.kind(), "if_statement" | "conditional_expression") {
            let arm = |name: &str| parent.child_by_field_name(name);
            let other = match child {
                c if arm("consequence") == Some(c) => arm("alternative"),
                c if arm("alternative") == Some(c) => arm("consequence"),
                _ => None,
            };
            is_split |= other.is_some_and(|o| contains(o, at));
        }
        child = parent;
    }
    is_split
}

/// The loop in `function` whose next pass can bring a later write back to
/// byte `at`: the outermost one around `at` that the variable (declared at
/// `declared`, if local) isn't declared in, since a variable declared in a
/// loop is a new one each pass. A loop whose once-only start (`for`'s
/// initializer, a range-`for`'s range) holds `at` doesn't repeat it.
pub(super) fn loop_around<'t>(
    function: Node<'t>,
    at: usize,
    declared: Option<usize>,
) -> Option<Node<'t>> {
    let mut current = function.descendant_for_byte_range(at, at)?;
    let mut outermost = None;
    for _ in 0..MAX_ANCESTORS {
        if current == function {
            break;
        }
        if LOOPS.contains(&current.kind()) {
            if declared.is_some_and(|d| contains(current, d)) {
                break;
            }
            let once = ["initializer", "right"]
                .iter()
                .filter_map(|f| current.child_by_field_name(f))
                .any(|part| contains(part, at));
            if !once {
                outermost = Some(current);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    outermost
}

/// Where control can jump in a function: its `break`, `continue` and
/// `goto` statements, and where its labels are.
pub(super) struct Jumps<'t> {
    /// `break` and `continue` statements, with what each jumps out of (if
    /// found).
    exits: Vec<(Node<'t>, Option<Node<'t>>)>,
    /// `goto` statements, with where their label is (if found).
    gotos: Vec<(Node<'t>, Option<usize>)>,
}

impl<'t> Jumps<'t> {
    pub(super) fn of(function: Node<'t>, source: &str) -> Self {
        let find = |word: &str, kind: &str| statements(function, source, word, kind);
        let exits = [
            ("break", "break_statement"),
            ("continue", "continue_statement"),
        ]
        .iter()
        .flat_map(|(word, kind)| find(word, kind))
        .map(|jump| (jump, target_of(jump, jump.kind() == "break_statement")))
        .collect();
        let goto_nodes = find("goto", "goto_statement");
        let labels: HashMap<&str, usize> = if goto_nodes.is_empty() {
            HashMap::new()
        } else {
            labels_in(function, source)
        };
        let gotos = goto_nodes
            .into_iter()
            .map(|g| {
                let label = g.child_by_field_name("label").map(|l| text(l, source));
                (g, label.and_then(|l| labels.get(l).copied()))
            })
            .collect();
        Self { exits, gotos }
    }

    /// Whether a jump before `node` can go past it to `at`: a `break` or
    /// `continue` out of a loop (or `switch`) that holds `node` but not
    /// `at`, or a `goto` to a label between them.
    pub(super) fn can_skip(&self, node: Node, at: usize) -> bool {
        let before = |jump: &Node| jump.start_byte() < node.start_byte();
        let exits_past = self
            .exits
            .iter()
            .filter(|(jump, _)| before(jump))
            .any(|(_, target)| {
                target.is_some_and(|t| contains(t, node.start_byte()) && !contains(t, at))
            });
        let jumps_past = self
            .gotos
            .iter()
            .filter(|(j, _)| before(j))
            .any(|(_, label)| label.is_none_or(|l| node.end_byte() <= l && l <= at));
        exits_past || jumps_past
    }
}

/// The statements of `kind` in `function`, found by their keyword.
fn statements<'t>(function: Node<'t>, source: &str, word: &str, kind: &str) -> Vec<Node<'t>> {
    let Some(body) = source.get(function.byte_range()) else {
        return Vec::new();
    };
    body.match_indices(word)
        .filter_map(|(i, _)| {
            let at = function.start_byte() + i;
            let node = function.descendant_for_byte_range(at, at + word.len())?;
            [Some(node), node.parent()]
                .into_iter()
                .flatten()
                .find(|n| n.kind() == kind && n.start_byte() == at)
        })
        .collect()
}

/// Where each label in `function` is.
fn labels_in<'a>(function: Node, source: &'a str) -> HashMap<&'a str, usize> {
    let mut labels = HashMap::new();
    let mut stack = vec![function];
    while let Some(node) = stack.pop() {
        if node.kind() == "labeled_statement" {
            if let Some(label) = node.child_by_field_name("label") {
                labels.insert(text(label, source), node.start_byte());
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    labels
}

/// What a `break` (also leaving a `switch`) or `continue` jumps out of.
fn target_of(jump: Node, is_break: bool) -> Option<Node> {
    let mut current = jump.parent()?;
    for _ in 0..MAX_ANCESTORS {
        let kind = current.kind();
        if LOOPS.contains(&kind) || (is_break && kind == "switch_statement") {
            return Some(current);
        }
        if kind == "function_definition" || kind == "lambda_expression" {
            return None;
        }
        current = current.parent()?;
    }
    None
}

pub(super) fn contains(node: Node, at: usize) -> bool {
    node.start_byte() <= at && at < node.end_byte()
}
