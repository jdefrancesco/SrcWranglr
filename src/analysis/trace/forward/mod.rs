//! The search behind a forward trace: from a value to the places it is
//! used, through what it is stored in, copied into, handed to and returned
//! from, recording a path each time it reaches a risky call.

mod calls;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tree_sitter::Node;

use super::calls::{library_name, method_write, risky_args, write_of};
use super::defs::{
    arguments, is_own, is_to_field, reaching, replaces_field, Def, DefKind, Query, Reaching, Writes,
};
use super::files::{
    call_name_node, called_name, callee, function_around, function_name, parameters, snippet, step,
    symbol_of, variable_at, Files,
};
use super::flows::{access_path, field_base, fields_of, name_of};
use super::goes::{goes, through_parameter, uses_of, Goes, Use};
use super::scope::{contains, loop_around};
use super::sites::{CallRef, CallSites};
use super::walk::set_by;
use super::{Direction, Endpoint, Sink, Step, Trace, TracePath};
use crate::analysis::reach::MAIN_NAMES;
use crate::index::syntax::{line_of, text};
use crate::model::CodeGraph;

/// Paths to risky calls kept per trace.
const MAX_PATHS: usize = 24;
/// Places the value stops being followed kept per trace, each once.
const MAX_STOPS: usize = 8;
/// Values followed per trace, so a huge call graph can't stall it.
const MAX_WORK: usize = 2_000;
/// Steps in one path.
const MAX_STEPS: usize = 40;
/// Callers a result goes back to, and definitions per called function.
const MAX_CALLERS: usize = 8;
const MAX_DEFINITIONS: usize = 4;
/// Calls stepped into, one inside another.
const MAX_CALL_DEPTH: usize = 8;
/// Uses of one value followed.
const MAX_USES: usize = 64;
/// How long one trace may take before it stops with what it has: it runs
/// while the window waits.
const MAX_TIME: Duration = Duration::from_millis(750);
/// How deeply the walk may nest before it stops: keeps the stack bounded.
const MAX_DEPTH: usize = 64;
/// Times one variable (or field) is followed, from any write and by any
/// route: each risky call it reaches is found on the first, and more show
/// other ways there. Values that go round between variables in a loop
/// (`from = out - d; *out++ = *from++;`) would otherwise be followed once
/// per write.
const MAX_VISITS: u32 = 3;
/// Wrappers looked through from a name to the call it is handed to.
const MAX_UNWRAP: usize = 8;

/// A write, by where it is: the node that writes, and the name written.
type WriteAt = (usize, usize, usize);

fn write_at(def: &Def) -> WriteAt {
    (
        def.node.start_byte(),
        def.node.end_byte(),
        def.name.start_byte(),
    )
}

/// A value followed: what the variable `name` (or `field` of it) holds in
/// `file` from byte `from` on.
#[derive(Debug, Clone)]
struct Held {
    file: String,
    name: String,
    field: Vec<String>,
    from: usize,
    /// The writes that put the value there.
    writes: Vec<WriteAt>,
    /// It is the value the variable had at `from` (a parameter's, or a
    /// use's), so a use no write is certain to come before still has it.
    is_from_before: bool,
    /// Calls stepped into, innermost last: a `return` goes back to the
    /// innermost one, not to every caller.
    returns_to: Vec<CallRef>,
}

/// The variable (or field of it) a value is in, by the file and start of
/// its function, and the calls stepped into: what visits are counted by.
type HeldKey = (String, usize, String, Vec<String>, Vec<CallRef>);

impl Held {
    /// What its visits are counted by, in the function starting at byte
    /// `function`.
    fn key(&self, function: usize) -> HeldKey {
        (
            self.file.clone(),
            function,
            self.name.clone(),
            self.field.clone(),
            self.returns_to.clone(),
        )
    }

