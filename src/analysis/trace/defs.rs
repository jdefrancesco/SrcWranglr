//! What sets a variable before a given point in a function: declarations,
//! assignments, library calls that copy or read into it, and project calls
//! it is handed to. Read from the syntax only, in source order.

use tree_sitter::Node;

use super::calls::{library_name, method_write, write_of};
use super::flows::{flows, name_of, refers_to, Flow};
use crate::analysis::reach::{input_source, Lands};
use crate::index::syntax::{clean_name, declarator_name, field_children, named_children, text};

/// More writes than this before one point aren't followed.
const MAX_DEFS: usize = 6;
/// A function body past this many nodes is only partly read.
const MAX_SCAN_NODES: usize = 100_000;

/// One place that sets the variable.
pub(super) struct Def<'t> {
    /// The node that sets it: a declaration, declarator, assignment or call.
    pub node: Node<'t>,
    /// Where the variable is written there.
    pub name: Node<'t>,
    pub kind: DefKind<'t>,
    /// It replaces the whole value, rather than part of it or adding to it.
    pub is_whole: bool,
}

pub(super) enum DefKind<'t> {
    /// From an expression: `T x = v`, `x = v`, `for (T x : v)`.
    Set(Node<'t>),
    /// Added to: `x += v`.
    Grows(Node<'t>),
    /// Written into: `x[i] = v`, `*x = v`, `x->f = v`.
    Part(Node<'t>),
    /// Declared with no value.
    Declared,
    /// A library call (or method) copies these into it; none clears it.
    Copied {
        function: String,
        from: Vec<Node<'t>>,
    },
    /// Outside input is read into it, by `fgets()`, `std::cin`, ….
    Read(String),
    /// Handed to a project function, which may write into it.
    Passed { call: Node<'t>, index: usize },
}

impl Def<'_> {
    /// A write the caller of the function sees: into what `name` points to.
    fn is_through(&self) -> bool {
        !matches!(
            self.kind,
            DefKind::Set(_) | DefKind::Grows(_) | DefKind::Declared
        )
    }

    /// Whether, on every path to `at`, nothing set before it survives.
    fn replaces_all(&self, function: Node, at: usize) -> bool {
        let is_replacing = match self.kind {
            DefKind::Set(_) | DefKind::Declared => true,
            DefKind::Copied { .. } | DefKind::Read(_) => self.is_whole,
            DefKind::Grows(_) | DefKind::Part(_) | DefKind::Passed { .. } => false,
        };
        is_replacing && is_unconditional(self.node, function, at)
    }

    /// Only a stand-in until something real is written: a bare
    /// declaration, or `= {0}`.
    pub(super) fn is_placeholder(&self, source: &str) -> bool {
        match self.kind {
            DefKind::Declared => true,
            DefKind::Set(value) => {
                value.kind() == "initializer_list"
                    && flows(value, source)
                        .iter()
                        .all(|f| matches!(f, Flow::Constant))
            }
            _ => false,
        }
    }
}

/// The writes to `name` in `function` that finish before byte `at`, latest
/// first, back to one that replaces the whole value on every path to `at`.
/// With `is_through`, only writes into what `name` points to count, as its
/// caller would see them. `is_project_call` says whether a call's
/// `function` node names a function the project defines.
pub(super) fn reaching<'t>(
    function: Node<'t>,
    source: &str,
    name: &str,
    at: usize,
    is_through: bool,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> Vec<Def<'t>> {
    let mut found: Vec<Def> = all_defs(function, source, name, at, is_project_call)
        .into_iter()
        .filter(|d| !is_through || d.is_through())
        .collect();
    found.sort_by_key(|d| std::cmp::Reverse((d.node.end_byte(), d.node.start_byte())));
    let mut out = Vec::new();
    for def in found {
        let is_last = !is_through && def.replaces_all(function, at);
        out.push(def);
        if is_last || out.len() >= MAX_DEFS {
            break;
        }
    }
    out
}

fn all_defs<'t>(
    function: Node<'t>,
    source: &str,
    name: &str,
    at: usize,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> Vec<Def<'t>> {
    let Some(body) = function.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = vec![body];
    let mut seen = 0usize;
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > MAX_SCAN_NODES {
            break;
        }
        if node.start_byte() >= at {
            continue;
        }
        let is_done = node.end_byte() <= at || node.kind() == "for_range_loop";
        if is_done {
            out.extend(def_at(node, source, name, at, is_project_call));
        }
        stack.extend(named_children(node));
    }
    out
}

fn def_at<'t>(
    node: Node<'t>,
    source: &str,
    name: &str,
    at: usize,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> Option<Def<'t>> {
    let declared =
        |declarator: Node<'t>| declarator_name(declarator).filter(|d| text(*d, source) == name);
    let set = |node, name, value| Def {
        node,
        name,
        kind: DefKind::Set(value),
        is_whole: true,
    };
    match node.kind() {
        "init_declarator" => {
            let name = declared(node.child_by_field_name("declarator")?)?;
            Some(set(node, name, node.child_by_field_name("value")?))
        }
        "declaration" => field_children(node, "declarator")
            .into_iter()
            .filter(|d| d.kind() != "init_declarator")
            .find_map(declared)
            .map(|name| Def {
                node,
                name,
                kind: DefKind::Declared,
                is_whole: true,
            }),
        "for_range_loop" => {
            let declarator = node.child_by_field_name("declarator")?;
            let name = declared(declarator).filter(|_| declarator.end_byte() <= at)?;
            Some(set(declarator, name, node.child_by_field_name("right")?))
        }
        "assignment_expression" => assignment(node, source, name),
        "call_expression" => call_def(node, source, name, is_project_call),
        "binary_expression" => stream_read(node, source, name),
        _ => None,
    }
}

