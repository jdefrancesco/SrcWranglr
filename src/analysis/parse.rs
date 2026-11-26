//! Small shared tree-sitter helpers for the C/C++ analyses (the memory-safety
//! walk and the emulator), so the parser setup and node plumbing live in one
//! place.

use tree_sitter::{Node, Parser};

/// A tree-sitter parser for C/C++, or `None` if the grammar won't load.
pub(crate) fn cpp_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .ok()?;
    Some(parser)
}

/// The named children of `node`, collected.
pub(crate) fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// The source text a node spans.
pub(crate) fn text<'s>(source: &'s str, node: Node) -> &'s str {
    source.get(node.byte_range()).unwrap_or_default()
}

/// The 1-based line a node starts on.
pub(crate) fn line_of(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

/// The innermost `function_definition` whose lines contain `line` (1-based).
pub(crate) fn function_containing(root: Node, line: u32) -> Option<Node> {
    let row = line.saturating_sub(1) as usize;
    let mut found = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let holds = node.start_position().row <= row && row <= node.end_position().row;
        if node.kind() == "function_definition" && holds {
            found = Some(node);
        }
        for child in named_children(node) {
            stack.push(child);
        }
    }
    found
}
