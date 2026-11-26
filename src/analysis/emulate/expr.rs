//! Evaluating C expressions to `Value`s against the machine's state: integer
//! arithmetic, pointer offsets, `sizeof`, `?:`, allocations, and NULL. Anything
//! outside this (an unmodeled call, a field, an odd cast) evaluates to
//! `Unknown`, which the caller turns into a settable input where it matters.

use tree_sitter::Node;

use super::machine::Machine;
use super::names;
use super::value::Value;
use crate::analysis::parse::{named_children, text};

impl Machine {
    /// Evaluates `node` in the current state.
    pub fn eval(&mut self, node: Node, source: &str) -> Value {
        match node.kind() {
            "number_literal" => parse_int(text(source, node)).map_or(Value::Unknown, Value::Int),
            "identifier" => self.eval_identifier(text(source, node)),
            "parenthesized_expression" | "cast_expression" | "condition_clause" => node
                .child_by_field_name("value")
                .or_else(|| last_named(node))
                .map_or(Value::Unknown, |n| self.eval(n, source)),
            "unary_expression" => self.eval_unary(node, source),
            "binary_expression" => self.eval_binary(node, source),
            "conditional_expression" => self.eval_conditional(node, source),
            "call_expression" => self.eval_call(node, source),
            "sizeof_expression" => Value::Int(sizeof(node, source)),
            "pointer_expression" => Value::Unknown,
            _ => Value::Unknown,
        }
    }

    fn eval_identifier(&mut self, name: &str) -> Value {
        match name {
            "NULL" => Value::Null,
            _ => self.get(name).unwrap_or(Value::Unknown),
        }
    }

    fn eval_unary(&mut self, node: Node, source: &str) -> Value {
        let Some(arg) = node.child_by_field_name("argument") else {
            return Value::Unknown;
        };
        let op = operator(node, source);
        match (op, self.eval(arg, source)) {
            ("-", Value::Int(n)) => Value::Int(-n),
            ("!", v) => v
                .as_bool()
                .map_or(Value::Unknown, |b| Value::Int(i128::from(!b))),
            ("~", Value::Int(n)) => Value::Int(!n),
            _ => Value::Unknown,
        }
    }

    fn eval_binary(&mut self, node: Node, source: &str) -> Value {
        let (Some(l), Some(r)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return Value::Unknown;
        };
        let (left, right) = (self.eval(l, source), self.eval(r, source));
        let op = operator(node, source);
        if let Some(result) = pointer_math(op, left, right) {
            return result;
        }
        let (Some(a), Some(b)) = (left.as_int(), right.as_int()) else {
            return Value::Unknown;
        };
        integer_math(op, a, b)
    }

    fn eval_conditional(&mut self, node: Node, source: &str) -> Value {
        let cond = node
            .child_by_field_name("condition")
            .map(|c| self.eval(c, source).as_bool());
        match cond {
            Some(Some(true)) => node
                .child_by_field_name("consequence")
                .map_or(Value::Unknown, |n| self.eval(n, source)),
            Some(Some(false)) => node
                .child_by_field_name("alternative")
                .map_or(Value::Unknown, |n| self.eval(n, source)),
            _ => Value::Unknown,
        }
    }

    fn eval_call(&mut self, node: Node, source: &str) -> Value {
        let name = node
            .child_by_field_name("function")
            .map(|f| text(source, f))
            .unwrap_or_default()
            .to_string();
        let args = call_args(node);
        match names::alloc_size(&name) {
            Some(kind) => {
                let size = self.alloc_bytes(&kind, &args, source);
                self.alloc(size, format!("{name} result"))
            }
            None => Value::Unknown,
        }
    }

    /// The byte size an allocator call makes, if it can be computed.
    fn alloc_bytes(
        &mut self,
        kind: &names::AllocKind,
        args: &[Node],
        source: &str,
    ) -> Option<i128> {
        match kind {
            names::AllocKind::Arg(i) => args.get(*i).and_then(|a| self.eval(*a, source).as_int()),
            names::AllocKind::Product(i, j) => {
                let a = args.get(*i).and_then(|n| self.eval(*n, source).as_int())?;
                let b = args.get(*j).and_then(|n| self.eval(*n, source).as_int())?;
                a.checked_mul(b)
            }
            names::AllocKind::UnknownSize => None,
        }
    }
}

