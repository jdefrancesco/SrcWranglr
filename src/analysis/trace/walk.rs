//! The search behind a trace: from a value back through what sets it, a
//! depth-first walk that records a path each time it reaches an origin.

use tree_sitter::Node;

use super::calls::{library_name, risky_args};
use super::defs::{
    arguments, is_to_field, reaching, replaces_field, Def, DefKind, Query, Reaching,
};
use super::files::{
    call_at, call_name_node, call_named_at, call_starting_at, called_name, callee, definitions,
    function_around, function_name, function_on_line, may_call, parameters, snippet, step,
    symbol_of, variable_at, Files,
};
use super::flows::{access_path, carries_data, field_base, fields_of, flows, name_of, Flow};
use super::sites::{stepped_into, CallRef, CallSite, CallSites};
use super::{Endpoint, Step, Trace, TracePath};
use crate::analysis::reach::{input_source, COMMAND_LINE, MAIN_NAMES};
use crate::index::syntax::{line_of, text};
use crate::model::{CodeGraph, SymbolId};

/// Paths kept per trace.
const MAX_PATHS: usize = 24;
/// Values looked up per trace, so a huge call graph can't stall it.
const MAX_WORK: usize = 2_000;
/// Steps in one path.
const MAX_STEPS: usize = 40;
/// Call sites followed per function, and definitions per called function.
const MAX_CALLERS: usize = 8;
const MAX_DEFINITIONS: usize = 4;
/// `return` statements followed per definition.
const MAX_RETURNS: usize = 6;
/// Calls stepped into for their result, one inside another.
const MAX_CALL_DEPTH: usize = 8;
/// How long one trace may take before it stops with what it has: it runs
/// while the window waits.
const MAX_TIME: std::time::Duration = std::time::Duration::from_millis(750);
/// How deeply the walk may nest (a value inside a value, a call inside a
/// call) before it stops: keeps the stack bounded on hostile code.
const MAX_DEPTH: usize = 64;

/// A value to trace back: `name` as used at byte `at` of `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Want {
    file: String,
    name: String,
    at: usize,
    /// Calls stepped into, innermost last: a parameter's value comes from
    /// the innermost one's arguments, not from every caller.
    returns_to: Vec<CallRef>,
    /// Only writes into what `name` points to count: a caller handed it
    /// over and wants what gets written there.
    is_through: bool,
    /// The field of `name` (a struct) wanted, if any: writes to its other
    /// fields don't count.
    field: Vec<String>,
    /// `at` is the end of a write the trace starts from, not a use: what
    /// the loop writes later doesn't reach it.
    is_at_write: bool,
}

impl Want {
    /// `name` as used at `at` of `file`.
    fn new(file: &str, name: String, at: usize, returns_to: &[CallRef]) -> Self {
        Self {
            file: file.to_string(),
            name,
            at,
            returns_to: returns_to.to_vec(),
            is_through: false,
            field: Vec::new(),
            is_at_write: false,
        }
    }

    /// `written` (a name, or a field path like `q->hdr.len`) as used at
    /// `at`: a field is wanted as that field of its struct, so writes to
    /// the whole struct count too, in order.
    fn of_written(file: &str, written: String, at: usize, returns_to: &[CallRef]) -> Self {
        match field_base(&written) {
            Some(base) => Self {
                field: fields_of(&written),
                ..Self::new(file, base.to_string(), at, returns_to)
            },
            None => Self::new(file, written, at, returns_to),
        }
    }

    /// Whether `def` (a write to `name`) can be to the field wanted.
    fn keeps(&self, def: &Def, source: &str) -> bool {
        is_to_field(&self.field, def, source)
    }

    /// Whether `def` writes all of the field wanted.
    fn replaces(&self, def: &Def, source: &str) -> bool {
        replaces_field(&self.field, def, source)
    }
}

