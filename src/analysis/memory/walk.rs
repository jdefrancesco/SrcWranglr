//! Walking a function body in source order to track each pointer's lifetime
//! — freed, used, freed again, or reassigned. A dereference or pass-to-a-call
//! of a freed pointer is a use-after-free, a second free a double-free.
//! Branches merge conservatively: freed on every path stays certain (high),
//! freed on some path becomes "maybe" (medium). The same walk feeds the
//! Findings (`findings`) and the per-function invariant matrix (`lifetimes`).
//! Intraprocedural and syntactic — a lead to confirm.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::names;
use super::state::{merge, Freed};
use crate::analysis::{Finding, Severity};
use crate::index::functions_holding_lines;
use crate::model::{CodeGraph, SymbolId};

const USE_AFTER_FREE: &str = "use-after-free";
const DOUBLE_FREE: &str = "double-free";

/// Which freed variables are live at a point, and where each was freed.
type State = HashMap<String, Freed>;

/// What happened to a tracked pointer at one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    /// The pointer was freed here.
    Freed,
    /// It was already freed and is freed again (double-free).
    FreedAgain,
    /// It is dereferenced or passed to a call while freed (use-after-free).
    Used,
    /// It was assigned a new value, so it is live again.
    Reassigned,
}

/// One thing the walk saw happen to a pointer.
struct Event {
    kind: PointKind,
    file: String,
    line: u32,
    columns: Option<(u32, u32)>,
    variable: String,
    /// For a violation: freed on every path (high) vs only some (medium).
    certain: bool,
    /// For a violation: the line the pointer was freed on.
    freed_at: u32,
}

/// Use-after-free and double-free findings across the project.
pub fn findings(graph: &CodeGraph) -> Vec<Finding> {
    let violations: Vec<Event> = all_events(graph)
        .into_iter()
        .filter(|e| matches!(e.kind, PointKind::Used | PointKind::FreedAgain))
        .collect();
    let places: Vec<(&str, u32)> = violations
        .iter()
        .map(|e| (e.file.as_str(), e.line))
        .collect();
    let symbols = functions_holding_lines(graph, &places);
    violations
        .into_iter()
        .zip(symbols)
        .map(|(event, symbol)| event.into_finding(symbol))
        .collect()
}

/// The pointer lifetimes in the function containing `line` of `file` — the
/// invariant matrix for one function. `None` if that function frees nothing.
pub fn lifetimes(graph: &CodeGraph, file: &str, line: u32) -> Option<Lifetimes> {
    let source = graph.file_contents.get(file)?;
    let mut parser = new_parser()?;
    let tree = parser.parse(source, None)?;
    let body = function_body_containing(tree.root_node(), line)?;
    let mut events = Vec::new();
    walk_stmt(file, source, body, &mut State::new(), &mut events);
    if events.is_empty() {
        return None;
    }
    let function = body.parent()?;
    let mut order = Vec::new();
    let mut points: HashMap<String, Vec<Point>> = HashMap::new();
    for event in events {
        if !points.contains_key(&event.variable) {
            order.push(event.variable.clone());
        }
        points
            .entry(event.variable.clone())
            .or_default()
            .push(Point {
                line: event.line,
                kind: event.kind,
            });
    }
    let rows = order
        .into_iter()
        .map(|variable| Lifeline {
            points: points.remove(&variable).unwrap_or_default(),
            variable,
        })
        .collect();
    Some(Lifetimes {
        first_line: function.start_position().row as u32 + 1,
        last_line: function.end_position().row as u32 + 1,
        rows,
    })
}

/// The pointer lifetimes in one function, for the invariant-matrix view.
pub struct Lifetimes {
    /// 1-based first and last line of the function.
    pub first_line: u32,
    pub last_line: u32,
    /// One row per pointer the function frees, in first-freed order.
    pub rows: Vec<Lifeline>,
}

/// One pointer's lifetime: its events, in line order.
pub struct Lifeline {
    pub variable: String,
    pub points: Vec<Point>,
}

/// A transition in a pointer's lifetime, to show in the matrix.
pub struct Point {
    pub line: u32,
    pub kind: PointKind,
}

/// Every pointer event across the project's functions.
fn all_events(graph: &CodeGraph) -> Vec<Event> {
    let Some(mut parser) = new_parser() else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for (file, source) in &graph.file_contents {
        // Cheap gate: every free-like name contains "free".
        if !source.contains("free") {
            continue;
        }
        if let Some(tree) = parser.parse(source, None) {
            walk_functions(file, source, tree.root_node(), &mut events);
        }
    }
    events
}

fn new_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .ok()?;
    Some(parser)
}

