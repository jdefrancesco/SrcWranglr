//! Drafting a semgrep rule that finds code like code you picked ("Find
//! More Like This"): a line, several lines, or several places that share
//! a pattern, with their variables turned into metavariables.
//!
//! - **Same shape** keeps the code as written but for its variables:
//!   `memcpy(buf, pkt->data, ntohs(hdr->len))` →
//!   `memcpy($A, $B->data, ntohs($C->len))`. Called names, fields, types,
//!   literals and capitalized names (macros, constants) stay.
//! - **Any values** (a single call) keeps only the call and its number of
//!   arguments: each argument becomes a metavariable, a string literal
//!   `"..."`, and one with a call inside `<... ntohs(...) ...>`.
//! - **Several lines** become a sequence of statements, with `...` between
//!   them when other code may come between. One variable is one
//!   metavariable throughout, so `p = malloc(n); ... memcpy(p, s, n);`
//!   follows the same `p`.
//! - **Several places** become what they share (see `unify`).
//!
//! A draft must keep something from the code (a called name, a field, a
//! type, a literal): `return $A;` would match every return. The code is
//! the code being audited, so everything taken from it is written into
//! the rule escaped, and literals that semgrep would read as pattern
//! syntax (`"$HOME"`, `"..."`, `"=~/re/"`) become `"..."`.

mod exception;
mod pick;
mod unify;

use std::collections::HashMap;

use tree_sitter::{Node, Parser, Tree};

pub use exception::exception_from;
pub use unify::MAX_EXAMPLES;

use super::semgrep_rule::{rule_id as id_from, rules_yaml, Clause, Exception, RuleSpec};

/// How far a draft strays from the code it was made from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Looseness {
    #[default]
    SameShape,
    AnyValues,
}

impl Looseness {
    pub const ALL: [Looseness; 2] = [Looseness::SameShape, Looseness::AnyValues];

    pub fn label(self) -> &'static str {
        match self {
            Looseness::SameShape => "Same shape",
            Looseness::AnyValues => "Any values",
        }
    }
}

/// Code picked to draft a rule from: lines `first_line..=last_line`
/// (1-based) of `file`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Example {
    pub file: String,
    pub first_line: u32,
    pub last_line: u32,
}

impl Example {
    #[cfg(test)]
    pub fn line(file: &str, line: u32) -> Self {
        Self::lines(file, line, line)
    }

    pub fn lines(file: &str, first_line: u32, last_line: u32) -> Self {
        Self {
            file: file.to_string(),
            first_line: first_line.min(last_line),
            last_line: first_line.max(last_line),
        }
    }

    /// `file:12` or `file:12-14`.
    pub fn place(&self) -> String {
        match self.first_line == self.last_line {
            true => format!("{}:{}", self.file, self.first_line),
            false => format!("{}:{}-{}", self.file, self.first_line, self.last_line),
        }
    }
}

/// A rule drafted from code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Also a fine file name: letters, digits and `-`.
    pub id: String,
    pub message: String,
    pub same_shape: String,
    /// Only the call and its arguments' kinds, for a draft of one call.
    pub any_values: Option<String>,
    /// The variables (as written in the first place picked) and their
    /// metavariables, for exceptions to use the same ones.
    pub names: Names,
    /// The name of the first call, as written: an exception calling it is
    /// the fixed form of the same call.
    pub callee: Option<String>,
}

impl Draft {
    pub fn pattern(&self, looseness: Looseness) -> &str {
        match looseness {
            Looseness::SameShape => &self.same_shape,
            Looseness::AnyValues => self.any_values.as_deref().unwrap_or(&self.same_shape),
        }
    }