pub(super) struct Tracer<'g> {
    files: Files<'g>,
    /// Where each project function is called from.
    call_sites: CallSites,
    /// The current path, from the subject back.
    trail: Vec<Step>,
    /// The values being traced along the current path, to stop cycles.
    wants: Vec<Want>,
    paths: Vec<TracePath>,
    work: usize,
    /// When the trace stops with what it has.
    deadline: std::time::Instant,
    /// How deeply the walk is nested right now.
    depth: usize,
    is_cut_short: bool,
}

impl<'g> Tracer<'g> {
    pub(super) fn new(graph: &'g CodeGraph) -> Self {
        Self {
            files: Files::new(graph),
            call_sites: CallSites::default(),
            trail: Vec::new(),
            wants: Vec::new(),
            paths: Vec::new(),
            work: 0,
            deadline: std::time::Instant::now() + MAX_TIME,
            depth: 0,
            is_cut_short: false,
        }
    }

    /// Traces the arguments that make the call at `line`, `column` of
    /// `file` risky.
    pub(super) fn call(mut self, file: &str, line: u32, column: u32) -> Option<Trace> {
        let tree = self.files.tree(file)?;
        let source = self.files.source(file);
        let call = call_at(tree.root_node(), line, column)?;
        let function = call.child_by_field_name("function")?;
        let called = called_name(function, source);
        let symbol = self.symbol_around(tree.root_node(), file, call.start_byte());
        let note = format!("the risky call to `{called}()`");
        self.trail
            .push(step(source, file, call_name_node(function), symbol, note));
        let args = arguments(call);
        for index in risky_args(library_name(&called), args.len()) {
            if carries_data(args[index], source) {
                self.follow_value(file, args[index], &[]);
            }
        }
        Some(self.finish(snippet(call, source), file, line_of(call)))
    }

    /// Traces the variable written at `line`, `column` of `file`.
    pub(super) fn variable(mut self, file: &str, line: u32, column: u32) -> Option<Trace> {
        let tree = self.files.tree(file)?;
        let source = self.files.source(file);
        let node = variable_at(tree.root_node(), line, column)?;
        let name = name_of(node, source);
        let function = function_around(tree.root_node(), node.start_byte())?;
        let at = match set_by(node) {
            // Picked where it is set: start from that write.
            Some(write) => write.end_byte(),
            None => {
                let symbol = symbol_of(self.files.graph(), file, function);
                let note = format!("`{name}` here");
                self.trail.push(step(source, file, node, symbol, note));
                node.start_byte()
            }
        };
        let is_at_write = at != node.start_byte();
        self.trace(Want {
            is_at_write,
            ..Want::of_written(file, name.clone(), at, &[])
        });
        Some(self.finish(format!("`{name}`"), file, line))
    }

    fn finish(self, subject: String, file: &str, line: u32) -> Trace {
        Trace {
            subject,
            file: file.to_string(),
            line,
            direction: super::Direction::Back,
            paths: super::ordered(self.paths),
            is_cut_short: self.is_cut_short,
        }
    }

    fn symbol_around(&self, root: Node, file: &str, at: usize) -> Option<SymbolId> {
        function_around(root, at).and_then(|f| symbol_of(self.files.graph(), file, f))
    }

    /// Records the current path as one that starts at `endpoint`.
    fn end(&mut self, endpoint: Endpoint) {
        if self.paths.len() >= MAX_PATHS {
            self.is_cut_short = true;
            return;
        }
        let steps = self.trail.iter().rev().cloned().collect();
        self.paths.push(TracePath { endpoint, steps });
    }

    fn stop(&mut self, why: String) {
        self.end(Endpoint::Stopped(why));
    }

    fn is_over_budget(&mut self) -> bool {
        let is_over = self.paths.len() >= MAX_PATHS
            || self.work >= MAX_WORK
            || std::time::Instant::now() >= self.deadline;
        self.is_cut_short |= is_over;
        is_over
    }

    /// Follows what sets `want` before it is used.
    fn trace(&mut self, want: Want) {
        self.deeper(|this| this.trace_here(want));
    }