/// Walk each function definition in the tree with a fresh state.
fn walk_functions(file: &str, source: &str, root: Node, out: &mut Vec<Event>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "function_definition" {
            if let Some(body) = node.child_by_field_name("body") {
                // A body with a parse error (macros, `#ifdef` mid-statement)
                // has unreliable control flow, which turns `free; return` plus
                // a later use into false positives. Skip it rather than guess.
                if !body.has_error() {
                    walk_stmt(file, source, body, &mut State::new(), out);
                }
            }
            continue;
        }
        for child in named_children(node) {
            stack.push(child);
        }
    }
}

/// The innermost function definition's body whose lines contain `line`.
fn function_body_containing(root: Node, line: u32) -> Option<Node> {
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
    found?.child_by_field_name("body")
}

/// Carry the state through one statement, in source order.
fn walk_stmt(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    match node.kind() {
        "compound_statement" => {
            for stmt in named_children(node) {
                walk_stmt(file, source, stmt, state, out);
            }
        }
        "if_statement" => walk_if(file, source, node, state, out),
        "for_statement" | "while_statement" | "do_statement" => {
            walk_loop(file, source, node, state, out);
        }
        "switch_statement" => walk_switch(file, source, node, state, out),
        // A `case`/`default` or goto label holds statements; walk them so the
        // control flow inside (e.g. a returning `if`) is tracked, not flattened.
        "case_statement" | "labeled_statement" => {
            for child in named_children(node) {
                walk_stmt(file, source, child, state, out);
            }
        }
        // A preprocessor conditional compiles only one arm, so treat it like
        // an `if`/`else`. `#elif` is itself a conditional (condition +
        // alternative); `#else` just holds statements.
        "preproc_ifdef" | "preproc_if" | "preproc_elif" => {
            walk_preproc(file, source, node, state, out);
        }
        "preproc_else" => {
            for stmt in preproc_body(node) {
                walk_stmt(file, source, stmt, state, out);
            }
        }
        _ => visit_expr(file, source, node, state, out),
    }
}

/// Both arms of an `if`, merged after the join. A branch whose control
/// doesn't fall through (it `return`s, `goto`s, `break`s or `continue`s)
/// never reaches the code after the `if`, so its state is excluded from the
/// merge — this is what keeps `if (err) { free(p); return; } use(p);` from
/// being flagged, the most common cleanup pattern.
fn walk_if(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    if let Some(cond) = node.child_by_field_name("condition") {
        visit_expr(file, source, cond, state, out);
    }
    let mut arms = Vec::new();
    if let Some(cons) = node.child_by_field_name("consequence") {
        let mut branch = state.clone();
        walk_stmt(file, source, cons, &mut branch, out);
        if !terminates(cons) {
            arms.push(branch);
        }
    }
    match node.child_by_field_name("alternative") {
        Some(alt) => {
            // tree-sitter-c wraps the else body in an `else_clause`.
            let body = if alt.kind() == "else_clause" {
                alt.named_child(0).unwrap_or(alt)
            } else {
                alt
            };
            let mut branch = state.clone();
            walk_stmt(file, source, body, &mut branch, out);
            if !terminates(body) {
                arms.push(branch);
            }
        }
        // No else: the other path falls through with the entry state.
        None => arms.push(state.clone()),
    }
    if !arms.is_empty() {
        *state = merge_states(&arms);
    }
}

/// A `#ifdef` / `#ifndef` / `#if` conditional (and its `#elif` chain). Only
/// one arm is compiled, so walk it like an `if`/`else`: each arm from the
/// entry state, an arm that can't fall through excluded from the merge, and no
/// `#else` leaving a fall-through path with the entry state. Without this the
/// guarded statements fell through to `visit_expr`, which flattens their
/// control flow — turning two frees on mutually exclusive paths (e.g. the
/// `#ifdef _WIN32` arm of a `realpath` shim) into a false double-free.
fn walk_preproc(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    let mut arms = Vec::new();
    let body = preproc_body(node);
    let mut branch = state.clone();
    for stmt in &body {
        walk_stmt(file, source, *stmt, &mut branch, out);
    }
    if !body.last().copied().is_some_and(terminates) {
        arms.push(branch);
    }
    match node.child_by_field_name("alternative") {
        Some(alt) => {
            let mut branch = state.clone();
            walk_stmt(file, source, alt, &mut branch, out);
            if !terminates(alt) {
                arms.push(branch);
            }
        }
        // No `#else`: the other configuration compiles none of this, so that
        // path falls through with the entry state.
        None => arms.push(state.clone()),
    }
    if !arms.is_empty() {
        *state = merge_states(&arms);
    }
}