    /// The whole rule, as semgrep reads it.
    pub fn yaml(&self, looseness: Looseness, exceptions: &[Exception]) -> String {
        let clauses = std::iter::once(Clause::Pattern(self.pattern(looseness).to_string()))
            .chain(exceptions.iter().filter_map(Exception::clause))
            .collect();
        rules_yaml(&[RuleSpec {
            id: self.id.clone(),
            message: self.message.clone(),
            severity: Default::default(),
            clauses,
        }])
    }
}

/// A draft's variables and their metavariables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    by_name: Vec<(String, String)>,
    /// Metavariables handed out, so new ones don't reuse a letter.
    count: usize,
}

/// Pattern text longer than this, in bytes, is too much code for a rule.
const MAX_PATTERN_BYTES: usize = 2_000;
/// Code longer than this, in bytes, isn't turned into a pattern at all.
const MAX_CODE_BYTES: usize = 8 * MAX_PATTERN_BYTES;
/// The same for several lines or places: a sequence of statements or a
/// whole function.
const MAX_LONG_PATTERN_BYTES: usize = 4 * MAX_PATTERN_BYTES;
const MAX_LONG_CODE_BYTES: usize = 4 * MAX_CODE_BYTES;
/// Files larger than this aren't parsed (the indexer keeps smaller ones).
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// How far up a name is followed to see whether it is called.
const MAX_NAME_DEPTH: usize = 16;
/// Rule ids are cut to this many characters.
const MAX_ID_CHARS: usize = 60;
/// The code quoted in the rule's message is cut to this many characters.
const MAX_QUOTED_CHARS: usize = 120;
/// Nodes looked at to find the code on the line, so a huge file can't
/// stall the UI thread.
const MAX_NODES_VISITED: usize = 200_000;
/// Literal kinds kept whole, as written.
const LITERALS: [&str; 5] = [
    "string_literal",
    "raw_string_literal",
    "char_literal",
    "concatenated_string",
    "number_literal",
];
const VARIABLES_ONLY: &str = "Every name in this code is a variable, so a rule from it would \
                              match almost anything. Pick code with a call, a field or a literal.";

/// A rule finding code like `examples`, each with the text of its file.
/// With several lines, `allow_gaps` lets other code come between their
/// statements.
pub fn draft_from(examples: &[(Example, &str)], allow_gaps: bool) -> Result<Draft, String> {
    match examples {
        [] => Err("Pick some code first.".into()),
        [(example, source)] if example.first_line == example.last_line => {
            one_line(source, &example.file, example.first_line)
        }
        [(example, source)] => lines(source, example, allow_gaps),
        several => unify::shared(several, allow_gaps),
    }
}

/// A rule from the call (or statement) on line `line` of `source`.
fn one_line(source: &str, file: &str, line: u32) -> Result<Draft, String> {
    let tree = parse(source, file)?;
    let row = row_of(line)?;
    let node = code_on(tree.root_node(), row).ok_or("No call or statement starts on this line.")?;
    let too_long = "The code on this line is too long for a rule.";
    if node.byte_range().len() > MAX_CODE_BYTES {
        return Err(too_long.into());
    }
    let mut names = Metavariables::default();
    let (shape, is_anchored) = same_shape(node, source, &mut names);
    if !is_anchored {
        return Err(VARIABLES_ONLY.into());
    }
    let any_values = (node.kind() == "call_expression").then(|| any_values(node, source));
    let is_too_long = |pattern: &str| pattern.len() > MAX_PATTERN_BYTES;
    if is_too_long(&shape) || any_values.as_deref().is_some_and(is_too_long) {
        return Err(too_long.into());
    }
    let callee = first_callee(node, source);
    let quoted_code = cut_short(source.lines().nth(row).unwrap_or("").trim());
    Ok(Draft {
        id: rule_id(callee.as_deref(), file, line),
        message: format!("Like {file}:{line}: {quoted_code}"),
        same_shape: shape,
        any_values,
        names: names.names(),
        callee,
    })
}