    /// Runs `walk` one level deeper, or ends the path if that is too deep.
    fn deeper(&mut self, walk: impl FnOnce(&mut Self)) {
        if self.depth >= MAX_DEPTH {
            self.is_cut_short = true;
            return self.stop("nested too deeply to follow further".into());
        }
        self.depth += 1;
        walk(self);
        self.depth -= 1;
    }

    fn trace_here(&mut self, want: Want) {
        if self.is_over_budget() || self.wants.contains(&want) {
            return;
        }
        if self.trail.len() >= MAX_STEPS {
            return self.stop("the path is too long to follow further".into());
        }
        self.work += 1;
        let Some(tree) = self.files.tree(&want.file) else {
            return self.stop(format!("{} couldn't be read", want.file));
        };
        let source = self.files.source(&want.file);
        let Some(function) = function_around(tree.root_node(), want.at) else {
            return self.stop(format!("`{}` is outside any function", want.name));
        };
        let graph = self.files.graph();
        let file = want.file.clone();
        let is_project_call = |f: Node| callee(graph, &file, f).is_some();
        let keeps = |d: &Def| want.keeps(d, source);
        let replaces = |d: &Def| want.replaces(d, source);
        let query = Query {
            name: &want.name,
            at: want.at,
            is_through: want.is_through,
            is_project_call: &is_project_call,
            keeps: &keeps,
            replaces: &replaces,
            has_later: !want.is_at_write,
        };
        let found = reaching(function, source, &query);
        self.is_cut_short |= found.is_truncated;
        self.wants.push(want.clone());
        self.follow_defs(&want, function, source, &found);
        // Writes that may not run leave the value it had coming in.
        if !found.is_complete && !found.is_truncated {
            self.undefined(&want, function, source);
        }
        self.wants.pop();
    }

    /// Follows each write found. A stand-in (a bare declaration, `= {0}`)
    /// only when the real writes may not run, or lead nowhere: a buffer
    /// filled by a call is what it was filled with, unless the call writes
    /// nothing.
    fn follow_defs(&mut self, want: &Want, function: Node, source: &str, found: &Reaching) {
        let symbol = symbol_of(self.files.graph(), &want.file, function);
        let (stand_ins, real): (Vec<&Def>, Vec<&Def>) =
            found.defs.iter().partition(|d| d.is_placeholder(source));
        let paths_before = self.paths.len();
        for def in real {
            self.follow_one(want, def, symbol, source);
        }
        if !found.has_certain_write || self.paths.len() == paths_before {
            for def in stand_ins {
                self.follow_one(want, def, symbol, source);
            }
        }
    }

    fn follow_one(&mut self, want: &Want, def: &Def, symbol: Option<SymbolId>, source: &str) {
        let note = def_note(def, &want.name, source);
        self.trail
            .push(step(source, &want.file, def.name, symbol, note));
        self.follow_def(want, def);
        self.trail.pop();
    }

    fn follow_def(&mut self, want: &Want, def: &Def) {
        let file = want.file.as_str();
        let name = &want.name;
        match &def.kind {
            DefKind::Set(value) => self.follow_value(file, *value, &want.returns_to),
            // What was there before is among the writes found too.
            DefKind::Grows(value) | DefKind::Part(value) => {
                self.follow_value(file, *value, &want.returns_to);
            }
            DefKind::Declared => self.stop(format!("`{name}` has no value before this")),
            DefKind::Copied { from, .. } if from.is_empty() => {
                self.stop(format!("`{name}` is cleared here"));
            }
            DefKind::Copied { from, .. } => {
                for value in from {
                    self.follow_value(file, *value, &want.returns_to);
                }
            }
            DefKind::Read(source) => self.end(Endpoint::Input(source.clone())),
            DefKind::Passed { call, index } => self.follow_into_callee(want, *call, *index),
        }
    }

    /// Follows what `value` (in `file`) is made from.
    fn follow_value(&mut self, file: &str, value: Node, returns_to: &[CallRef]) {
        self.deeper(|this| this.value_here(file, value, returns_to));
    }

