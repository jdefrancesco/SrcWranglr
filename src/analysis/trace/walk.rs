//! The search behind a trace: from a value back through what sets it, a
//! depth-first walk that records a path each time it reaches an origin.

use std::collections::HashMap;

use tree_sitter::Node;

use super::calls::{library_name, risky_args};
use super::defs::{arguments, reaching, Def, DefKind};
use super::files::{
    call_at, call_name_node, call_named_at, call_starting_at, called_name, callee, definitions,
    function_around, function_name, function_on_line, may_call, parameters, snippet, step,
    symbol_of, variable_at, Files,
};
use super::flows::{carries_data, field_base, flows, name_of, Flow};
use super::{Origin, Step, Trace, TracePath};
use crate::analysis::reach::{input_source, COMMAND_LINE, MAIN_NAMES};
use crate::analysis::Finding;
use crate::index::syntax::{line_of, text};
use crate::model::{CodeGraph, EdgeKind, OccurrenceRole, Span, SymbolId};

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
}

/// A call, by where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CallRef {
    file: String,
    at: usize,
}

/// A place a function is called from: file, the name's span, and the
/// function the call is in.
type CallSite = (String, Span, Option<SymbolId>);

pub(super) struct Tracer<'g> {
    files: Files<'g>,
    /// Where each project function is called from, built when first needed.
    call_sites: Option<HashMap<SymbolId, Vec<CallSite>>>,
    /// The current path, from the subject back.
    trail: Vec<Step>,
    /// The values being traced along the current path, to stop cycles.
    wants: Vec<Want>,
    paths: Vec<TracePath>,
    work: usize,
    /// How deeply the walk is nested right now.
    depth: usize,
    is_cut_short: bool,
}

impl<'g> Tracer<'g> {
    pub(super) fn new(graph: &'g CodeGraph) -> Self {
        Self {
            files: Files::new(graph),
            call_sites: None,
            trail: Vec::new(),
            wants: Vec::new(),
            paths: Vec::new(),
            work: 0,
            depth: 0,
            is_cut_short: false,
        }
    }

    /// Traces the arguments that make `finding`'s call risky.
    pub(super) fn finding(mut self, finding: &Finding) -> Option<Trace> {
        let file = finding.file.as_str();
        let tree = self.files.tree(file)?;
        let source = self.files.source(file);
        let column = finding.columns.map_or(0, |(start, _)| start);
        let call = call_at(tree.root_node(), finding.line, column)?;
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
        self.trace(Want {
            file: file.to_string(),
            name: name.clone(),
            at,
            returns_to: Vec::new(),
            is_through: false,
        });
        Some(self.finish(format!("`{name}`"), file, line))
    }

    fn finish(self, subject: String, file: &str, line: u32) -> Trace {
        Trace {
            subject,
            file: file.to_string(),
            line,
            paths: super::ordered(self.paths),
            is_cut_short: self.is_cut_short,
        }
    }

    fn symbol_around(&self, root: Node, file: &str, at: usize) -> Option<SymbolId> {
        function_around(root, at).and_then(|f| symbol_of(self.files.graph(), file, f))
    }

    /// Records the current path as one that starts at `origin`.
    fn end(&mut self, origin: Origin) {
        if self.paths.len() >= MAX_PATHS {
            self.is_cut_short = true;
            return;
        }
        let steps = self.trail.iter().rev().cloned().collect();
        self.paths.push(TracePath { origin, steps });
    }

    fn stop(&mut self, why: String) {
        self.end(Origin::Stopped(why));
    }

    fn is_over_budget(&mut self) -> bool {
        let is_over = self.paths.len() >= MAX_PATHS || self.work >= MAX_WORK;
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
        let defs = reaching(
            function,
            source,
            &want.name,
            want.at,
            want.is_through,
            &is_project_call,
        );
        self.wants.push(want.clone());
        if defs.is_empty() {
            self.undefined(&want, function, source);
        } else {
            self.follow_defs(&want, function, source, &defs);
        }
        self.wants.pop();
    }

    fn follow_defs(&mut self, want: &Want, function: Node, source: &str, defs: &[Def]) {
        let symbol = symbol_of(self.files.graph(), &want.file, function);
        let is_all_placeholders = defs.iter().all(|d| d.is_placeholder(source));
        for def in defs {
            if def.is_placeholder(source) && !is_all_placeholders {
                continue;
            }
            let note = def_note(def, &want.name, source);
            self.trail
                .push(step(source, &want.file, def.name, symbol, note));
            self.follow_def(want, def);
            self.trail.pop();
        }
    }

    fn follow_def(&mut self, want: &Want, def: &Def) {
        let file = want.file.as_str();
        let name = &want.name;
        match &def.kind {
            DefKind::Set(value) => self.follow_value(file, *value, &want.returns_to),
            DefKind::Grows(value) | DefKind::Part(value) => {
                self.follow_value(file, *value, &want.returns_to);
                self.earlier(want, def);
            }
            DefKind::Declared => self.stop(format!("`{name}` has no value before this")),
            DefKind::Copied { from, .. } if from.is_empty() => {
                self.stop(format!("`{name}` is cleared here"));
            }
            DefKind::Copied { from, .. } => {
                for value in from {
                    self.follow_value(file, *value, &want.returns_to);
                }
                if !def.is_whole {
                    self.earlier(want, def);
                }
            }
            DefKind::Read(source) => self.end(Origin::Input(source.clone())),
            DefKind::Passed { call, index } => self.follow_into_callee(want, *call, *index),
        }
    }