/// A rule from the statements on lines `example` of `source`.
fn lines(source: &str, example: &Example, allow_gaps: bool) -> Result<Draft, String> {
    let tree = parse(source, &example.file)?;
    let (first, last) = (row_of(example.first_line)?, row_of(example.last_line)?);
    let nodes = match pick::picked(tree.root_node(), first, last) {
        Some(pick::Picked::One(node)) => vec![node],
        Some(pick::Picked::Sequence(nodes)) => nodes,
        None => return Err("No code starts on these lines.".into()),
    };
    let too_long = "The code on these lines is too long for a rule: pick fewer.";
    let code_bytes: usize = nodes.iter().map(|n| n.byte_range().len()).sum();
    if code_bytes > MAX_LONG_CODE_BYTES || nodes.len() > pick::MAX_STATEMENTS {
        return Err(too_long.into());
    }
    let mut names = Metavariables::default();
    let mut is_anchored = false;
    let parts: Vec<String> = nodes
        .iter()
        .map(|node| {
            let (shape, anchored) = same_shape(*node, source, &mut names);
            is_anchored |= anchored;
            shape
        })
        .collect();
    if !is_anchored {
        return Err(VARIABLES_ONLY.into());
    }
    let gaps = vec![allow_gaps; parts.len().saturating_sub(1)];
    let shape = sequence(&parts, &gaps);
    let any_values = match nodes.as_slice() {
        [call] if call.kind() == "call_expression" => Some(any_values(*call, source)),
        _ => None,
    };
    let is_too_long = |pattern: &str| pattern.len() > MAX_LONG_PATTERN_BYTES;
    if is_too_long(&shape) || any_values.as_deref().is_some_and(is_too_long) {
        return Err(too_long.into());
    }
    let callee = nodes.iter().find_map(|node| first_callee(*node, source));
    let quoted_code = cut_short(source.lines().nth(first).unwrap_or("").trim());
    Ok(Draft {
        id: rule_id(callee.as_deref(), &example.file, example.first_line),
        message: format!("Like {}: {quoted_code}", example.place()),
        same_shape: shape,
        any_values,
        names: names.names(),
        callee,
    })
}

/// `parts` one per line, with `...` after each part whose gap is set.
fn sequence(parts: &[String], gaps: &[bool]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            if gaps.get(i - 1).copied().unwrap_or(false) {
                out.push_str("...\n");
            }
        }
        out.push_str(part);
    }
    out
}

fn parse(source: &str, file: &str) -> Result<Tree, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!("{file} is too large to draft a rule from."));
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .map_err(|e| e.to_string())?;
    parser
        .parse(source, None)
        .ok_or_else(|| format!("{file} couldn't be parsed."))
}

/// The 0-based row of 1-based `line`.
fn row_of(line: u32) -> Result<usize, String> {
    usize::try_from(line.checked_sub(1).ok_or("No line 0.")?).map_err(|_| "No such line.".into())
}

/// The code to match on row `row`: the first call starting there, else
/// the first statement or declaration (an expression statement's
/// expression); a call that only goes on there if nothing starts there.
fn code_on(root: Node, row: usize) -> Option<Node> {
    let mut first_statement = None;
    let mut holding_call = None;
    let mut cursor = root.walk();
    let mut visited = 0;
    loop {
        let node = cursor.node();
        visited += 1;
        let (start, end) = (node.start_position().row, node.end_position().row);
        let holds_row = start <= row && row <= end;
        if holds_row {
            if node.kind() == "call_expression" {
                if start == row {
                    return Some(node);
                }
                holding_call = Some(node);
            }
            if start == row && first_statement.is_none() {
                first_statement = statement_code(node);
            }
        }
        if holds_row && visited < MAX_NODES_VISITED && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return first_statement.or(holding_call);
            }
        }
    }
}

/// The code a statement or declaration is matched by, if `node` is one.
fn statement_code(node: Node) -> Option<Node> {
    match node.kind() {
        // `x = y;` as `x = y`, so it matches inside other code too.
        "expression_statement" => node.named_child(0),
        "declaration" | "return_statement" | "field_declaration" => Some(node),
        _ => None,
    }
}

