//! What an expression's value is made from: the variables, calls and
//! constants in it, without the parts that don't carry data (an index, a
//! condition, a cast's type).

use tree_sitter::Node;

use crate::index::syntax::{named_children, text};

/// An expression past this many nodes is only partly read.
const MAX_NODES: usize = 256;
/// Unwrapping `(char *)&buf[2]` and the like goes no deeper than this.
const MAX_UNWRAP: usize = 16;

/// One thing an expression's value comes from.
#[derive(Debug, Clone, Copy)]
pub(super) enum Flow<'t> {
    /// A variable, or a field written as a path (`conn->buf`).
    Var(Node<'t>),
    /// What a call returns.
    Call(Node<'t>),
    /// A literal, `sizeof`, `NULL`, an upper-case macro.
    Constant,
    /// Something not followed, and what it is.
    Opaque(&'static str),
}

/// The flows in `expr`, leftmost first.
pub(super) fn flows<'t>(expr: Node<'t>, source: &str) -> Vec<Flow<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![expr];
    let mut seen = 0usize;
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > MAX_NODES {
            break;
        }
        let field = |name: &str| node.child_by_field_name(name);
        match node.kind() {
            "identifier" if is_constant_name(text(node, source)) => out.push(Flow::Constant),
            "identifier" | "field_expression" | "qualified_identifier" => out.push(Flow::Var(node)),
            "call_expression" => out.push(Flow::Call(node)),
            "number_literal"
            | "string_literal"
            | "char_literal"
            | "concatenated_string"
            | "raw_string_literal"
            | "user_defined_literal"
            | "true"
            | "false"
            | "null"
            | "nullptr"
            | "sizeof_expression"
            | "alignof_expression" => out.push(Flow::Constant),
            "this" => out.push(Flow::Opaque("`this` (not followed)")),
            "lambda_expression" => out.push(Flow::Opaque("a lambda (not followed)")),
            "subscript_expression"
            | "pointer_expression"
            | "unary_expression"
            | "update_expression" => stack.extend(field("argument")),
            "cast_expression" => stack.extend(field("value")),
            "assignment_expression" => stack.extend(field("right")),
            "binary_expression" => stack.extend(field("right").into_iter().chain(field("left"))),
            "conditional_expression" => {
                stack.extend(field("alternative").into_iter().chain(field("consequence")))
            }
            "comment" | "type_descriptor" | "primitive_type" | "type_identifier" => {}
            _ => stack.extend(named_children(node).into_iter().rev()),
        }
    }
    out
}

/// Whether any of `expr`'s value comes from something other than constants.
pub(super) fn carries_data(expr: Node, source: &str) -> bool {
    flows(expr, source)
        .iter()
        .any(|f| !matches!(f, Flow::Constant))
}

/// How a variable is written, whitespace dropped: `conn -> buf` is
/// `conn->buf`.
pub(super) fn name_of(node: Node, source: &str) -> String {
    text(node, source).split_whitespace().collect()
}

/// Whether `node` is written as `name` (whitespace aside), without
/// copying its text: most nodes are rejected by their length.
fn is_named(node: Node, source: &str, name: &str) -> bool {
    let written = text(node, source);
    written.len() >= name.len()
        && written
            .split_whitespace()
            .flat_map(str::chars)
            .eq(name.chars())
}

/// Whether `arg` is `name` (`Some(true)`: it, its address, or it cast) or a
/// part of it (`Some(false)`: `name[i]`, `*name`, `name->f`, `name + 4`).
pub(super) fn refers_to(arg: Node, source: &str, name: &str) -> Option<bool> {
    let mut node = arg;
    let mut is_whole = true;
    for _ in 0..MAX_UNWRAP {
        if is_named(node, source, name) {
            return Some(is_whole);
        }
        let operator = node
            .child_by_field_name("operator")
            .map_or("", |o| text(o, source));
        node = match node.kind() {
            "cast_expression" => node.child_by_field_name("value")?,
            "parenthesized_expression" => node.named_child(0)?,
            // `p++` is `p`, moved on after.
            "update_expression" => node.child_by_field_name("argument")?,
            "pointer_expression" => {
                is_whole &= operator == "&";
                node.child_by_field_name("argument")?
            }
            "subscript_expression" | "field_expression" => {
                is_whole = false;
                node.child_by_field_name("argument")?
            }
            "binary_expression" if matches!(operator, "+" | "-") => {
                is_whole = false;
                node.child_by_field_name("left")?
            }
            _ => return None,
        };
    }
    None
}

