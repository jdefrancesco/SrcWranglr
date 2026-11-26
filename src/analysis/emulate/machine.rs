//! The little machine: variables, a heap of buffers, the inputs the user set,
//! and the step-by-step execution of one function's body. It runs concretely
//! over the inputs — real branches are taken, writes are bounds-checked — and
//! records a step per statement for the UI.

use std::collections::HashMap;

use tree_sitter::Node;

use super::names;
use super::value::{Buffer, Value};
use super::{Overflow, Step};
use crate::analysis::parse::{line_of, named_children, text};

/// Caps loops and recursion so a runaway function can't hang the UI.
const STEP_BUDGET: usize = 10_000;

/// Whether execution falls through to the next statement or leaves the block.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Next,
    Stop,
}

pub struct Machine {
    /// Local variables and seeded parameters.
    vars: HashMap<String, Value>,
    /// The heap, indexed by `BufId`.
    buffers: Vec<Buffer>,
    /// Inputs set by the user (name → value); unset names default to 0.
    given: HashMap<String, i128>,
    /// Inputs discovered, in first-seen order (name, value used).
    pub inputs: Vec<(String, i128)>,
    /// One entry per statement executed.
    pub steps: Vec<Step>,
    budget: usize,
}

impl Machine {
    pub fn new(given: &HashMap<String, i128>) -> Self {
        Self {
            vars: HashMap::new(),
            buffers: Vec::new(),
            given: given.clone(),
            inputs: Vec::new(),
            steps: Vec::new(),
            budget: STEP_BUDGET,
        }
    }

    /// The value of input `name`, registering it (first-seen order) with the
    /// user's value or a default of 0.
    pub fn input(&mut self, name: &str) -> i128 {
        if let Some((_, v)) = self.inputs.iter().find(|(n, _)| n == name) {
            return *v;
        }
        let value = self.given.get(name).copied().unwrap_or(0);
        self.inputs.push((name.to_string(), value));
        value
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.vars.get(name).copied()
    }

    pub fn set(&mut self, name: &str, value: Value) {
        // A buffer bound straight to a variable takes that variable's name, so
        // overflow reports read naturally ("entry", not "malloc result").
        if let Value::Ptr { buffer, offset: 0 } = value {
            if let Some(b) = self.buffers.get_mut(buffer) {
                if b.label.ends_with(" result") {
                    b.label = name.to_string();
                }
            }
        }
        self.vars.insert(name.to_string(), value);
    }

    pub fn buffer(&self, id: usize) -> Option<&Buffer> {
        self.buffers.get(id)
    }

    /// Adds a buffer of `size` bytes (unknown when `None`) and returns a
    /// pointer to its start.
    pub fn alloc(&mut self, size: Option<i128>, label: String) -> Value {
        let id = self.buffers.len();
        self.buffers.push(Buffer { id, size, label });
        Value::Ptr {
            buffer: id,
            offset: 0,
        }
    }

    /// Seeds the function's parameters: integer ones as inputs, pointer ones as
    /// an external buffer of unknown size (the caller's), others as `Unknown`.
    pub fn setup_params(&mut self, function: Node, source: &str) {
        let Some(declarator) = function.child_by_field_name("declarator") else {
            return;
        };
        let Some(params) = find_parameter_list(declarator) else {
            return;
        };
        for param in named_children(params) {
            if param.kind() != "parameter_declaration" {
                continue;
            }
            let Some(name_node) = super::expr::declared_name(param) else {
                continue;
            };
            let name = text(source, name_node).to_string();
            let value = if is_pointer_param(param) {
                self.alloc(None, format!("{name} (parameter)"))
            } else if is_integer_type(param, source) {
                Value::Int(self.input(&name))
            } else {
                Value::Unknown
            };
            self.set(&name, value);
        }
    }

    /// Runs a block or single statement, returning whether control fell out.
    pub fn run_block(&mut self, node: Node, source: &str) -> Flow {
        for stmt in named_children(node) {
            if self.budget == 0 {
                return Flow::Stop;
            }
            if self.exec(stmt, source) == Flow::Stop {
                return Flow::Stop;
            }
        }
        Flow::Next
    }

