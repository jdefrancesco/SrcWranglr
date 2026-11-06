//! The project's files, parsed when a trace first needs them, and finding
//! things in them: the call or variable at a position, the function around
//! a point, a function's parameters.

use std::collections::HashMap;

use tree_sitter::{Node, Parser, Point, Tree};

use super::Step;
use crate::index::syntax::{
    clean_name, declarator_name, find_function_declarator, line_of, name_leaf, named_children,
    span_of, text,
};
use crate::model::{CodeGraph, EdgeKind, OccurrenceRole, SymbolId};

/// A step's code line is cut to this many characters.
const MAX_CODE_CHARS: usize = 200;
/// Code quoted in a note is cut to this many characters.
const MAX_SNIPPET_CHARS: usize = 60;
/// Finding a node by walking up from a point gives up past this depth.
const MAX_ANCESTORS: usize = 64;

/// Parsed project files, for one trace.
pub(super) struct Files<'g> {
    graph: &'g CodeGraph,
    parser: Option<Parser>,
    trees: HashMap<String, Option<Tree>>,
}

impl<'g> Files<'g> {
    pub(super) fn new(graph: &'g CodeGraph) -> Self {
        let mut parser = Parser::new();
        let parser = parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .ok()
            .map(|()| parser);
        Self {
            graph,
            parser,
            trees: HashMap::new(),
        }
    }

    pub(super) fn graph(&self) -> &'g CodeGraph {
        self.graph
    }

    pub(super) fn source(&self, file: &str) -> &'g str {
        self.graph
            .file_contents
            .get(file)
            .map_or("", String::as_str)
    }

    /// `file`'s syntax tree (a cheap copy of the one kept).
    pub(super) fn tree(&mut self, file: &str) -> Option<Tree> {
        if !self.trees.contains_key(file) {
            let source = self.source(file);
            let tree = self.parser.as_mut().and_then(|p| p.parse(source, None));
            self.trees.insert(file.to_string(), tree);
        }
        self.trees.get(file).cloned().flatten()
    }
}

/// The function definition around byte `at`.
pub(super) fn function_around(root: Node, at: usize) -> Option<Node> {
    let mut node = root.descendant_for_byte_range(at, at)?;
    for _ in 0..MAX_ANCESTORS {
        if node.kind() == "function_definition" {
            return Some(node);
        }
        node = node.parent()?;
    }
    None
}

/// The function definition that starts on 1-based `line`.
pub(super) fn function_on_line(root: Node, line: u32) -> Option<Node> {
    let row = usize::try_from(line.checked_sub(1)?).ok()?;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.end_position().row < row || node.start_position().row > row {
            continue;
        }
        if node.kind() == "function_definition" && node.start_position().row == row {
            return Some(node);
        }
        stack.extend(named_children(node).into_iter().rev());
    }
    None
}

fn point(line: u32, column: u32) -> Option<Point> {
    let row = usize::try_from(line.checked_sub(1)?).ok()?;
    Some(Point::new(row, usize::try_from(column).ok()?))
}

/// The call whose function name is written at `line`, `column`.
pub(super) fn call_named_at(root: Node, line: u32, column: u32) -> Option<Node> {
    let at = point(line, column)?;
    let mut node = root.descendant_for_point_range(at, at);
    for _ in 0..MAX_ANCESTORS {
        let current = node?;
        let names_it = current
            .child_by_field_name("function")
            .is_some_and(|f| f.start_position() <= at && at < f.end_position());
        if current.kind() == "call_expression" && names_it {
            return Some(current);
        }
        node = current.parent();
    }
    None
}

/// The call a finding at `line`, `column` is about: the one named there,
/// else the first call starting on that line at or after `column`.
pub(super) fn call_at(root: Node, line: u32, column: u32) -> Option<Node> {
    if let Some(call) = call_named_at(root, line, column) {
        return Some(call);
    }
    let at = point(line, column)?;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let (start, end) = (node.start_position(), node.end_position());
        if end.row < at.row || start.row > at.row {
            continue;
        }
        if node.kind() == "call_expression" && start.row == at.row && start.column >= at.column {
            return Some(node);
        }
        stack.extend(named_children(node).into_iter().rev());
    }
    None
}

/// The call that starts at byte `at`.
pub(super) fn call_starting_at(root: Node, at: usize) -> Option<Node> {
    let mut node = root.descendant_for_byte_range(at, at);
    for _ in 0..MAX_ANCESTORS {
        let current = node?;
        if current.kind() == "call_expression" && current.start_byte() == at {
            return Some(current);
        }
        node = current.parent();
    }
    None
}

/// The variable written at `line`, `column`: a name, or the whole `a->b`
/// on a field's name.
pub(super) fn variable_at(root: Node, line: u32, column: u32) -> Option<Node> {
    let at = point(line, column)?;
    let node = root.descendant_for_point_range(at, at)?;
    match node.kind() {
        "identifier" => Some(node),
        "field_identifier" => node.parent().filter(|p| p.kind() == "field_expression"),
        _ => None,
    }
}

