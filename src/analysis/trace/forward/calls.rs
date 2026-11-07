//! Following a value across calls: into the function it is handed to,
//! back out of the one it is returned from, and out through a parameter a
//! function writes it into.

use tree_sitter::{Node, Tree};

use super::{At, Follower, Held, MAX_CALLERS, MAX_CALL_DEPTH, MAX_DEFINITIONS};
use crate::analysis::reach::MAIN_NAMES;
use crate::analysis::trace::defs::arguments;
use crate::analysis::trace::files::{
    call_name_node, call_named_at, call_starting_at, called_name, definitions, function_around,
    function_name, function_on_line, may_call, parameters, snippet, symbol_of,
};
use crate::analysis::trace::flows::access_path;
use crate::analysis::trace::sites::{stepped_into, CallRef};
use crate::index::syntax::{line_of, text};
use crate::model::{Span, SymbolId};

impl Follower<'_> {
    /// Follows the value into the parameter `index` of each definition of
    /// the project function `call` calls. Whether there was one.
    pub(super) fn follow_into_callee(
        &mut self,
        at: At,
        value: Node,
        call: Node,
        symbol: SymbolId,
        index: usize,
    ) -> bool {
        if at.returns_to.len() >= MAX_CALL_DEPTH {
            self.give_up();
            return true;
        }
        let called = call
            .child_by_field_name("function")
            .map_or_else(String::new, |f| called_name(f, at.source));
        let returns_to = stepped_into(at.returns_to, at.file, call);
        let mut is_followed = false;
        let found = definitions(self.files.graph(), symbol, at.file);
        for (file, line) in found.into_iter().take(MAX_DEFINITIONS) {
            let Some((name, body_start)) = self.parameter_of(&file, line, index) else {
                continue;
            };
            is_followed = true;
            let shown = snippet(value, at.source);
            self.push(
                &at,
                format!("`{shown}` is passed to `{called}()` as `{name}`"),
            );
            self.follow(Held {
                file,
                name,
                field: at.rest.to_vec(),
                from: body_start,
                writes: Vec::new(),
                is_from_before: true,
                returns_to: returns_to.clone(),
            });
            self.trail.pop();
        }
        is_followed
    }

    /// The name of parameter `index` of the function defined at `line` of
    /// `file`, and where its body starts.
    fn parameter_of(&mut self, file: &str, line: u32, index: usize) -> Option<(String, usize)> {
        let tree = self.files.tree(file)?;
        let source = self.files.source(file);
        let function = function_on_line(tree.root_node(), line)?;
        let param = parameters(function).get(index).copied().flatten()?;
        let body = function.child_by_field_name("body")?;
        Some((text(param, source).to_string(), body.start_byte()))
    }

    /// Follows the value `value` returned: back to the call stepped in
    /// from, or (none) to every call of the function.
    pub(super) fn returned(&mut self, at: At, value: Node) {
        let name = function_name(at.function, at.source);
        if MAIN_NAMES.contains(&name.as_str()) {
            return;
        }
        // Back up to every caller from a function already on the way up
        // there: a recursion, which brings nothing new.
        let function = (at.file.to_string(), at.function.start_byte());
        let is_to_every_caller = at.returns_to.is_empty();
        if is_to_every_caller && self.returning.contains(&function) {
            return;
        }
        let note = format!("`{name}()` returns `{}`", snippet(value, at.source));
        self.push(&at, note);
        if is_to_every_caller {
            self.returning.push(function);
        }
        self.at_callers(&at, &|this, next, call| {
            let next = At {
                rest: at.rest,
                ..next
            };
            this.goes_on(next, call);
        });
        if is_to_every_caller {
            self.returning.pop();
        }
        self.trail.pop();
    }

    /// The parameter `index` of the function at `at` was written through
    /// (into its `fields`): the caller's argument gets the value.
    pub(super) fn out_through(&mut self, at: At, index: usize, fields: &[String]) {
        self.at_callers(&at, &|this, next, call| {
            this.follow_into_argument(next, call, index, fields);
        });
    }

    /// Runs `then` at each call the function at `at` returns to: the one
    /// stepped in from, or (none) every call of it.
    fn at_callers(&mut self, at: &At, then: &dyn Fn(&mut Self, At, Node)) {
        match at.returns_to.split_last() {
            Some((call, outer)) => {
                let source = self.files.source(&call.file);
                if let Some(tree) = self.files.tree(&call.file) {
                    self.at_call(&tree, &call.file, source, call.at, outer, then);
                }
            }
            None => {
                let name = function_name(at.function, at.source);
                for (file, span) in self.callers(at, &name) {
                    let Some(tree) = self.files.tree(&file) else {
                        continue;
                    };
                    let source = self.files.source(&file);
                    let root = tree.root_node();
                    if let Some(call) = call_named_at(root, span.line, span.start) {
                        self.at_call(&tree, &file, source, call.start_byte(), &[], then);
                    }
                }
            }
        }
    }

    /// Runs `then` at the call starting at byte `call_at` of `file`.
    fn at_call(
        &mut self,
        tree: &Tree,
        file: &str,
        source: &str,
        call_at: usize,
        returns_to: &[CallRef],
        then: &dyn Fn(&mut Self, At, Node),
    ) {
        let root = tree.root_node();
        let Some(call) = call_starting_at(root, call_at) else {
            return;
        };
        let Some(function) = function_around(root, call_at) else {
            return;
        };
        let found = call
            .child_by_field_name("function")
            .map_or(call, call_name_node);
        let next = At {
            file,
            source,
            function,
            returns_to,
            found,
            rest: &[],
        };
        then(self, next, call);
    }

    /// Where the function at `at` (named `name`) is called from, up to
    /// `MAX_CALLERS`; a stop says so when nowhere is known.
    fn callers(&mut self, at: &At, name: &str) -> Vec<(String, Span)> {
        let graph = self.files.graph();
        let Some(symbol) = symbol_of(graph, at.file, at.function) else {
            self.stop(format!("returned by `{name}()`, which isn't indexed"));
            return Vec::new();
        };
        let definition = (at.file, line_of(at.function));
        let sites: Vec<(String, Span)> = self
            .call_sites
            .of(graph, symbol)
            .into_iter()
            .filter(|(file, _, _)| may_call(graph, symbol, definition, file))
            .map(|(file, span, _)| (file, span))
            .collect();
        if sites.is_empty() {
            self.stop(format!(
                "returned by `{name}()`, which nothing indexed calls"
            ));
        }
        self.is_cut_short |= sites.len() > MAX_CALLERS;
        sites.into_iter().take(MAX_CALLERS).collect()
    }

    /// The function `call` calls wrote the value into its parameter
    /// `index` (into its `fields`): follows the argument passed there.
    fn follow_into_argument(&mut self, at: At, call: Node, index: usize, fields: &[String]) {
        let Some(arg) = arguments(call).get(index).copied() else {
            return;
        };
        let (base, arg_fields) = access_path(arg, at.source);
        if base.kind() != "identifier" {
            return;
        }
        let called = call
            .child_by_field_name("function")
            .map_or_else(String::new, |f| called_name(f, at.source));
        let next = At { found: arg, ..at };
        let note = format!("`{called}()` writes it into `{}`", snippet(arg, at.source));
        self.push(&next, note);
        let field = arg_fields
            .into_iter()
            .chain(fields.iter().cloned())
            .collect();
        let name = text(base, at.source).to_string();
        self.store(next, name, field, call, call.end_byte());
        self.trail.pop();
    }
}