/// The variable an access is to, and the fields it goes through:
/// `(char *)&r->hdr.name[2]` is `r`, through `hdr` then `name`.
pub(super) fn access_path<'t>(node: Node<'t>, source: &str) -> (Node<'t>, Vec<String>) {
    let mut fields = Vec::new();
    let mut current = node;
    for _ in 0..MAX_UNWRAP * 4 {
        let field = |name: &str| current.child_by_field_name(name);
        let next = match current.kind() {
            "field_expression" => {
                fields.extend(field("field").map(|f| text(f, source).to_string()));
                field("argument")
            }
            "cast_expression" => field("value"),
            "parenthesized_expression" => current.named_child(0),
            "pointer_expression" | "subscript_expression" | "update_expression" => {
                field("argument")
            }
            "binary_expression" => field("left"),
            _ => None,
        };
        match next {
            Some(next) => current = next,
            None => break,
        }
    }
    fields.reverse();
    (current, fields)
}

/// The fields in a written field path: `q.hdr->name` → `hdr`, `name`.
pub(super) fn fields_of(name: &str) -> Vec<String> {
    name.replace("->", ".")
        .split('.')
        .skip(1)
        .map(String::from)
        .collect()
}

/// The variable a field path is in: `conn->buf.data` → `conn`.
pub(super) fn field_base(name: &str) -> Option<&str> {
    let end = name.find(['.', '-'])?;
    let base = &name[..end];
    let is_name = base
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && base.chars().all(|c| c.is_alphanumeric() || c == '_');
    is_name.then_some(base)
}

/// `NULL`, `true`, `stdin`, and upper-case names, which are macros.
fn is_constant_name(name: &str) -> bool {
    let is_macro = name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    is_macro
        || matches!(
            name,
            "true" | "false" | "nullptr" | "stdin" | "stdout" | "stderr"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::{Parser, Tree};

    fn parse(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("grammar");
        parser.parse(source, None).expect("tree")
    }

    /// The expression of the statement `expr;` inside a function body.
    fn expression(tree: &Tree) -> Node<'_> {
        let mut node = tree.root_node();
        while node.kind() != "expression_statement" {
            let last = u32::try_from(node.named_child_count() - 1).expect("few children");
            node = node.named_child(last).expect("child");
        }
        node.named_child(0).expect("expression")
    }

    fn described(source: &str) -> Vec<String> {
        let code = format!("void f() {{ {source}; }}");
        let tree = parse(&code);
        flows(expression(&tree), &code)
            .iter()
            .map(|f| match f {
                Flow::Var(n) => name_of(*n, &code),
                Flow::Call(n) => format!("call {}", name_of(*n, &code)),
                Flow::Constant => "constant".into(),
                Flow::Opaque(why) => (*why).into(),
            })
            .collect()
    }

    #[test]
    fn reads_the_values_out_of_an_expression() {
        assert_eq!(described("(char *)conn->buf + n"), ["conn->buf", "n"]);
        assert_eq!(described("argv[i + 1]"), ["argv"]);
        assert_eq!(described("c ? a : \"x\""), ["a", "constant"]);
        assert_eq!(described("sizeof buf + MAX_LEN"), ["constant", "constant"]);
        assert_eq!(described("trim(raw)"), ["call trim(raw)"]);
    }

    #[test]
    fn knows_when_an_argument_is_a_variable_or_part_of_it() {
        let code = "void f() { g(&buf, buf + 4, r->name, (char *)p, other); }";
        let tree = parse(code);
        let call = expression(&tree);
        let args = named_children(call.child_by_field_name("arguments").expect("args"));
        let check = |i: usize, name: &str| refers_to(args[i], code, name);
        assert_eq!(check(0, "buf"), Some(true));
        assert_eq!(check(1, "buf"), Some(false));
        assert_eq!(check(2, "r"), Some(false));
        assert_eq!(check(2, "r->name"), Some(true));
        assert_eq!(check(3, "p"), Some(true));
        assert_eq!(check(4, "buf"), None);
    }

    #[test]
    fn reads_an_access_path() {
        let code = "void f() { g((char *)&r->hdr.name[2]); }";
        let tree = parse(code);
        let call = expression(&tree);
        let args = named_children(call.child_by_field_name("arguments").expect("args"));
        let (base, fields) = access_path(args[0], code);
        assert_eq!(text(base, code), "r");
        assert_eq!(fields, ["hdr", "name"]);
        assert_eq!(fields_of("q.hdr->name"), ["hdr", "name"]);
        assert!(fields_of("q").is_empty());
    }

    #[test]
    fn finds_the_variable_a_field_is_in() {
        assert_eq!(field_base("conn->buf.data"), Some("conn"));
        assert_eq!(field_base("q.name"), Some("q"));
        assert_eq!(field_base("name"), None);
    }
}
