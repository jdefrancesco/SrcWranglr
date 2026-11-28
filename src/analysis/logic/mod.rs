//! Static logic-bug checks over each function's tree-sitter AST, independent of
//! any library call. Where the risky-call and memory walks key off *what a
//! function calls*, these find mistakes in the code's own comparisons and
//! control flow: an assignment used as a condition (`if (x = y)`), an `&`/`|`
//! precedence surprise (`x & M == v`), a self-comparison (`a == a`), a
//! self-assignment (`x = x`), and an allocation whose NULL return is
//! dereferenced unchecked. Each becomes a `Finding`, so the Findings pane,
//! triage and the agent's `findings` tool pick them up like any other.

mod allocation;
mod condition;
mod leak;
mod uninit;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use tree_sitter::Node;

use crate::analysis::parse::{cpp_parser, named_children, text};
use crate::analysis::{Finding, Severity};
use crate::index::functions_holding_lines;
use crate::model::{CodeGraph, SymbolId};

/// The tool stamped on logic findings — their own class in the Findings pane,
/// apart from the risky-call checks, and triaged like any native finding.
pub const TOOL: &str = "SrcWranglr logic";

/// One suspected logic bug at a single place, before its enclosing symbol is
/// resolved.
pub(super) struct Flaw {
    rule: &'static str,
    cwe: u32,
    severity: Severity,
    message: String,
    fix: Option<String>,
    /// 1-based line.
    line: u32,
    /// Byte columns `start..end` on `line`, when it stays on one line.
    columns: Option<(u32, u32)>,
}

/// Logic-bug findings across the whole project.
pub fn findings(graph: &CodeGraph) -> Vec<Finding> {
    let mut located: Vec<(&str, Flaw)> = Vec::new();
    for (file, source) in &graph.file_contents {
        let Some(mut parser) = cpp_parser() else {
            return Vec::new();
        };
        let Some(tree) = parser.parse(source, None) else {
            continue;
        };
        let mut flaws = Vec::new();
        walk(tree.root_node(), source, &mut flaws);
        located.extend(flaws.into_iter().map(|flaw| (file.as_str(), flaw)));
    }
    let places: Vec<(&str, u32)> = located
        .iter()
        .map(|(file, flaw)| (*file, flaw.line))
        .collect();
    let symbols = functions_holding_lines(graph, &places);
    located
        .into_iter()
        .zip(symbols)
        .map(|((file, flaw), symbol)| flaw.into_finding(file.to_string(), symbol))
        .collect()
}

impl Flaw {
    fn into_finding(self, file: String, symbol: Option<SymbolId>) -> Finding {
        Finding {
            tool: TOOL.to_string(),
            rule: self.rule.to_string(),
            severity: self.severity,
            message: self.message,
            fix: self.fix,
            cwe: Some(self.cwe),
            file,
            line: self.line,
            columns: self.columns,
            symbol,
        }
    }
}

/// Run the node-local condition checks on every node, and the allocation check
/// once per function body.
fn walk(node: Node, source: &str, flaws: &mut Vec<Flaw>) {
    condition::check(node, source, flaws);
    if node.kind() == "function_definition" {
        if let Some(body) = node.child_by_field_name("body") {
            allocation::check(body, source, flaws);
            uninit::check(body, source, flaws);
            leak::check(body, source, flaws);
        }
    }
    for child in named_children(node) {
        walk(child, source, flaws);
    }
}

/// A node's operator token (`+`, `==`, `=`, ...), or `""` if it has none.
pub(super) fn operator<'s>(node: Node, source: &'s str) -> &'s str {
    node.child_by_field_name("operator")
        .map(|op| text(source, op))
        .unwrap_or_default()
}

/// The byte columns a node spans, if it stays on one line.
pub(super) fn columns_of(node: Node) -> Option<(u32, u32)> {
    let (start, end) = (node.start_position(), node.end_position());
    (start.row == end.row).then_some((start.column as u32, end.column as u32))
}

/// The first identifier inside a declarator (`*p`, `**p`, `p` all yield `p`).
pub(super) fn leaf_identifier<'s>(node: Node, source: &'s str) -> Option<&'s str> {
    if node.kind() == "identifier" {
        return Some(text(source, node));
    }
    named_children(node)
        .into_iter()
        .find_map(|child| leaf_identifier(child, source))
}

/// Collect every `identifier` leaf in source order, not descending into nested
/// functions (they have their own flow).
pub(super) fn identifier_nodes<'t>(node: Node<'t>, out: &mut Vec<Node<'t>>) {
    match node.kind() {
        "function_definition" | "lambda_expression" => return,
        "identifier" => out.push(node),
        _ => {}
    }
    for child in named_children(node) {
        identifier_nodes(child, out);
    }
}