    /// Whether it is `other`, in the same calls (so a loop ends; a
    /// recursion ends at `MAX_CALL_DEPTH`).
    fn is_same(&self, other: &Held) -> bool {
        (&self.file, &self.name, &self.field, self.from, &self.writes)
            == (
                &other.file,
                &other.name,
                &other.field,
                other.from,
                &other.writes,
            )
            && self.is_from_before == other.is_from_before
            && self.returns_to == other.returns_to
    }
}

/// Where the value followed was found, and what is around it.
#[derive(Clone, Copy)]
struct At<'a, 't> {
    file: &'a str,
    source: &'a str,
    function: Node<'t>,
    returns_to: &'a [CallRef],
    /// Where the value is: what the step there marks.
    found: Node<'t>,
    /// The fields of the struct found that hold the value (empty: all).
    rest: &'a [String],
}

pub(super) struct Follower<'g> {
    files: Files<'g>,
    /// Risky calls by file, then line.
    sinks: HashMap<&'g str, HashMap<u32, Vec<&'g Sink>>>,
    call_sites: CallSites,
    /// The current path, from where it starts.
    trail: Vec<Step>,
    /// The values followed along the current path, to stop cycles.
    held: Vec<Held>,
    /// How many times each variable has been followed, by any route.
    visits: HashMap<HeldKey, u32>,
    /// Functions whose value is being returned to every caller, on the
    /// current path: going round again (recursion) ends it.
    returning: Vec<(String, usize)>,
    /// Paths to risky calls.
    paths: Vec<TracePath>,
    /// Paths to where the value stops being followed.
    stops: Vec<TracePath>,
    work: usize,
    /// When the trace stops with what it has.
    deadline: Instant,
    /// How deeply the walk is nested right now.
    depth: usize,
    is_cut_short: bool,
}

impl<'g> Follower<'g> {
    pub(super) fn new(graph: &'g CodeGraph, sinks: &'g [Sink]) -> Self {
        let mut by_file: HashMap<&str, HashMap<u32, Vec<&Sink>>> = HashMap::new();
        for sink in sinks {
            by_file
                .entry(sink.file.as_str())
                .or_default()
                .entry(sink.line)
                .or_default()
                .push(sink);
        }
        Self {
            files: Files::new(graph),
            sinks: by_file,
            call_sites: CallSites::default(),
            trail: Vec::new(),
            held: Vec::new(),
            visits: HashMap::new(),
            returning: Vec::new(),
            paths: Vec::new(),
            stops: Vec::new(),
            work: 0,
            deadline: Instant::now() + MAX_TIME,
            depth: 0,
            is_cut_short: false,
        }
    }

    /// Follows the variable written at `line`, `column` of `file`.
    pub(super) fn variable(mut self, file: &str, line: u32, column: u32) -> Option<Trace> {
        let tree = self.files.tree(file)?;
        let source = self.files.source(file);
        let node = variable_at(tree.root_node(), line, column)?;
        let function = function_around(tree.root_node(), node.start_byte())?;
        let written = name_of(node, source);
        let held = Held {
            file: file.to_string(),
            name: field_base(&written).unwrap_or(&written).to_string(),
            field: fields_of(&written),
            from: node.start_byte(),
            writes: Vec::new(),
            is_from_before: true,
            returns_to: Vec::new(),
        };
        let (held, note) = self.start(held, function, source, node, &written);
        let symbol = symbol_of(self.files.graph(), file, function);
        self.trail.push(step(source, file, node, symbol, note));
        self.follow(held);
        Some(Trace {
            subject: format!("`{written}`"),
            file: file.to_string(),
            line,
            direction: Direction::Forward,
            paths: super::ordered(self.paths.into_iter().chain(self.stops).collect()),
            is_cut_short: self.is_cut_short,
        })
    }