    fn value_here(&mut self, file: &str, value: Node, returns_to: &[CallRef]) {
        let source = self.files.source(file);
        let found = flows(value, source);
        if found.is_empty() {
            return self.stop(format!(
                "set from `{}` (not followed)",
                snippet(value, source)
            ));
        }
        if found.iter().all(|f| matches!(f, Flow::Constant)) {
            return self.stop("set to a constant".into());
        }
        for flow in found {
            match flow {
                Flow::Var(node) => self.trace(Want::of_written(
                    file,
                    name_of(node, source),
                    node.start_byte(),
                    returns_to,
                )),
                Flow::Call(call) => self.follow_call(file, call, returns_to),
                Flow::Opaque(why) => self.stop(why.to_string()),
                Flow::Constant => {}
            }
        }
    }

    /// Follows what `call` returns.
    fn follow_call(&mut self, file: &str, call: Node, returns_to: &[CallRef]) {
        let source = self.files.source(file);
        let Some(function) = call.child_by_field_name("function") else {
            return;
        };
        let called = called_name(function, source);
        let is_method = function.kind() == "field_expression";
        if let Some((label, _)) = input_source(&called).filter(|_| !is_method) {
            return self.end(Endpoint::Input(format!("{label}()")));
        }
        let project = callee(self.files.graph(), file, function);
        if let Some(symbol) = project {
            if self.follow_returns(file, call, symbol, &called, returns_to) {
                return;
            }
        }
        match function.kind() {
            // A method's result comes from its object: `s.c_str()`.
            "field_expression" => {
                if let Some(object) = function.child_by_field_name("argument") {
                    self.follow_value(file, object, returns_to);
                }
            }
            "identifier" | "qualified_identifier" | "template_function" => {
                let args: Vec<Node> = arguments(call)
                    .into_iter()
                    .filter(|a| carries_data(*a, source))
                    .collect();
                if args.is_empty() {
                    let place = if project.is_some() {
                        "with no `return` value indexed"
                    } else {
                        "outside the project"
                    };
                    return self.stop(format!("returned by `{called}()`, {place}"));
                }
                for arg in args {
                    self.follow_value(file, arg, returns_to);
                }
            }
            _ => self.stop("returned through a function pointer (not followed)".into()),
        }
    }

    /// Follows the `return` values of `symbol`'s definitions back from
    /// `call`. Whether there were any.
    fn follow_returns(
        &mut self,
        file: &str,
        call: Node,
        symbol: SymbolId,
        called: &str,
        returns_to: &[CallRef],
    ) -> bool {
        if returns_to.len() >= MAX_CALL_DEPTH {
            self.stop("calls nest too deep to follow".into());
            return true;
        }
        let inner = stepped_into(returns_to, file, call);
        let mut has_returns = false;
        let found = definitions(self.files.graph(), symbol, file);
        for (def_file, line) in found.into_iter().take(MAX_DEFINITIONS) {
            let Some(tree) = self.files.tree(&def_file) else {
                continue;
            };
            let source = self.files.source(&def_file);
            let Some(function) = function_on_line(tree.root_node(), line) else {
                continue;
            };
            for value in returned_values(function).into_iter().take(MAX_RETURNS) {
                has_returns = true;
                let note = format!("`{called}()` returns `{}`", snippet(value, source));
                self.trail
                    .push(step(source, &def_file, value, Some(symbol), note));
                self.follow_value(&def_file, value, &inner);
                self.trail.pop();
            }
        }
        has_returns
    }

    /// `want` isn't set in `function` before its use: a parameter, a
    /// field of something that is, or a name from elsewhere.
    fn undefined(&mut self, want: &Want, function: Node, source: &str) {
        if want.is_through {
            // The function writes nothing into it.
            return;
        }
        let index = parameters(function)
            .iter()
            .position(|p| p.is_some_and(|p| text(p, source) == want.name));
        if let Some(index) = index {
            return self.parameter(want, function, source, index);
        }
        if want.name == "this" {
            let member = want.field.join(".");
            return self.stop(format!(
                "`this->{member}` is a member, set elsewhere (not followed)"
            ));
        }
        let name = function_name(function, source);
        self.stop(format!(
            "`{}` isn't set in `{name}`: a global, a member or a macro, set elsewhere (not followed)",
            want.name
        ));
    }