/// The guarded statements of a preproc conditional — its named children minus
/// the directive's macro `name`/`condition` and its `#else`/`#elif`
/// `alternative`.
fn preproc_body(node: Node) -> Vec<Node> {
    let name = node.child_by_field_name("name");
    let condition = node.child_by_field_name("condition");
    let alternative = node.child_by_field_name("alternative");
    named_children(node)
        .into_iter()
        .filter(|child| {
            Some(*child) != name && Some(*child) != condition && Some(*child) != alternative
        })
        .collect()
}

/// Whether control cannot fall out the bottom of `node` — it ends in a
/// `return`/`goto`/`break`/`continue`, or is an if/else whose arms both do.
fn terminates(node: Node) -> bool {
    match node.kind() {
        "return_statement" | "goto_statement" | "break_statement" | "continue_statement" => true,
        // The last *statement*, skipping a trailing comment (`return; /* ok */`).
        "compound_statement" => named_children(node)
            .iter()
            .rev()
            .find(|child| child.kind() != "comment")
            .copied()
            .is_some_and(terminates),
        "else_clause" => node.named_child(0).is_some_and(terminates),
        "if_statement" => matches!(
            (
                node.child_by_field_name("consequence"),
                node.child_by_field_name("alternative"),
            ),
            (Some(cons), Some(alt)) if terminates(cons) && terminates(alt)
        ),
        // A `#else` arm ends where its last statement does.
        "preproc_else" => preproc_body(node)
            .iter()
            .rev()
            .find(|child| child.kind() != "comment")
            .copied()
            .is_some_and(terminates),
        // A preproc conditional terminates only if its guarded arm and its
        // `#else`/`#elif` alternative both do (no fall-through configuration).
        "preproc_ifdef" | "preproc_if" | "preproc_elif" => {
            preproc_body(node).last().copied().is_some_and(terminates)
                && node
                    .child_by_field_name("alternative")
                    .is_some_and(terminates)
        }
        _ => false,
    }
}

/// A loop body may or may not run, so a free inside it is only "maybe". A
/// `for`'s initializer runs once before the loop (often reassigning the
/// pointer, e.g. `for (tmp = list; …)`); its update runs after the body.
fn walk_loop(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    if let Some(init) = node.child_by_field_name("initializer") {
        visit_expr(file, source, init, state, out);
    }
    if let Some(cond) = node.child_by_field_name("condition") {
        visit_expr(file, source, cond, state, out);
    }
    if let Some(body) = node.child_by_field_name("body") {
        let mut ran = state.clone();
        walk_stmt(file, source, body, &mut ran, out);
        if let Some(update) = node.child_by_field_name("update") {
            visit_expr(file, source, update, &mut ran, out);
        }
        *state = merge_states(&[state.clone(), ran]);
    }
}

/// A `switch`: walk the body, then weaken frees to "maybe" since which case
/// ran is conditional.
fn walk_switch(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    if let Some(body) = node.child_by_field_name("body") {
        let mut inner = state.clone();
        walk_stmt(file, source, body, &mut inner, out);
        for freed in inner.values_mut() {
            *freed = freed.demote();
        }
        *state = merge_states(&[state.clone(), inner]);
    }
}

/// Walk an expression for frees, reassignments and uses.
fn visit_expr(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    match node.kind() {
        "assignment_expression" => {
            if let Some(right) = node.child_by_field_name("right") {
                visit_expr(file, source, right, state, out);
            }
            if let Some(left) = node.child_by_field_name("left") {
                if left.kind() == "identifier" {
                    reassign(file, source, left, state, out);
                } else {
                    visit_expr(file, source, left, state, out);
                }
            }
        }
        "init_declarator" => {
            if let Some(value) = node.child_by_field_name("value") {
                visit_expr(file, source, value, state, out);
            }
            if let Some(name) = node
                .child_by_field_name("declarator")
                .and_then(declared_name)
            {
                state.remove(text(source, name));
            }
        }
        "call_expression" => visit_call(file, source, node, state, out),
        "pointer_expression" | "field_expression" | "subscript_expression" => {
            if let Some(base) = deref_base(node) {
                use_ident(file, source, base, state, out);
            }
            for child in named_children(node) {
                visit_expr(file, source, child, state, out);
            }
        }
        _ => {
            for child in named_children(node) {
                visit_expr(file, source, child, state, out);
            }
        }
    }
}

/// A call: `free(p)` frees `p`; any other call uses its simple arguments.
fn visit_call(file: &str, source: &str, node: Node, state: &mut State, out: &mut Vec<Event>) {
    let name = node
        .child_by_field_name("function")
        .map(|f| text(source, f));
    let args = node.child_by_field_name("arguments");
    if let (Some(name), Some(args)) = (name, args) {
        if names::is_free(name) {
            if let Some(arg) = single_identifier(args) {
                free_variable(file, source, node, arg, state, out);
                return;
            }
        }
        for arg in named_children(args) {
            if arg.kind() == "identifier" {
                use_ident(file, source, arg, state, out);
            } else {
                visit_expr(file, source, arg, state, out);
            }
        }
    }
}