/// `node`'s code with its variables as metavariables (from `names`), its
/// tokens joined by single spaces where they had any space between them;
/// and whether anything but variables and punctuation was kept.
fn same_shape(node: Node, src: &str, names: &mut Metavariables) -> (String, bool) {
    let mut out = String::new();
    let mut is_anchored = false;
    let mut previous_end = None;
    for token in tokens(node) {
        if previous_end.is_some_and(|end| has_gap(src, end, token.start_byte())) {
            out.push(' ');
        }
        let text = src.get(token.byte_range()).unwrap_or("");
        if is_variable(token, text) {
            out.push_str(&names.of(&[text]));
        } else if is_pattern_syntax(token, text) {
            out.push_str("\"...\"");
            is_anchored = true;
        } else {
            out.push_str(text);
            is_anchored |= token.is_named();
        }
        previous_end = Some(token.end_byte());
    }
    (out, is_anchored)
}

/// Whether there is any text (space, a comment) between two tokens.
fn has_gap(src: &str, end: usize, start: usize) -> bool {
    src.get(end..start).is_some_and(|gap| !gap.is_empty())
}

/// Whether the string literal `token` (written `text`) would mean more
/// than itself in a pattern: a metavariable inside, an ellipsis, a regex.
fn is_pattern_syntax(token: Node, text: &str) -> bool {
    is_string(token.kind()) && (text.contains('$') || text.contains("...") || text.contains("=~"))
}

fn is_string(kind: &str) -> bool {
    matches!(
        kind,
        "string_literal" | "raw_string_literal" | "concatenated_string"
    )
}

/// The call at `node` with each argument loosened.
fn any_values(node: Node, src: &str) -> String {
    let callee = callee_text(node, src);
    let mut names = Metavariables::default();
    let arguments: Vec<String> = node
        .child_by_field_name("arguments")
        .map(|list| {
            let mut cursor = list.walk();
            list.named_children(&mut cursor)
                .filter(|arg| arg.kind() != "comment")
                .map(|arg| loose_argument(arg, src, &mut names))
                .collect()
        })
        .unwrap_or_default();
    format!("{callee}({})", arguments.join(", "))
}

fn loose_argument(arg: Node, src: &str, names: &mut Metavariables) -> String {
    if arg.kind() == "string_literal" || arg.kind() == "concatenated_string" {
        return "\"...\"".into();
    }
    match first_call_in(arg) {
        Some(call) if call == arg => format!("{}(...)", callee_text(call, src)),
        Some(call) => format!("<... {}(...) ...>", callee_text(call, src)),
        None => names.of(&[src.get(arg.byte_range()).unwrap_or("")]),
    }
}

fn callee_text(call: Node, src: &str) -> String {
    call.child_by_field_name("function")
        .map_or(String::new(), |f| {
            same_shape(f, src, &mut Metavariables::default()).0
        })
}

/// The name of the first call at or under `node`, as written.
fn first_callee(node: Node, src: &str) -> Option<String> {
    let function = first_call_in(node)?.child_by_field_name("function")?;
    let name = src.get(function.byte_range())?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The first call at or under `node`, outermost first.
fn first_call_in(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    loop {
        let current = cursor.node();
        if current.kind() == "call_expression" {
            return Some(current);
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.node() == node {
                return None;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return None;
            }
        }
    }
}

/// The tokens of `node` in order: its leaves, literals whole, comments
/// left out.
fn tokens(node: Node) -> Vec<Node> {
    let mut found = Vec::new();
    let mut cursor = node.walk();
    loop {
        let current = cursor.node();
        if is_token(current) {
            if current.kind() != "comment" {
                found.push(current);
            }
        } else if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.node() == node {
                return found;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return found;
            }
        }
    }
}