    /// The value `node` holds where it was picked, and the first step's
    /// note: a parameter's coming in, a write's, or the value at a use.
    fn start(
        &mut self,
        held: Held,
        function: Node,
        source: &str,
        node: Node,
        written: &str,
    ) -> (Held, String) {
        if parameters(function).contains(&Some(node)) {
            let from = function
                .child_by_field_name("body")
                .map_or(node.end_byte(), |b| b.start_byte());
            let is_main = MAIN_NAMES.contains(&function_name(function, source).as_str());
            let note = match is_main {
                true => format!("`{written}` is the command line"),
                false => format!("`{written}` comes in here"),
            };
            return (Held { from, ..held }, note);
        }
        if let Some((from, writes)) = self.set_at(&held, function, source, node) {
            let set = Held {
                from,
                writes,
                is_from_before: false,
                ..held
            };
            return (set, format!("`{written}` is set here"));
        }
        let graph = self.files.graph();
        let is_project_call = |f: Node| callee(graph, &held.file, f).is_some();
        let writes = Writes::of(function, source, &held.name, &is_project_call);
        let found = self.reaching_at(&held, function, source, node.start_byte(), Some(&writes));
        self.is_cut_short |= found.is_truncated;
        let at_use = Held {
            writes: found.defs.iter().map(write_at).collect(),
            is_from_before: !found.is_complete,
            ..held
        };
        (at_use, format!("`{written}` here"))
    }

    /// Where `node` is set, when it is picked where it is: the target of
    /// an assignment or declaration, or a buffer a library call fills
    /// (`fgets(buf, …)`), not one merely handed to a function. Where the
    /// new value counts from, and the writes.
    fn set_at(
        &self,
        held: &Held,
        function: Node,
        source: &str,
        node: Node,
    ) -> Option<(usize, Vec<WriteAt>)> {
        let (write, is_assignment) = match set_by(node) {
            Some(write) => (write, true),
            None => (call_handed(node)?, false),
        };
        let is_filled = |d: &Def| matches!(d.kind, DefKind::Read(_) | DefKind::Copied { .. });
        let writes: Vec<WriteAt> = self
            .reaching_at(held, function, source, write.end_byte(), None)
            .defs
            .iter()
            .filter(|d| is_inside(write, d) && (is_assignment || is_filled(d)))
            .map(write_at)
            .collect();
        (!writes.is_empty()).then_some((write.end_byte(), writes))
    }

    /// Records the current path to a risky call.
    fn record(&mut self, endpoint: Endpoint) {
        if self.paths.len() >= MAX_PATHS {
            self.is_cut_short = true;
            return;
        }
        let steps = self.trail.clone();
        self.paths.push(TracePath { endpoint, steps });
    }

    /// Records that the value stops being followed here, and why: once
    /// for each place and reason.
    fn stop(&mut self, why: String) {
        let endpoint = Endpoint::Stopped(why);
        let place = |steps: &[Step]| steps.last().map(|s| (s.file.clone(), s.line));
        let here = place(&self.trail);
        let is_known = self
            .stops
            .iter()
            .any(|p| p.endpoint == endpoint && place(&p.steps) == here);
        if !is_known && self.stops.len() < MAX_STOPS {
            let steps = self.trail.clone();
            self.stops.push(TracePath { endpoint, steps });
        }
    }

    /// A limit stops the walk here: there may be more paths.
    fn give_up(&mut self) {
        self.is_cut_short = true;
    }

    /// Adds a step at where `at` found the value.
    fn push(&mut self, at: &At, note: String) {
        let symbol = symbol_of(self.files.graph(), at.file, at.function);
        self.trail
            .push(step(at.source, at.file, at.found, symbol, note));
    }

    fn is_over_budget(&mut self) -> bool {
        let is_over = self.paths.len() >= MAX_PATHS
            || self.work >= MAX_WORK
            || Instant::now() >= self.deadline;
        self.is_cut_short |= is_over;
        is_over
    }