/// The name a call calls: `strcpy`, `std::strcpy`, or a method's name.
pub(super) fn called_name(function: Node, source: &str) -> String {
    match function.kind() {
        "field_expression" => function
            .child_by_field_name("field")
            .map_or_else(String::new, |f| clean_name(text(f, source))),
        _ => clean_name(text(function, source)),
    }
}

/// The node of a call's `function` the index records the call at.
pub(super) fn call_name_node(function: Node) -> Node {
    match function.kind() {
        "field_expression" => function.child_by_field_name("field").unwrap_or(function),
        _ => name_leaf(function),
    }
}

/// The project function a call's `function` node calls, if the index
/// resolved it.
pub(super) fn callee(graph: &CodeGraph, file: &str, function: Node) -> Option<SymbolId> {
    let span = span_of(call_name_node(function))?;
    graph
        .occurrences_on_line(file, span.line)
        .iter()
        .find(|o| {
            o.role == OccurrenceRole::Reference(EdgeKind::Calls) && o.span.start == span.start
        })
        .map(|o| o.symbol)
}

/// The indexed symbol of the function definition `function` in `file`.
pub(super) fn symbol_of(graph: &CodeGraph, file: &str, function: Node) -> Option<SymbolId> {
    let line = line_of(function);
    graph
        .functions
        .iter()
        .find(|f| f.file == file && f.line == line)
        .map(|f| f.symbol)
}

/// Where the definitions of `symbol` a call written in `call_file` may call
/// are: each one's file and first line.
pub(super) fn definitions(
    graph: &CodeGraph,
    symbol: SymbolId,
    call_file: &str,
) -> Vec<(String, u32)> {
    graph
        .functions
        .iter()
        .filter(|f| f.symbol == symbol && may_call(graph, symbol, (&f.file, f.line), call_file))
        .map(|f| (f.file.clone(), f.line))
        .collect()
}

/// Whether a call written in `call_file` may call the definition of
/// `symbol` at `definition` (file, line). The index merges same-named
/// functions into one symbol, so: a call in a file with its own
/// definition calls that one, and a `static` one outside a header is only
/// called from its own file.
pub(super) fn may_call(
    graph: &CodeGraph,
    symbol: SymbolId,
    definition: (&str, u32),
    call_file: &str,
) -> bool {
    let (file, line) = definition;
    if file == call_file {
        return true;
    }
    let mut defs = graph.functions.iter().filter(|f| f.symbol == symbol);
    let is_static = defs
        .clone()
        .find(|f| f.file == file && f.line == line)
        .is_some_and(|f| f.signature.is_static);
    let has_own = defs.any(|f| f.file == call_file);
    !has_own && (!is_static || is_header(file))
}

fn is_header(file: &str) -> bool {
    let extension = file.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "h" | "hh" | "hpp" | "hxx" | "h++" | "inl" | "ipp" | "tpp"
    )
}

/// A function definition's name as written: `main`, `Foo::bar`.
pub(super) fn function_name(function: Node, source: &str) -> String {
    function
        .child_by_field_name("declarator")
        .and_then(find_function_declarator)
        .and_then(|f| f.child_by_field_name("declarator"))
        .map_or_else(String::new, |n| clean_name(text(n, source)))
}

/// A function definition's parameters, as their names' nodes (`None` for
/// an unnamed one).
pub(super) fn parameters(function: Node) -> Vec<Option<Node>> {
    let Some(list) = function
        .child_by_field_name("declarator")
        .and_then(find_function_declarator)
        .and_then(|f| f.child_by_field_name("parameters"))
    else {
        return Vec::new();
    };
    named_children(list)
        .into_iter()
        .filter(|p| p.kind().ends_with("parameter_declaration"))
        .map(|p| {
            p.child_by_field_name("declarator")
                .and_then(declarator_name)
        })
        .collect()
}

/// A step at `node` in `file`.
pub(super) fn step(
    source: &str,
    file: &str,
    node: Node,
    symbol: Option<SymbolId>,
    note: String,
) -> Step {
    let line = line_of(node);
    Step {
        file: file.to_string(),
        line,
        span: span_of(node),
        symbol,
        code: code_line(source, line),
        note,
    }
}

/// Line `line` (1-based) of `source`, trimmed and cut short.
fn code_line(source: &str, line: u32) -> String {
    let index = usize::try_from(line.saturating_sub(1)).unwrap_or(usize::MAX);
    cut(
        source.lines().nth(index).unwrap_or("").trim(),
        MAX_CODE_CHARS,
    )
}

/// `node`'s code for a note: whitespace collapsed, cut short.
pub(super) fn snippet(node: Node, source: &str) -> String {
    let compact = text(node, source)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cut(&compact, MAX_SNIPPET_CHARS)
}

fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}