/// Whether `node` is matched as one token: a leaf, or a literal.
fn is_token(node: Node) -> bool {
    node.child_count() == 0 || LITERALS.contains(&node.kind())
}

/// Whether the identifier `token` (written `text`) names a variable: not
/// a called function, nor a capitalized macro or constant. A name with a
/// `$` in it (a compiler extension) is one too, so it can't pass for a
/// metavariable.
fn is_variable(token: Node, text: &str) -> bool {
    token.kind() == "identifier"
        && (text.contains('$') || (!is_called_name(token) && !is_capitalized(text)))
}

/// Whether `node` names the function a call calls (`f` in `f(x)` or
/// `ns::f(x)`).
fn is_called_name(node: Node) -> bool {
    let mut name = node;
    for _ in 0..MAX_NAME_DEPTH {
        let Some(parent) = name.parent() else {
            return false;
        };
        match parent.kind() {
            "call_expression" => return parent.child_by_field_name("function") == Some(name),
            "qualified_identifier" | "template_function" => name = parent,
            _ => return false,
        }
    }
    false
}

fn is_capitalized(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_uppercase())
        && text
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// One metavariable per distinct name (or, across several places, per
/// distinct set of names): `$A`, `$B`, … `$Z`, then `$V27`, ….
#[derive(Default)]
struct Metavariables {
    names: HashMap<Vec<String>, String>,
    /// The names in the order they were given metavariables.
    order: Vec<Vec<String>>,
    /// Metavariables handed out before these (an exception's start after
    /// its rule's).
    base: usize,
}

impl Metavariables {
    /// Starting from a draft's names, so code drafted next uses them too.
    fn seeded(names: &Names) -> Self {
        let mut seeded = Self {
            base: names.count,
            ..Self::default()
        };
        for (name, metavariable) in &names.by_name {
            seeded
                .names
                .entry(vec![name.clone()])
                .or_insert_with(|| metavariable.clone());
        }
        seeded
    }

    fn of(&mut self, texts: &[&str]) -> String {
        let key: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        if let Some(existing) = self.names.get(&key) {
            return existing.clone();
        }
        let next = self.base + self.order.len();
        let metavariable = match u8::try_from(next).ok().filter(|n| *n < 26) {
            Some(n) => format!("${}", char::from(b'A' + n)),
            None => format!("$V{}", next + 1),
        };
        self.names.insert(key.clone(), metavariable.clone());
        self.order.push(key);
        metavariable
    }

    /// How many were handed out, to go back to with `roll_back`.
    fn mark(&self) -> usize {
        self.order.len()
    }

    /// Forgets the metavariables handed out since `mark`.
    fn roll_back(&mut self, mark: usize) {
        for key in self.order.drain(mark..) {
            self.names.remove(&key);
        }
    }

    /// The names as written in the first place, for exceptions.
    fn names(&self) -> Names {
        let mut by_name: Vec<(String, String)> = Vec::new();
        for key in &self.order {
            let (Some(name), Some(metavariable)) = (key.first(), self.names.get(key)) else {
                continue;
            };
            if !by_name.iter().any(|(n, _)| n == name) {
                by_name.push((name.clone(), metavariable.clone()));
            }
        }
        Names {
            by_name,
            count: self.base + self.order.len(),
        }
    }
}

/// `like-memcpy-12`: the call's name (or the file's) and the line.
fn rule_id(callee: Option<&str>, file: &str, line: u32) -> String {
    let stem = file
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .unwrap_or("code");
    let name = callee.filter(|c| !c.is_empty()).unwrap_or(stem);
    id_from(&format!("like-{name}-{line}"), MAX_ID_CHARS)
}

fn cut_short(text: &str) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(MAX_QUOTED_CHARS).collect();
    match chars.next() {
        Some(_) => kept + "…",
        None => kept,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lines_tests.rs"]
mod lines_tests;