    /// `want` is parameter `index` of `function`: from the command line,
    /// the call stepped in from, or every call site.
    fn parameter(&mut self, want: &Want, function: Node, source: &str, index: usize) {
        let name = function_name(function, source);
        let symbol = symbol_of(self.files.graph(), &want.file, function);
        if MAIN_NAMES.contains(&name.as_str()) {
            if let Some(Some(param)) = parameters(function).get(index) {
                let note = format!("`{}` is the command line", want.name);
                self.trail
                    .push(step(source, &want.file, *param, symbol, note));
                self.end(Endpoint::Input(COMMAND_LINE.to_string()));
                self.trail.pop();
            }
            return;
        }
        if let Some((call, rest)) = want.returns_to.split_last() {
            return self.back_to_call(call, rest, index, want);
        }
        let Some(symbol) = symbol else {
            return self.stop(format!(
                "`{}` is a parameter of `{name}`, which isn't indexed",
                want.name
            ));
        };
        let definition = (want.file.as_str(), line_of(function));
        let graph = self.files.graph();
        let sites: Vec<CallSite> = self
            .call_sites_of(symbol)
            .into_iter()
            .filter(|(file, _, _)| may_call(graph, symbol, definition, file))
            .collect();
        if sites.is_empty() {
            return self.stop(format!(
                "`{}` is a parameter of `{name}`, and nothing indexed calls it",
                want.name
            ));
        }
        self.is_cut_short |= sites.len() > MAX_CALLERS;
        for (file, span, context) in sites.into_iter().take(MAX_CALLERS) {
            let Some(tree) = self.files.tree(&file) else {
                continue;
            };
            if let Some(call) = call_named_at(tree.root_node(), span.line, span.start) {
                self.follow_argument(&file, call, index, want, context, &[]);
            }
        }
    }

    /// Back out of the call stepped into, to its argument `index`.
    fn back_to_call(&mut self, call: &CallRef, rest: &[CallRef], index: usize, param: &Want) {
        let Some(tree) = self.files.tree(&call.file) else {
            return;
        };
        let Some(node) = call_starting_at(tree.root_node(), call.at) else {
            return;
        };
        let symbol = self.symbol_around(tree.root_node(), &call.file, call.at);
        self.follow_argument(&call.file, node, index, param, symbol, rest);
    }

    /// Follows argument `index` of `call`, passed as the parameter `wanted`
    /// is: for a field of it, that field of the argument's struct.
    fn follow_argument(
        &mut self,
        file: &str,
        call: Node,
        index: usize,
        wanted: &Want,
        symbol: Option<SymbolId>,
        returns_to: &[CallRef],
    ) {
        let param = wanted.name.as_str();
        let source = self.files.source(file);
        let Some(arg) = arguments(call).get(index).copied() else {
            let note = format!("leaves out `{param}`");
            self.trail.push(step(source, file, call, symbol, note));
            self.stop(format!(
                "`{param}` takes its default value, which isn't followed"
            ));
            self.trail.pop();
            return;
        };
        let note = format!("passes `{}` as `{param}`", snippet(arg, source));
        self.trail.push(step(source, file, arg, symbol, note));
        let (base, fields) = access_path(arg, source);
        if !wanted.field.is_empty() && base.kind() == "identifier" {
            let name = text(base, source).to_string();
            self.trace(Want {
                field: fields
                    .into_iter()
                    .chain(wanted.field.iter().cloned())
                    .collect(),
                ..Want::new(file, name, base.start_byte(), returns_to)
            });
        } else {
            self.follow_value(file, arg, returns_to);
        }
        self.trail.pop();
    }

