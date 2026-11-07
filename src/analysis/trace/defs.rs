//! What sets a variable before a given point in a function: declarations,
//! assignments, library calls that copy or read into it, and project calls
//! it is handed to. Read from the syntax only, in source order.

use tree_sitter::Node;

use super::calls::{library_name, method_write, write_of};
use super::files::parameters;
use super::flows::{access_path, flows, name_of, refers_to, Flow};
use super::scope::{binding, contains, is_exclusive, loop_around, Declaration, Jumps};
use crate::analysis::reach::{input_source, Lands};
use crate::index::syntax::{clean_name, declarator_name, field_children, named_children, text};

/// More writes than this before one point aren't followed.
const MAX_DEFS: usize = 6;
/// A function body past this many nodes is only partly read.
const MAX_SCAN_NODES: usize = 100_000;

/// One place that sets the variable.
#[derive(Clone)]
pub(super) struct Def<'t> {
    /// The node that sets it: a declaration, declarator, assignment or call.
    pub node: Node<'t>,
    /// Where the variable is written there.
    pub name: Node<'t>,
    pub kind: DefKind<'t>,
    /// It replaces the whole value, rather than part of it or adding to it.
    pub is_whole: bool,
    /// It declares the variable (and so starts a new one).
    pub is_declaration: bool,
}

#[derive(Clone)]
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

    /// Whether it replaces the whole variable (wherever it runs).
    fn is_replacing(&self) -> bool {
        match self.kind {
            DefKind::Set(_) | DefKind::Declared => true,
            DefKind::Copied { .. } | DefKind::Read(_) => self.is_whole,
            DefKind::Grows(_) | DefKind::Part(_) | DefKind::Passed { .. } => false,
        }
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

/// Whether `def` (a write to a struct) can be to `field` of it: to the
/// whole struct, to a field holding it, or to the field itself. Any write
/// is, when no field is wanted.
pub(super) fn is_to_field(field: &[String], def: &Def, source: &str) -> bool {
    if field.is_empty() {
        return true;
    }
    let (_, written) = access_path(def.name, source);
    written.is_empty() || field.starts_with(&written) || written.starts_with(field)
}

/// Whether `def` writes all of `field` (the field, a field holding it, or
/// the struct through a pointer: `*p = …`), so what it held before is gone.
pub(super) fn replaces_field(field: &[String], def: &Def, source: &str) -> bool {
    if field.is_empty()
        || !matches!(
            def.kind,
            DefKind::Part(_) | DefKind::Copied { .. } | DefKind::Read(_)
        )
    {
        return false;
    }
    let (_, written) = access_path(def.name, source);
    let is_deref = def.name.kind() == "pointer_expression"
        && def
            .name
            .child_by_field_name("operator")
            .is_some_and(|o| text(o, source) == "*");
    match written.is_empty() {
        true => is_deref,
        false => field.starts_with(&written),
    }
}

/// What sets a variable before a use.
pub(super) struct Reaching<'t> {
    /// Latest first; then writes later in a loop around the use, which
    /// reach it on the next pass.
    pub defs: Vec<Def<'t>>,
    /// One of them replaces the value on every path to the use, so nothing
    /// from outside the function (a parameter's, a global's) reaches it.
    pub is_complete: bool,
    /// There were more than `MAX_DEFS`.
    pub is_truncated: bool,
    /// A real write (not a stand-in like a bare declaration) among them
    /// runs on every path to the use.
    pub has_certain_write: bool,
}

/// What is looked up: the writes to `name` that reach byte `at`.
pub(super) struct Query<'a, 't> {
    pub name: &'a str,
    pub at: usize,
    /// Only writes into what `name` points to count, as its caller would
    /// see them.
    pub is_through: bool,
    /// Whether a call's `function` node names a function the project
    /// defines.
    pub is_project_call: &'a dyn Fn(Node<'t>) -> bool,
    /// Whether a write is to what is wanted (not another field of it).
    pub keeps: &'a dyn Fn(&Def<'t>) -> bool,
    /// Whether a write replaces all that is wanted, though not the whole
    /// variable: the field of a struct wanted.
    pub replaces: &'a dyn Fn(&Def<'t>) -> bool,
    /// `at` is a use, which writes later in a loop reach on the next pass
    /// (not the end of a write being traced from).
    pub has_later: bool,
}

/// The writes to the variable `q.name` means at byte `q.at` of `function`
/// that can reach it: finished before it, latest first, back to one that
/// replaces the value on every path (no branch, loop or jump around it);
/// then the ones later in a loop around it. A write to another variable of
/// the same name (in a closed block, or a lambda's parameter) or in the
/// other arm of an `if` doesn't count.
pub(super) fn reaching<'t>(function: Node<'t>, source: &str, q: &Query<'_, 't>) -> Reaching<'t> {
    Writes::of(function, source, q.name, q.is_project_call).reaching(function, source, q)
}