    /// Follows where `held` goes after it is set.
    fn follow(&mut self, held: Held) {
        if self.depth >= MAX_DEPTH {
            return self.give_up();
        }
        self.depth += 1;
        self.follow_here(held);
        self.depth -= 1;
    }

    fn follow_here(&mut self, held: Held) {
        if self.is_over_budget() || self.held.iter().any(|h| h.is_same(&held)) {
            return;
        }
        if self.trail.len() >= MAX_STEPS {
            return self.give_up();
        }
        self.work += 1;
        let Some(tree) = self.files.tree(&held.file) else {
            return;
        };
        let source = self.files.source(&held.file);
        let Some(function) = function_around(tree.root_node(), held.from) else {
            return;
        };
        let visits = self
            .visits
            .entry(held.key(function.start_byte()))
            .or_default();
        if *visits >= MAX_VISITS {
            return self.give_up();
        }
        *visits += 1;
        let uses = self.uses_after(function, source, &held);
        let graph = self.files.graph();
        let is_project_call = |f: Node| callee(graph, &held.file, f).is_some();
        let writes = Writes::of(function, source, &held.name, &is_project_call);
        self.held.push(held.clone());
        for found in &uses {
            let Some(rest) = rest_of(&held.field, &found.fields) else {
                continue;
            };
            if self.is_over_budget() {
                break;
            }
            if !self.reaches(&held, (function, source), found.name, &writes) {
                continue;
            }
            let at = At {
                file: &held.file,
                source,
                function,
                returns_to: &held.returns_to,
                found: found.access,
                rest: &rest,
            };
            self.goes_on(at, found.access);
        }
        self.held.pop();
    }