    fn exec(&mut self, node: Node, source: &str) -> Flow {
        self.budget = self.budget.saturating_sub(1);
        match node.kind() {
            "compound_statement" => self.run_block(node, source),
            "declaration" => {
                self.exec_declaration(node, source);
                Flow::Next
            }
            "expression_statement" => {
                if let Some(expr) = node.named_child(0) {
                    self.exec_expr_stmt(expr, source);
                }
                Flow::Next
            }
            "if_statement" => self.exec_if(node, source),
            "while_statement" | "for_statement" | "do_statement" => self.exec_loop(node, source),
            "return_statement" => {
                self.record(line_of(node), "return".to_string(), None);
                Flow::Stop
            }
            "break_statement" | "continue_statement" | "goto_statement" => Flow::Stop,
            _ => Flow::Next,
        }
    }

    fn exec_declaration(&mut self, node: Node, source: &str) {
        let integer = decl_is_integer(node, source);
        for child in named_children(node) {
            if child.kind() != "init_declarator" {
                continue;
            }
            let Some(name_node) = super::expr::declared_name(child) else {
                continue;
            };
            let name = text(source, name_node).to_string();
            let value = match child.child_by_field_name("value") {
                Some(init) => self.eval_init(init, source, &name, integer),
                // An uninitialised integer is a fresh input; a pointer is null.
                None if integer => Value::Int(self.input(&name)),
                None => Value::Unknown,
            };
            self.set(&name, value);
            self.record(
                line_of(node),
                format!("{name} = {}", show(value, self)),
                None,
            );
        }
    }

    /// Evaluates a declaration's initialiser; an unknown integer initialiser
    /// (e.g. `strlen(name)`) makes the variable a settable input.
    fn eval_init(&mut self, init: Node, source: &str, name: &str, integer: bool) -> Value {
        match self.eval(init, source) {
            Value::Unknown if integer => Value::Int(self.input(name)),
            other => other,
        }
    }

    fn exec_expr_stmt(&mut self, expr: Node, source: &str) {
        match expr.kind() {
            "assignment_expression" => self.exec_assignment(expr, source),
            "call_expression" => {
                let line = line_of(expr);
                let (text, overflow) = self.exec_call(expr, source);
                self.record(line, text, overflow);
            }
            _ => {
                self.eval(expr, source);
            }
        }
    }

    fn exec_assignment(&mut self, node: Node, source: &str) {
        let value = node
            .child_by_field_name("right")
            .map_or(Value::Unknown, |r| self.eval(r, source));
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let line = line_of(node);
        if left.kind() == "identifier" {
            let name = text(source, left).to_string();
            self.set(&name, value);
            self.record(line, format!("{name} = {}", show(value, self)), None);
        } else {
            // A write through a pointer/array: `*p = …`, `p[i] = …`.
            let overflow = self.write_through(left, source, Some(1));
            self.record(line, format!("{} = …", text(source, left)), overflow);
        }
    }

    fn exec_if(&mut self, node: Node, source: &str) -> Flow {
        let taken = node
            .child_by_field_name("condition")
            .map(|c| self.eval(c, source).as_bool())
            // An undecidable condition: take the then-branch (explore the work).
            .is_none_or(|b| b.unwrap_or(true));
        let branch = if taken {
            node.child_by_field_name("consequence")
        } else {
            node.child_by_field_name("alternative")
                .map(|alt| alt.named_child(0).unwrap_or(alt))
        };
        match branch {
            Some(b) => self.exec(b, source),
            None => Flow::Next,
        }
    }

    fn exec_loop(&mut self, node: Node, source: &str) -> Flow {
        if let Some(init) = node.child_by_field_name("initializer") {
            self.exec_expr_stmt(init, source);
        }
        let Some(body) = node.child_by_field_name("body") else {
            return Flow::Next;
        };
        while self.budget > 0 {
            let run = node
                .child_by_field_name("condition")
                .map(|c| self.eval(c, source).as_bool())
                .is_some_and(|b| b.unwrap_or(false));
            if !run {
                break;
            }
            if self.exec(body, source) == Flow::Stop {
                break;
            }
            if let Some(update) = node.child_by_field_name("update") {
                self.exec_expr_stmt(update, source);
            }
            self.budget = self.budget.saturating_sub(1);
        }
        Flow::Next
    }