/// Every write to one name in a function, read once for many lookups.
pub(super) struct Writes<'t> {
    all: Vec<Def<'t>>,
    /// The declarations of the name: which variable it means where.
    declarations: Vec<Declaration>,
    jumps: Jumps<'t>,
}

impl<'t> Writes<'t> {
    /// The writes to `name` in `function`.
    pub(super) fn of(
        function: Node<'t>,
        source: &str,
        name: &str,
        is_project_call: &dyn Fn(Node<'t>) -> bool,
    ) -> Self {
        let (all, parameters) = all_defs(function, source, name, is_project_call);
        let declarations = all
            .iter()
            .filter(|d| d.is_declaration)
            .map(|d| d.name)
            .chain(parameters)
            .map(Declaration::of)
            .collect();
        Self {
            all,
            declarations,
            jumps: Jumps::of(function, source),
        }
    }

    /// Those that reach `q.at`, as `reaching` says.
    pub(super) fn reaching(
        &self,
        function: Node<'t>,
        source: &str,
        q: &Query<'_, 't>,
    ) -> Reaching<'t> {
        let meant = binding(&self.declarations, q.at);
        let counts = |d: &Def<'t>| {
            (!q.is_through || d.is_through())
                && (q.keeps)(d)
                && binding(&self.declarations, d.name.start_byte()) == meant
                && !is_exclusive(d.node, q.at, function)
        };
        let counted: Vec<Def> = self.all.iter().filter(|d| counts(d)).cloned().collect();
        let around = loop_around(function, q.at, meant).filter(|_| q.has_later);
        let (before, later) = before_and_later(counted, around, q.at);
        let mut found = Reaching {
            defs: Vec::new(),
            is_complete: false,
            is_truncated: false,
            has_certain_write: false,
        };
        // Back to the first write that replaces the value; earlier ones are
        // gone by `at`. Then the loop's later writes.
        let is_certain = |d: &Def| {
            is_unconditional(d.node, function, q.at) && !self.jumps.can_skip(d.node, q.at)
        };
        let replacing = before
            .iter()
            .position(|d| !q.is_through && (d.is_replacing() || (q.replaces)(d)) && is_certain(d));
        found.has_certain_write = before
            .iter()
            .take(replacing.map_or(before.len(), |i| i + 1))
            .any(|d| !d.is_placeholder(source) && is_certain(d));
        found.is_complete = replacing.is_some();
        let kept = replacing.map_or(before.len(), |i| i + 1);
        for def in before.into_iter().take(kept).chain(later) {
            if found.defs.len() >= MAX_DEFS {
                found.is_truncated = true;
                break;
            }
            found.defs.push(def);
        }
        found
    }
}

/// Whether `name` at byte `at` of `function` is one of its own variables,
/// declared in it or a parameter, rather than a global or a member.
pub(super) fn is_own(function: Node, source: &str, name: &str, at: usize) -> bool {
    let is_parameter = parameters(function)
        .iter()
        .any(|p| p.is_some_and(|p| text(p, source) == name));
    if is_parameter {
        return true;
    }
    let (all, lambda_parameters) = all_defs(function, source, name, &|_| false);
    let declarations: Vec<Declaration> = all
        .iter()
        .filter(|d| d.is_declaration)
        .map(|d| d.name)
        .chain(lambda_parameters)
        .map(Declaration::of)
        .collect();
    binding(&declarations, at).is_some()
}

/// `defs` split into those finished before `at`, latest first, and those
/// later in the loop `around` it (which reach it on the next pass), in
/// source order. Others can't reach `at`.
fn before_and_later<'t>(
    defs: Vec<Def<'t>>,
    around: Option<Node<'t>>,
    at: usize,
) -> (Vec<Def<'t>>, Vec<Def<'t>>) {
    let is_later_in_loop = |d: &Def| {
        d.node.start_byte() >= at
            && around.is_some_and(|l| {
                contains(l, d.node.start_byte()) && d.node.end_byte() <= l.end_byte()
            })
    };
    let (mut before, mut later): (Vec<Def>, Vec<Def>) = (Vec::new(), Vec::new());
    for def in defs {
        if def.node.end_byte() <= at {
            before.push(def);
        } else if is_later_in_loop(&def) {
            later.push(def);
        }
    }
    before.sort_by_key(|d| std::cmp::Reverse((d.node.end_byte(), d.node.start_byte())));
    later.sort_by_key(|d| d.node.start_byte());
    (before, later)
}