fn assignment<'t>(node: Node<'t>, source: &str, name: &str) -> Option<Def<'t>> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let operator = node
        .child_by_field_name("operator")
        .map_or("=", |o| text(o, source));
    let (kind, is_whole) = match refers_to(left, source, name)? {
        true if operator == "=" => (DefKind::Set(right), true),
        true => (DefKind::Grows(right), false),
        false => (DefKind::Part(right), false),
    };
    Some(Def {
        node,
        name: left,
        kind,
        is_whole,
    })
}

/// A call that reads input into `name`, copies into it, or is handed it.
fn call_def<'t>(
    call: Node<'t>,
    source: &str,
    name: &str,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> Option<Def<'t>> {
    let function = call.child_by_field_name("function")?;
    let args = arguments(call);
    if function.kind() == "field_expression" {
        return method_def(call, function, &args, source, name);
    }
    let site = Site {
        call,
        args: &args,
        source,
        name,
    };
    let called = clean_name(text(function, source));
    if let Some((label, lands)) = input_source(&called) {
        let positions = match lands {
            Lands::Arg(i) => vec![i],
            Lands::ArgsFrom(i) => (i..args.len()).collect(),
            Lands::Returned => Vec::new(),
        };
        return site.write(positions, || DefKind::Read(format!("{label}()")), false);
    }
    let library = library_name(&called);
    if let Some(write) = write_of(library) {
        let from: Vec<Node> = write
            .from
            .indices(args.len())
            .into_iter()
            .map(|i| args[i])
            .collect();
        let copied = || DefKind::Copied {
            function: library.to_string(),
            from: from.clone(),
        };
        return site.write(write.into.indices(args.len()), copied, write.is_append);
    }
    is_project_call(function).then(|| site.passed()).flatten()
}

/// A call's arguments, looked at for a write into `name`.
struct Site<'a, 't> {
    call: Node<'t>,
    args: &'a [Node<'t>],
    source: &'a str,
    name: &'a str,
}

impl<'t> Site<'_, 't> {
    /// The first argument at `positions` that is `name` (or part of it),
    /// as written there with `kind`.
    fn write(
        &self,
        positions: Vec<usize>,
        kind: impl Fn() -> DefKind<'t>,
        is_append: bool,
    ) -> Option<Def<'t>> {
        positions.into_iter().find_map(|i| {
            let arg = *self.args.get(i)?;
            let is_whole = refers_to(arg, self.source, self.name)?;
            Some(Def {
                node: self.call,
                name: arg,
                kind: kind(),
                is_whole: is_whole && !is_append,
            })
        })
    }

    /// `name` handed to a project function, which may write into it.
    fn passed(&self) -> Option<Def<'t>> {
        let index = self
            .args
            .iter()
            .position(|a| refers_to(*a, self.source, self.name).is_some())?;
        Some(Def {
            node: self.call,
            name: self.args[index],
            kind: DefKind::Passed {
                call: self.call,
                index,
            },
            is_whole: false,
        })
    }
}

/// `s.assign(x)`, `s.append(x)`, … with `s` being `name`.
fn method_def<'t>(
    call: Node<'t>,
    function: Node<'t>,
    args: &[Node<'t>],
    source: &str,
    name: &str,
) -> Option<Def<'t>> {
    let receiver = function.child_by_field_name("argument")?;
    let method = text(function.child_by_field_name("field")?, source);
    let is_append = method_write(method)?;
    let is_whole = refers_to(receiver, source, name)?;
    Some(Def {
        node: call,
        name: receiver,
        kind: DefKind::Copied {
            function: method.to_string(),
            from: args.to_vec(),
        },
        is_whole: is_whole && !is_append,
    })
}

/// `std::cin >> name` (also further along `cin >> a >> name`).
fn stream_read<'t>(node: Node<'t>, source: &str, name: &str) -> Option<Def<'t>> {
    let is_shift = |n: Node| {
        n.kind() == "binary_expression"
            && n.child_by_field_name("operator")
                .is_some_and(|o| text(o, source) == ">>")
    };
    if !is_shift(node) {
        return None;
    }
    let right = node.child_by_field_name("right")?;
    let is_whole = refers_to(right, source, name)?;
    let mut stream = node.child_by_field_name("left")?;
    while is_shift(stream) {
        stream = stream.child_by_field_name("left")?;
    }
    let stream_name = name_of(stream, source);
    let is_console = matches!(
        stream_name.as_str(),
        "cin" | "std::cin" | "wcin" | "std::wcin"
    );
    is_console.then_some(Def {
        node,
        name: right,
        kind: DefKind::Read(stream_name),
        is_whole,
    })
}

/// A call's arguments, comments left out.
pub(super) fn arguments(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.kind() != "comment")
        .collect()
}

/// Whether `node` runs on every path through `function` that reaches `at`:
/// it isn't inside a branch, loop body, `case` or `&&`/`||` operand that
/// `at` is outside of.
fn is_unconditional(node: Node, function: Node, at: usize) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if parent == function {
            return true;
        }
        let holds_at = child.start_byte() <= at && at <= child.end_byte();
        if is_branch(parent, child) && !holds_at {
            return false;
        }
        child = parent;
    }
    true
}

fn is_branch(parent: Node, child: Node) -> bool {
    let is = |field: &str| parent.child_by_field_name(field) == Some(child);
    match parent.kind() {
        "if_statement" | "conditional_expression" => is("consequence") || is("alternative"),
        "while_statement" | "for_statement" | "for_range_loop" => is("body"),
        "case_statement" | "catch_clause" => true,
        "binary_expression" => {
            is("right")
                && parent
                    .child_by_field_name("operator")
                    .is_some_and(|o| matches!(o.kind(), "&&" | "||"))
        }
        _ => false,
    }
}