    /// The uses of `held` in `function` that may see it: after it is set,
    /// or earlier in a loop around that (which brings it back to them), up
    /// to `MAX_USES`.
    fn uses_after<'t>(&mut self, function: Node<'t>, source: &str, held: &Held) -> Vec<Use<'t>> {
        let (all, is_partial) = uses_of(function, source, &held.name);
        let around = loop_around(function, held.from, None);
        let mut uses: Vec<Use> = all
            .into_iter()
            .filter(|u| {
                let at = u.name.start_byte();
                at >= held.from || around.is_some_and(|l| contains(l, at))
            })
            .collect();
        self.is_cut_short |= is_partial || uses.len() > MAX_USES;
        uses.truncate(MAX_USES);
        uses
    }

    /// Whether `held` is still the value at the use `name` (in `function`
    /// of `source`, whose `writes` to it are known): one of its writes
    /// reaches it, or (a value from before) nothing replaces it. Too many
    /// writes to tell count as reaching it.
    fn reaches<'t>(
        &self,
        held: &Held,
        (function, source): (Node<'t>, &str),
        name: Node<'t>,
        writes: &Writes<'t>,
    ) -> bool {
        let found = self.reaching_at(held, function, source, name.start_byte(), Some(writes));
        found.is_truncated
            || found
                .defs
                .iter()
                .any(|d| held.writes.contains(&write_at(d)))
            || (held.is_from_before && !found.is_complete)
    }

    /// The writes to `held` that reach byte `at` of `function` (before it
    /// only, unless `writes` is given: a use, which a loop brings later
    /// writes back to), from `writes` when they are known.
    fn reaching_at<'t>(
        &self,
        held: &Held,
        function: Node<'t>,
        source: &str,
        at: usize,
        writes: Option<&Writes<'t>>,
    ) -> Reaching<'t> {
        let graph = self.files.graph();
        let file = held.file.as_str();
        let is_project_call = |f: Node<'t>| callee(graph, file, f).is_some();
        let keeps = |d: &Def<'t>| is_to_field(&held.field, d, source);
        let replaces = |d: &Def<'t>| replaces_field(&held.field, d, source);
        let query = Query {
            name: &held.name,
            at,
            is_through: false,
            is_project_call: &is_project_call,
            keeps: &keeps,
            replaces: &replaces,
            has_later: writes.is_some(),
        };
        match writes {
            Some(writes) => writes.reaching(function, source, &query),
            None => reaching(function, source, &query),
        }
    }

    /// The writes `write` makes to `held`'s variable, as seen from `at`.
    fn writes_in(
        &self,
        held: &Held,
        function: Node,
        source: &str,
        write: Node,
        at: usize,
    ) -> Vec<WriteAt> {
        self.reaching_at(held, function, source, at, None)
            .defs
            .iter()
            .filter(|d| is_inside(write, d))
            .map(write_at)
            .collect()
    }

    /// Follows the value at `from` (found at `at`) to where it ends up.
    fn goes_on(&mut self, at: At, from: Node) {
        if self.depth >= MAX_DEPTH {
            return self.give_up();
        }
        if self.is_over_budget() {
            return;
        }
        self.depth += 1;
        self.goes_on_here(at, from);
        self.depth -= 1;
    }

    fn goes_on_here(&mut self, at: At, from: Node) {
        let (end, value) = goes(from, at.source);
        match end {
            Goes::Stored {
                write,
                target,
                from,
            } => {
                let note = format!(
                    "`{}` is stored in `{}`",
                    snippet(value, at.source),
                    snippet(target, at.source)
                );
                self.push(&at, note);
                self.keep_in(at, target, write, from, false);
                self.trail.pop();
            }
            Goes::Argument { call, index } => self.argument(at, value, call, index),
            Goes::Returned => self.returned(at, value),
            Goes::Used => {}
        }
    }

    /// Follows the value into `target`, which `write` sets from byte
    /// `from` on; out through a parameter too, when that is what it writes.
    fn keep_in(&mut self, at: At, target: Node, write: Node, from: usize, is_into_pointee: bool) {
        let (base, fields) = access_path(target, at.source);
        let shown = snippet(target, at.source);
        if base.kind() != "identifier" {
            let what = match base.kind() {
                "this" => "a member, used elsewhere",
                _ => "not followed",
            };
            return self.stop(format!("stored in `{shown}` ({what})"));
        }
        let name = text(base, at.source).to_string();
        if !is_own(at.function, at.source, &name, from) {
            // Its uses elsewhere aren't followed; its uses here still are.
            self.stop(format!(
                "stored in `{shown}`, a global or member, used elsewhere (not followed)"
            ));
        }
        let field: Vec<String> = fields.iter().chain(at.rest).cloned().collect();
        self.store(at, name, field, write, from);
        let parameters = parameters(at.function);
        if let Some(index) = through_parameter(&parameters, at.source, target, is_into_pointee) {
            self.out_through(at, index, &fields);
        }
    }

    /// Follows `field` of the variable `name` from `write` on.
    fn store(&mut self, at: At, name: String, field: Vec<String>, write: Node, from: usize) {
        let held = Held {
            file: at.file.to_string(),
            name,
            field,
            from,
            writes: Vec::new(),
            is_from_before: false,
            returns_to: at.returns_to.to_vec(),
        };
        let writes = self.writes_in(&held, at.function, at.source, write, from);
        if writes.is_empty() {
            let name = &held.name;
            return self.stop(format!("stored in `{name}` in a way this can't follow"));
        }
        self.follow(Held { writes, ..held });
    }

    /// The value `value` is argument `index` of `call`: a risky call it
    /// reaches, what the call copies it into, the function it goes into,
    /// or (any other call) what the call returns.
    fn argument(&mut self, at: At, value: Node, call: Node, index: usize) {
        let Some(function) = call.child_by_field_name("function") else {
            return;
        };
        let called = called_name(function, at.source);
        let count = arguments(call).len();
        if risky_args(library_name(&called), count).contains(&index) {
            if let Some(sink) = self.sink_at(at.file, call) {
                let note = format!(
                    "`{}` reaches `{called}()`, argument {}",
                    snippet(value, at.source),
                    index + 1
                );
                self.push(&at, note);
                self.record(Endpoint::Sink(format!("`{called}()` ({})", sink.detail)));
                self.trail.pop();
            }
        }
        let is_copied = self.copied(at, value, call, &called, index);
        if !is_copied {
            if let Some(symbol) = callee(self.files.graph(), at.file, function) {
                if self.follow_into_callee(at, value, call, symbol, index) {
                    return;
                }
            }
        }
        match function.kind() {
            // What it returns may be made from it: `strlen(s)`, `strdup(s)`,
            // and a copy returns its destination.
            "identifier" | "qualified_identifier" | "template_function" | "field_expression" => {
                self.goes_on(at, call);
            }
            _ if is_copied => {}
            _ => self.stop("passed to a call through a function pointer (not followed)".into()),
        }
    }

    /// The risky call `call` is, if a finding is there.
    fn sink_at(&self, file: &str, call: Node) -> Option<&'g Sink> {
        let function = call.child_by_field_name("function")?;
        let name = call_name_node(function);
        let (first, last) = (call.start_position(), name.end_position());
        let columns = match first.row == last.row {
            true => first.column..=last.column,
            false => 0..=last.column,
        };
        let on_line = self.sinks.get(file)?.get(&line_of(name))?;
        on_line.iter().copied().find(|s| {
            s.column
                .is_none_or(|c| usize::try_from(c).is_ok_and(|c| columns.contains(&c)))
        })
    }

    /// Follows the value into what `call` copies argument `index` into:
    /// `strcpy`'s destination, `s.append(x)`'s object. Whether it does.
    fn copied(&mut self, at: At, value: Node, call: Node, called: &str, index: usize) -> bool {
        let Some(function) = call.child_by_field_name("function") else {
            return false;
        };
        let args = arguments(call);
        let into: Vec<Node> = match function.kind() {
            "field_expression" if method_write(called).is_some() => function
                .child_by_field_name("argument")
                .into_iter()
                .collect(),
            "field_expression" => return false,
            _ => match write_of(library_name(called)) {
                Some(write) if write.from.indices(args.len()).contains(&index) => {
                    let into = write.into.indices(args.len());
                    into.into_iter()
                        .filter_map(|i| args.get(i).copied())
                        .collect()
                }
                _ => return false,
            },
        };
        if into.is_empty() {
            return false;
        }
        for target in into {
            let note = format!(
                "`{called}()` copies `{}` into `{}`",
                snippet(value, at.source),
                snippet(target, at.source)
            );
            self.push(&at, note);
            self.keep_in(at, target, call, call.end_byte(), true);
            self.trail.pop();
        }
        true
    }
}

/// Whether `def` is made by `write`.
fn is_inside(write: Node, def: &Def) -> bool {
    write.start_byte() <= def.node.start_byte() && def.node.end_byte() <= write.end_byte()
}

/// What of the value a use of `fields` of the variable holds, when the
/// value is in `field` of it: the fields still to go down to it (`None`:
/// the use is of another field).
fn rest_of(field: &[String], fields: &[String]) -> Option<Vec<String>> {
    if fields.starts_with(field) {
        Some(Vec::new())
    } else if field.starts_with(fields) {
        Some(field[fields.len()..].to_vec())
    } else {
        None
    }
}

/// The call `node` is handed to as an argument (through `&`, a cast, …).
fn call_handed(node: Node) -> Option<Node> {
    let mut child = node;
    for _ in 0..MAX_UNWRAP {
        let parent = child.parent()?;
        match parent.kind() {
            "argument_list" => return parent.parent().filter(|c| c.kind() == "call_expression"),
            "pointer_expression"
            | "cast_expression"
            | "parenthesized_expression"
            | "field_expression"
            | "subscript_expression" => child = parent,
            _ => return None,
        }
    }
    None
}