    /// Follows what the function `call` calls writes into its argument
    /// `index`.
    fn follow_into_callee(&mut self, want: &Want, call: Node, index: usize) {
        let Some(function) = call.child_by_field_name("function") else {
            return;
        };
        let Some(symbol) = callee(self.files.graph(), &want.file, function) else {
            return;
        };
        let returns_to = stepped_into(&want.returns_to, &want.file, call);
        let found = definitions(self.files.graph(), symbol, &want.file);
        for (def_file, line) in found.into_iter().take(MAX_DEFINITIONS) {
            let Some(tree) = self.files.tree(&def_file) else {
                continue;
            };
            let source = self.files.source(&def_file);
            let Some(callee_fn) = function_on_line(tree.root_node(), line) else {
                continue;
            };
            let Some(Some(param)) = parameters(callee_fn).get(index).copied() else {
                continue;
            };
            let Some(body) = callee_fn.child_by_field_name("body") else {
                continue;
            };
            self.trace(Want {
                is_through: true,
                field: want.field.clone(),
                ..Want::new(
                    &def_file,
                    text(param, source).to_string(),
                    // Inside the body, at its closing brace.
                    body.end_byte().saturating_sub(1),
                    &returns_to,
                )
            });
        }
    }

    fn call_sites_of(&mut self, symbol: SymbolId) -> Vec<CallSite> {
        self.call_sites.of(self.files.graph(), symbol)
    }
}

/// The values `function` returns, in source order.
fn returned_values(function: Node) -> Vec<Node> {
    let Some(body) = function.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "return_statement" => values.extend(
                crate::index::syntax::named_children(node)
                    .into_iter()
                    .find(|c| c.kind() != "comment"),
            ),
            // A nested function's returns aren't this one's.
            "lambda_expression" => {}
            _ => stack.extend(crate::index::syntax::named_children(node).into_iter().rev()),
        }
    }
    values
}

/// The write `node` is the target of, when it is: `name = …`,
/// `char *name = …`, `name[i] = …`, `for (auto name : …)`.
pub(super) fn set_by(node: Node) -> Option<Node> {
    let mut child = node;
    for _ in 0..8 {
        let parent = child.parent()?;
        let is_target = |field: &str| parent.child_by_field_name(field) == Some(child);
        match parent.kind() {
            "init_declarator" if is_target("declarator") => return Some(parent),
            "assignment_expression" if is_target("left") => return Some(parent),
            "for_range_loop" if is_target("declarator") => return Some(child),
            "pointer_declarator"
            | "array_declarator"
            | "reference_declarator"
            | "subscript_expression"
            | "field_expression"
            | "pointer_expression"
            | "parenthesized_declarator" => child = parent,
            _ => return None,
        }
    }
    None
}

/// What a write does to `name`, for its step.
fn def_note(def: &Def, name: &str, source: &str) -> String {
    let code = |node: Node| snippet(node, source);
    // Where it is written, as written: `buf[i]`, `state->path`, `&q`.
    let place = code(def.name);
    match &def.kind {
        DefKind::Set(value)
            if def
                .node
                .parent()
                .is_some_and(|p| p.kind() == "for_range_loop") =>
        {
            format!("`{name}` takes each item of `{}`", code(*value))
        }
        DefKind::Set(value) => format!("`{name}` set from `{}`", code(*value)),
        DefKind::Grows(value) => format!("adds `{}` to `{name}`", code(*value)),
        DefKind::Part(value) => format!("writes `{}` into `{place}`", code(*value)),
        DefKind::Declared => format!("`{name}` declared with no value"),
        DefKind::Copied { function, from } if from.is_empty() => {
            format!("`{function}()` clears `{place}`")
        }
        DefKind::Copied { function, from } => {
            let from: Vec<String> = from.iter().map(|n| format!("`{}`", code(*n))).collect();
            format!("`{function}()` copies {} into `{place}`", from.join(", "))
        }
        DefKind::Read(input) => format!("`{input}` reads outside input into `{place}`"),
        DefKind::Passed { call, .. } => {
            let called = call
                .child_by_field_name("function")
                .map_or_else(String::new, |f| called_name(f, source));
            format!("`{name}` passed to `{called}()`, which may write into it")
        }
    }
}