/// Every write to `name` in `function`, in no particular order, and the
/// lambda parameters named `name` (other variables, in their lambdas).
/// Only the parts of the body where `name` is written are read.
fn all_defs<'t>(
    function: Node<'t>,
    source: &str,
    name: &str,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> (Vec<Def<'t>>, Vec<Node<'t>>) {
    let none = || (Vec::new(), Vec::new());
    let Some(body) = function.child_by_field_name("body") else {
        return none();
    };
    // `conn->buf` is written with `buf` in it, however it is spaced.
    let needle = name.rsplit(['.', '>']).next().unwrap_or(name);
    let Some(text) = source.get(body.byte_range()).filter(|_| !needle.is_empty()) else {
        return none();
    };
    let places: Vec<usize> = text
        .match_indices(needle)
        .map(|(i, _)| body.start_byte() + i)
        .collect();
    let mentions = |node: Node| {
        let first = places.partition_point(|&p| p < node.start_byte());
        places.get(first).is_some_and(|&p| p < node.end_byte())
    };
    let mut out = Vec::new();
    let mut parameters = Vec::new();
    let mut stack = vec![body];
    let mut seen = 0usize;
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > MAX_SCAN_NODES {
            break;
        }
        if !mentions(node) {
            continue;
        }
        if node.kind() == "lambda_expression" {
            parameters.extend(lambda_parameters(node, source, name));
        }
        out.extend(def_at(node, source, name, is_project_call));
        stack.extend(named_children(node));
    }
    (out, parameters)
}

/// The parameters named `name` of a lambda, by their names' nodes.
fn lambda_parameters<'t>(lambda: Node<'t>, source: &str, name: &str) -> Vec<Node<'t>> {
    lambda
        .child_by_field_name("declarator")
        .and_then(|d| d.child_by_field_name("parameters"))
        .map(named_children)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            p.child_by_field_name("declarator")
                .and_then(declarator_name)
        })
        .filter(|n| text(*n, source) == name)
        .collect()
}

fn def_at<'t>(
    node: Node<'t>,
    source: &str,
    name: &str,
    is_project_call: &dyn Fn(Node<'t>) -> bool,
) -> Option<Def<'t>> {
    let declared =
        |declarator: Node<'t>| declarator_name(declarator).filter(|d| text(*d, source) == name);
    match node.kind() {
        "init_declarator" => {
            let name = declared(node.child_by_field_name("declarator")?)?;
            Some(declaring(node, name, node.child_by_field_name("value")?))
        }
        "declaration" => {
            let declarators = field_children(node, "declarator");
            // `if (char *p = f())`: in a condition, the value is the
            // declaration's own.
            if let Some(value) = node.child_by_field_name("value") {
                let name = declarators.into_iter().find_map(declared)?;
                return Some(declaring(node, name, value));
            }
            let name = declarators
                .into_iter()
                .filter(|d| d.kind() != "init_declarator")
                .find_map(declared)?;
            Some(Def {
                kind: DefKind::Declared,
                ..declaring(node, name, node)
            })
        }
        "for_range_loop" => {
            let declarator = node.child_by_field_name("declarator")?;
            let name = declared(declarator)?;
            Some(declaring(
                declarator,
                name,
                node.child_by_field_name("right")?,
            ))
        }
        "assignment_expression" => assignment(node, source, name),
        "call_expression" => call_def(node, source, name, is_project_call),
        "binary_expression" => stream_read(node, source, name),
        _ => None,
    }
}

/// A declaration of `name` at `node`, set from `value`.
fn declaring<'t>(node: Node<'t>, name: Node<'t>, value: Node<'t>) -> Def<'t> {
    Def {
        node,
        name,
        kind: DefKind::Set(value),
        is_whole: true,
        is_declaration: true,
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
        is_declaration: false,
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
                is_declaration: false,
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
            is_declaration: false,
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
        is_declaration: false,
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
        is_declaration: false,
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
        if is_branch(parent, child) && !contains(child, at) {
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
        "while_statement" | "for_range_loop" => is("body"),
        // The update runs only after the body, from the second pass on.
        "for_statement" => is("body") || is("update"),
        // A lambda's body runs when it is called, if ever.
        "case_statement" | "catch_clause" | "lambda_expression" => true,
        // An exception may leave a `try` body before a write.
        "try_statement" => is("body"),
        "binary_expression" => {
            is("right")
                && parent
                    .child_by_field_name("operator")
                    .is_some_and(|o| matches!(o.kind(), "&&" | "||"))
        }
        _ => false,
    }
}