/// Record a free of `ident`: a double-free if already freed, then freed.
fn free_variable(
    file: &str,
    source: &str,
    call: Node,
    ident: Node,
    state: &mut State,
    out: &mut Vec<Event>,
) {
    let name = text(source, ident);
    let line = line_of(call);
    if let Some(freed) = state.get(name).copied() {
        out.push(Event {
            kind: PointKind::FreedAgain,
            file: file.to_string(),
            line,
            columns: columns_of(call),
            variable: name.to_string(),
            certain: freed.is_certain(),
            freed_at: freed.line(),
        });
    } else {
        out.push(Event {
            kind: PointKind::Freed,
            file: file.to_string(),
            line,
            columns: columns_of(ident),
            variable: name.to_string(),
            certain: true,
            freed_at: line,
        });
    }
    state.insert(name.to_string(), Freed::Always(line));
}

/// Record a use of `ident` if it is freed (then stop tracking it, so one
/// free yields one finding).
fn use_ident(file: &str, source: &str, ident: Node, state: &mut State, out: &mut Vec<Event>) {
    let name = text(source, ident);
    if let Some(freed) = state.remove(name) {
        out.push(Event {
            kind: PointKind::Used,
            file: file.to_string(),
            line: line_of(ident),
            columns: columns_of(ident),
            variable: name.to_string(),
            certain: freed.is_certain(),
            freed_at: freed.line(),
        });
    }
}

/// Record that a freed variable was reassigned, so it is live again.
fn reassign(file: &str, source: &str, ident: Node, state: &mut State, out: &mut Vec<Event>) {
    let name = text(source, ident);
    if state.remove(name).is_some() {
        out.push(Event {
            kind: PointKind::Reassigned,
            file: file.to_string(),
            line: line_of(ident),
            columns: columns_of(ident),
            variable: name.to_string(),
            certain: false,
            freed_at: 0,
        });
    }
}

/// Merge several branch states into the state after their join.
fn merge_states(states: &[State]) -> State {
    let keys: HashSet<&String> = states.iter().flat_map(HashMap::keys).collect();
    let mut out = State::new();
    for key in keys {
        let merged = states
            .iter()
            .map(|s| s.get(key).copied())
            .reduce(merge)
            .flatten();
        if let Some(freed) = merged {
            out.insert(key.clone(), freed);
        }
    }
    out
}

/// The pointer a `*p` / `p->f` / `p[i]` dereferences, if a plain identifier.
fn deref_base(node: Node) -> Option<Node> {
    let base = node
        .child_by_field_name("argument")
        .or_else(|| node.named_child(0))?;
    (base.kind() == "identifier").then_some(base)
}

/// The identifier a declarator declares, e.g. `*p` or `p[8]` → `p`.
fn declared_name(declarator: Node) -> Option<Node> {
    let mut node = declarator;
    loop {
        if node.kind() == "identifier" {
            return Some(node);
        }
        node = node
            .child_by_field_name("declarator")
            .or_else(|| node.named_child(0))?;
    }
}

/// The one identifier argument of a call, if it has exactly one and it is a
/// plain identifier.
fn single_identifier(args: Node) -> Option<Node> {
    match named_children(args).as_slice() {
        [arg] if arg.kind() == "identifier" => Some(*arg),
        _ => None,
    }
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn text<'a>(source: &'a str, node: Node) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn line_of(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

/// The byte columns the node spans on its line, if it stays on one line.
fn columns_of(node: Node) -> Option<(u32, u32)> {
    let (start, end) = (node.start_position(), node.end_position());
    (start.row == end.row).then_some((start.column as u32, end.column as u32))
}

impl Event {
    fn into_finding(self, symbol: Option<SymbolId>) -> Finding {
        let on_a_path = if self.certain { "" } else { " on some path" };
        let (rule, cwe, message) = match self.kind {
            PointKind::FreedAgain => (
                DOUBLE_FREE,
                415,
                format!(
                    "`{}` is freed again{on_a_path}; it was already freed at line {}.",
                    self.variable, self.freed_at
                ),
            ),
            _ => (
                USE_AFTER_FREE,
                416,
                format!(
                    "`{}` is used{on_a_path} after it was freed at line {}.",
                    self.variable, self.freed_at
                ),
            ),
        };
        Finding {
            tool: "SrcWranglr".to_string(),
            rule: rule.to_string(),
            severity: if self.certain {
                Severity::High
            } else {
                Severity::Medium
            },
            message,
            fix: None,
            cwe: Some(cwe),
            file: self.file,
            line: self.line,
            columns: self.columns,
            symbol,
        }
    }
}