    /// What set `want` before `def`, which only added to it.
    fn earlier(&mut self, want: &Want, def: &Def) {
        self.trace(Want {
            at: def.node.start_byte(),
            ..want.clone()
        });
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
                Flow::Var(node) => self.trace(Want {
                    file: file.to_string(),
                    name: name_of(node, source),
                    at: node.start_byte(),
                    returns_to: returns_to.to_vec(),
                    is_through: false,
                }),
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
            return self.end(Origin::Input(format!("{label}()")));
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
        let inner: Vec<CallRef> = returns_to
            .iter()
            .cloned()
            .chain([CallRef {
                file: file.to_string(),
                at: call.start_byte(),
            }])
            .collect();
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
        if let Some(base) = field_base(&want.name) {
            if base == "this" {
                return self.stop(format!(
                    "`{}` is a member, set elsewhere (not followed)",
                    want.name
                ));
            }
            return self.trace(Want {
                name: base.to_string(),
                ..want.clone()
            });
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
                self.end(Origin::Input(COMMAND_LINE.to_string()));
                self.trail.pop();
            }
            return;
        }
        if let Some((call, rest)) = want.returns_to.split_last() {
            return self.back_to_call(call, rest, index, &want.name);
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
                self.follow_argument(&file, call, index, &want.name, context, &[]);
            }
        }
    }

    /// Back out of the call stepped into, to its argument `index`.
    fn back_to_call(&mut self, call: &CallRef, rest: &[CallRef], index: usize, param: &str) {
        let Some(tree) = self.files.tree(&call.file) else {
            return;
        };
        let Some(node) = call_starting_at(tree.root_node(), call.at) else {
            return;
        };
        let symbol = self.symbol_around(tree.root_node(), &call.file, call.at);
        self.follow_argument(&call.file, node, index, param, symbol, rest);
    }

    /// Follows argument `index` of `call`, passed as `param`.
    fn follow_argument(
        &mut self,
        file: &str,
        call: Node,
        index: usize,
        param: &str,
        symbol: Option<SymbolId>,
        returns_to: &[CallRef],
    ) {
        let Some(arg) = arguments(call).get(index).copied() else {
            return;
        };
        let source = self.files.source(file);
        let note = format!("passes `{}` as `{param}`", snippet(arg, source));
        self.trail.push(step(source, file, arg, symbol, note));
        self.follow_value(file, arg, returns_to);
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
        let returns_to: Vec<CallRef> = want
            .returns_to
            .iter()
            .cloned()
            .chain([CallRef {
                file: want.file.clone(),
                at: call.start_byte(),
            }])
            .collect();
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
                file: def_file.clone(),
                name: text(param, source).to_string(),
                // Inside the body, at its closing brace.
                at: body.end_byte().saturating_sub(1),
                returns_to: returns_to.clone(),
                is_through: true,
            });
        }
    }

    fn call_sites_of(&mut self, symbol: SymbolId) -> Vec<CallSite> {
        let graph = self.files.graph();
        self.call_sites
            .get_or_insert_with(|| call_sites(graph))
            .get(&symbol)
            .cloned()
            .unwrap_or_default()
    }
}

/// Where every project function is called from, files in name order.
fn call_sites(graph: &CodeGraph) -> HashMap<SymbolId, Vec<CallSite>> {
    let mut files: Vec<&String> = graph.occurrences.keys().collect();
    files.sort_unstable();
    let mut sites: HashMap<SymbolId, Vec<CallSite>> = HashMap::new();
    for file in files {
        let calls = graph.occurrences[file]
            .iter()
            .filter(|o| o.role == OccurrenceRole::Reference(EdgeKind::Calls));
        for occurrence in calls {
            sites.entry(occurrence.symbol).or_default().push((
                file.clone(),
                occurrence.span,
                occurrence.context,
            ));
        }
    }
    sites
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
fn set_by(node: Node) -> Option<Node> {
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
        DefKind::Part(value) => format!("writes `{}` into `{name}`", code(*value)),
        DefKind::Declared => format!("`{name}` declared with no value"),
        DefKind::Copied { function, from } if from.is_empty() => {
            format!("`{function}()` clears `{name}`")
        }
        DefKind::Copied { function, from } => {
            let from: Vec<String> = from.iter().map(|n| format!("`{}`", code(*n))).collect();
            format!("`{function}()` copies {} into `{name}`", from.join(", "))
        }
        DefKind::Read(input) => format!("`{input}` reads outside input into `{name}`"),
        DefKind::Passed { call, .. } => {
            let called = call
                .child_by_field_name("function")
                .map_or_else(String::new, |f| called_name(f, source));
            format!("`{name}` passed to `{called}()`, which may write into it")
        }
    }
}
