//! Drafting a semgrep rule that finds code like a line you picked ("Find
//! More Like This"): the call on the line (or its statement), with its
//! variables turned into metavariables.
//!
//! - **Same shape** keeps the code as written but for its variables:
//!   `memcpy(buf, pkt->data, ntohs(hdr->len))` →
//!   `memcpy($A, $B->data, ntohs($C->len))`. Called names, fields, types,
//!   literals and capitalized names (macros, constants) stay.
//! - **Any values** keeps only the call and its number of arguments: each
//!   argument becomes a metavariable, a string literal `"..."`, and one
//!   with a call inside `<... ntohs(...) ...>`.
//!
//! A draft must keep something from the code (a called name, a field, a
//! type, a literal): `return $A;` would match every return. The line comes
//! from the code being audited, so everything taken from it is written
//! into the rule escaped, and literals that semgrep would read as pattern
//! syntax (`"$HOME"`, `"..."`, `"=~/re/"`) become `"..."`.

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

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

/// A rule drafted from one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDraft {
    /// The rule's id, also a fine file name: letters, digits and `-`.
    pub id: String,
    pub pattern: String,
    /// The whole rule, as semgrep reads it.
    pub yaml: String,
}

/// Pattern text longer than this, in bytes, is too much code for a rule.
const MAX_PATTERN_BYTES: usize = 2_000;
/// Code longer than this, in bytes, isn't turned into a pattern at all.
const MAX_CODE_BYTES: usize = 8 * MAX_PATTERN_BYTES;
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

/// A rule finding code like line `line` (1-based) of `source`, the text
/// of `file`.
#[cfg(test)]
pub fn draft(
    source: &str,
    file: &str,
    line: u32,
    looseness: Looseness,
) -> Result<RuleDraft, String> {
    let [same_shape, any_values] = drafts(source, file, line)?;
    Ok(match looseness {
        Looseness::SameShape => same_shape,
        Looseness::AnyValues => any_values,
    })
}

/// Both drafts for line `line` of `source`, in `Looseness` order, from
/// one parse.
pub fn drafts(source: &str, file: &str, line: u32) -> Result<[RuleDraft; 2], String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("This file is too large to draft a rule from.".into());
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .map_err(|e| e.to_string())?;
    let tree = parser
        .parse(source, None)
        .ok_or("The file couldn't be parsed.")?;
    let row = usize::try_from(line.checked_sub(1).ok_or("No line 0.")?)
        .map_err(|_| "No such line.".to_string())?;
    let node = code_on(tree.root_node(), row).ok_or("No call or statement starts on this line.")?;
    if node.byte_range().len() > MAX_CODE_BYTES {
        return Err("The code on this line is too long for a rule.".into());
    }
    let (shape, is_anchored) = same_shape(node, source);
    if !is_anchored {
        return Err(
            "Every name on this line is a variable, so a rule from it would match \
                    almost anything. Pick a line with a call, a field or a literal."
                .into(),
        );
    }
    let loose = match node.kind() {
        "call_expression" => any_values(node, source),
        _ => shape.clone(),
    };
    let id = rule_id(node, source, file, line);
    let quoted_code = cut_short(source.lines().nth(row).unwrap_or("").trim());
    let message = format!("Like {file}:{line}: {quoted_code}");
    let rule = |pattern: String| -> Result<RuleDraft, String> {
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err("The code on this line is too long for a rule.".into());
        }
        let yaml = rule_yaml(&id, &pattern, &message);
        Ok(RuleDraft {
            id: id.clone(),
            pattern,
            yaml,
        })
    };
    Ok([rule(shape)?, rule(loose)?])
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

/// `node`'s code with its variables as metavariables, its tokens joined
/// by single spaces where they had any space between them; and whether
/// anything but variables and punctuation was kept.
fn same_shape(node: Node, src: &str) -> (String, bool) {
    let mut names = Metavariables::default();
    let mut out = String::new();
    let mut is_anchored = false;
    let mut previous_end = None;
    for token in tokens(node) {
        if let Some(end) = previous_end {
            if src
                .get(end..token.start_byte())
                .is_some_and(|gap| !gap.is_empty())
            {
                out.push(' ');
            }
        }
        let text = src.get(token.byte_range()).unwrap_or("");
        if is_variable(token, text) {
            out.push_str(&names.of(text));
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

/// Whether the string literal `token` (written `text`) would mean more
/// than itself in a pattern: a metavariable inside, an ellipsis, a regex.
fn is_pattern_syntax(token: Node, text: &str) -> bool {
    let is_string = matches!(
        token.kind(),
        "string_literal" | "raw_string_literal" | "concatenated_string"
    );
    is_string && (text.contains('$') || text.contains("...") || text.contains("=~"))
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
        None => names.of(src.get(arg.byte_range()).unwrap_or("")),
    }
}

fn callee_text(call: Node, src: &str) -> String {
    call.child_by_field_name("function")
        .map_or(String::new(), |f| same_shape(f, src).0)
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
        let is_token = current.child_count() == 0 || LITERALS.contains(&current.kind());
        if is_token {
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

/// One metavariable per distinct name: `$A`, `$B`, … `$Z`, then `$V27`, ….
#[derive(Default)]
struct Metavariables {
    names: HashMap<String, String>,
}

impl Metavariables {
    fn of(&mut self, name: &str) -> String {
        let next = self.names.len();
        self.names
            .entry(name.to_string())
            .or_insert_with(|| match u8::try_from(next).ok().filter(|n| *n < 26) {
                Some(n) => format!("${}", char::from(b'A' + n)),
                None => format!("$V{}", next + 1),
            })
            .clone()
    }
}

/// `like-memcpy-12`: the call's name (or the file's) and the line.
fn rule_id(node: Node, src: &str, file: &str, line: u32) -> String {
    let callee = (node.kind() == "call_expression")
        .then(|| callee_text(node, src))
        .filter(|name| !name.is_empty());
    let stem = file
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .unwrap_or("code");
    let raw = format!("like-{}-{line}", callee.as_deref().unwrap_or(stem));
    let mut id = String::new();
    for c in raw.chars().take(MAX_ID_CHARS) {
        let c = c.to_ascii_lowercase();
        let keep = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(keep == '-' && id.ends_with('-')) {
            id.push(keep);
        }
    }
    id.trim_matches('-').to_string()
}

/// The rule, with everything from the code quoted.
fn rule_yaml(id: &str, pattern: &str, message: &str) -> String {
    format!(
        concat!(
            "rules:\n",
            "  - id: {id}\n",
            "    languages: [c, cpp]\n",
            "    severity: WARNING\n",
            "    message: {}\n",
            "    pattern: {}\n",
        ),
        quoted(message),
        quoted(pattern),
        id = id,
    )
}

/// `text` as a YAML double-quoted string: anything but printable ASCII
/// escaped, so nothing in it can end the string or hide in it.
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            c if u32::from(c) <= 0xFFFF => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push_str(&format!("\\U{:08X}", u32::from(c))),
        }
    }
    out.push('"');
    out
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
#[path = "similar_tests.rs"]
mod tests;