    /// A call statement: an allocation bound to nothing, a copy (bounds
    /// checked), or anything else. Returns its step text and any overflow.
    pub fn exec_call(&mut self, node: Node, source: &str) -> (String, Option<Overflow>) {
        let name = node
            .child_by_field_name("function")
            .map(|f| text(source, f))
            .unwrap_or_default();
        let args = super::expr::call_args(node);
        let Some(copy) = names::copy_bytes(name) else {
            return (format!("{name}(…)"), None);
        };
        let dest = args
            .get(copy.dest)
            .map_or(Value::Unknown, |a| self.eval(*a, source));
        let n = copy
            .size
            .and_then(|i| args.get(i))
            .and_then(|a| self.eval(*a, source).as_int());
        let overflow = self.check_write(dest, n);
        let bytes = n.map_or_else(
            || "an unknown number of bytes".to_string(),
            |k| format!("{k} bytes"),
        );
        (format!("{name}(…) writes {bytes}"), overflow)
    }

    /// A write of `n` bytes through the pointer `left` evaluates to.
    fn write_through(&mut self, left: Node, source: &str, n: Option<i128>) -> Option<Overflow> {
        let base = super::expr::deref_base(left);
        let dest = base.map_or(Value::Unknown, |b| self.eval(b, source));
        self.check_write(dest, n)
    }

    /// Bounds-checks a write of `n` bytes starting at pointer `dest`.
    pub fn check_write(&self, dest: Value, n: Option<i128>) -> Option<Overflow> {
        let Value::Ptr { buffer, offset } = dest else {
            return None;
        };
        let (Some(size), Some(bytes)) = (self.buffer(buffer).and_then(|b| b.size), n) else {
            return None;
        };
        let end = offset + bytes;
        (end > size).then(|| Overflow {
            buffer: self
                .buffer(buffer)
                .map(|b| b.label.clone())
                .unwrap_or_default(),
            end,
            size,
        })
    }

    fn record(&mut self, line: u32, text: String, overflow: Option<Overflow>) {
        self.steps.push(Step {
            line,
            text,
            overflow,
        });
    }
}

/// How a value reads in a step line.
fn show(value: Value, machine: &Machine) -> String {
    match value {
        Value::Int(n) => n.to_string(),
        Value::Null => "NULL".to_string(),
        Value::Unknown => "?".to_string(),
        Value::Ptr { buffer, offset } => {
            let size = machine
                .buffer(buffer)
                .and_then(|b| b.size)
                .map(|s| format!("{s} bytes"))
                .unwrap_or_else(|| "? bytes".to_string());
            if offset == 0 {
                format!("buffer[{size}]")
            } else {
                format!("buffer[{size}]+{offset}")
            }
        }
    }
}

/// The `parameter_list` inside a (possibly pointer) function declarator.
fn find_parameter_list(mut declarator: Node) -> Option<Node> {
    loop {
        if let Some(params) = declarator
            .child_by_field_name("parameters")
            .or_else(|| child_of_kind(declarator, "parameter_list"))
        {
            return Some(params);
        }
        declarator = declarator.child_by_field_name("declarator")?;
    }
}

fn child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    named_children(node).into_iter().find(|c| c.kind() == kind)
}

fn is_pointer_param(param: Node) -> bool {
    fn has_pointer(node: Node) -> bool {
        if node.kind() == "pointer_declarator" {
            return true;
        }
        named_children(node).iter().any(|c| has_pointer(*c))
    }
    param
        .child_by_field_name("declarator")
        .is_some_and(has_pointer)
}

fn is_integer_type(param: Node, source: &str) -> bool {
    param
        .child_by_field_name("type")
        .map(|t| is_integer_type_text(text(source, t)))
        .unwrap_or(false)
}

fn decl_is_integer(node: Node, source: &str) -> bool {
    // A pointer declarator makes it a pointer, not an integer.
    let pointer = named_children(node).iter().any(|c| {
        c.kind() == "init_declarator"
            && c.child_by_field_name("declarator")
                .is_some_and(|d| d.kind() == "pointer_declarator")
    });
    !pointer
        && node
            .child_by_field_name("type")
            .map(|t| is_integer_type_text(text(source, t)))
            .unwrap_or(false)
}

/// Whether a type's text is an integer type the emulator tracks.
fn is_integer_type_text(text: &str) -> bool {
    const INTS: [&str; 10] = [
        "int", "size_t", "ssize_t", "long", "short", "char", "unsigned", "uint", "int32", "int64",
    ];
    INTS.iter().any(|i| text.contains(i)) && !text.contains('*')
}