/// The identifier a declarator declares: `*p`, `p[8]`, `**pp` → `p`.
pub fn declared_name(node: Node) -> Option<Node> {
    let mut node = node.child_by_field_name("declarator").unwrap_or(node);
    loop {
        if node.kind() == "identifier" {
            return Some(node);
        }
        node = node
            .child_by_field_name("declarator")
            .or_else(|| node.named_child(0))?;
    }
}

/// The argument expressions of a call.
pub fn call_args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default()
}

/// The pointer a `*p` or `p[i]` writes through.
pub fn deref_base(node: Node) -> Option<Node> {
    match node.kind() {
        "pointer_expression" => node.child_by_field_name("argument"),
        "subscript_expression" => node
            .child_by_field_name("argument")
            .or_else(|| node.named_child(0)),
        _ => None,
    }
}

fn pointer_math(op: &str, left: Value, right: Value) -> Option<Value> {
    // Pointer ± integer (byte offsets: element size is treated as 1).
    match (op, left, right) {
        ("+", Value::Ptr { buffer, offset }, Value::Int(n))
        | ("+", Value::Int(n), Value::Ptr { buffer, offset })
        | ("-", Value::Ptr { buffer, offset }, Value::Int(n)) => {
            let delta = if op == "-" { -n } else { n };
            Some(Value::Ptr {
                buffer,
                offset: offset + delta,
            })
        }
        // Comparisons with NULL (or a null-ish 0) resolve to a boolean.
        ("==" | "!=", a, b) if is_null_cmp(a, b) => {
            let equal = matches!((a, b), (Value::Null, Value::Null));
            Some(Value::Int(i128::from(equal == (op == "=="))))
        }
        _ => None,
    }
}

/// A comparison where at least one side is a pointer/null and the other is a
/// pointer, null, or literal 0.
fn is_null_cmp(a: Value, b: Value) -> bool {
    let nullish = |v: Value| matches!(v, Value::Null | Value::Ptr { .. } | Value::Int(0));
    (matches!(a, Value::Ptr { .. } | Value::Null) || matches!(b, Value::Ptr { .. } | Value::Null))
        && nullish(a)
        && nullish(b)
}

fn integer_math(op: &str, a: i128, b: i128) -> Value {
    let value = match op {
        "+" => a.checked_add(b),
        "-" => a.checked_sub(b),
        "*" => a.checked_mul(b),
        "/" => (b != 0).then(|| a / b),
        "%" => (b != 0).then(|| a % b),
        "<<" => u32::try_from(b).ok().and_then(|s| a.checked_shl(s)),
        ">>" => u32::try_from(b).ok().map(|s| a >> s),
        "&" => Some(a & b),
        "|" => Some(a | b),
        "^" => Some(a ^ b),
        "<" => Some(i128::from(a < b)),
        "<=" => Some(i128::from(a <= b)),
        ">" => Some(i128::from(a > b)),
        ">=" => Some(i128::from(a >= b)),
        "==" => Some(i128::from(a == b)),
        "!=" => Some(i128::from(a != b)),
        "&&" => Some(i128::from(a != 0 && b != 0)),
        "||" => Some(i128::from(a != 0 || b != 0)),
        _ => None,
    };
    value.map_or(Value::Unknown, Value::Int)
}

/// A node's operator token text (e.g. `+`, `==`).
fn operator<'s>(node: Node, source: &'s str) -> &'s str {
    node.child_by_field_name("operator")
        .map(|o| text(source, o))
        .unwrap_or_default()
}

fn last_named(node: Node) -> Option<Node> {
    named_children(node).into_iter().next_back()
}

/// A modeled `sizeof`: the type's size in bytes, else pointer size.
fn sizeof(node: Node, source: &str) -> i128 {
    let inner = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("value"))
        .map(|t| text(source, t))
        .unwrap_or_default();
    type_size(inner)
}

fn type_size(text: &str) -> i128 {
    if text.contains('*') {
        return 8;
    }
    for (name, size) in [
        ("char", 1),
        ("short", 2),
        ("int", 4),
        ("long long", 8),
        ("long", 8),
        ("double", 8),
        ("float", 4),
        ("size_t", 8),
    ] {
        if text.contains(name) {
            return size;
        }
    }
    8
}

/// Parses a C integer literal (decimal, `0x…` hex, with `u`/`l` suffixes).
fn parse_int(text: &str) -> Option<i128> {
    let trimmed = text.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return i128::from_str_radix(hex, 16).ok();
    }
    trimmed.parse().ok()
}
