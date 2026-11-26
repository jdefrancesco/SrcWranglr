//! A small concrete emulator for one C function: set the unknown input sizes
//! (parameters and values like `strlen(name)` the engine can't compute), then
//! it runs the body statement by statement over those values — taking the real
//! branches, modeling allocations as sized buffers, and bounds-checking each
//! copy. Any write that runs past its buffer is flagged (CWE-787). It answers
//! "do these writes fit the buffer for these inputs", not full C semantics;
//! see the boundaries in the engine files.

mod expr;
mod machine;
mod names;
mod value;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::analysis::parse::{cpp_parser, function_containing, text};
use crate::model::CodeGraph;

/// A value the user sets before running: a function parameter, or an integer
/// the engine couldn't compute (an unmodeled call like `strlen(name)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub value: i128,
}

/// A write that ran past the end of its buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overflow {
    /// The buffer written past (its label).
    pub buffer: String,
    /// The byte one past the last written (`offset + bytes`).
    pub end: i128,
    /// The buffer's size in bytes.
    pub size: i128,
}

/// One executed statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// 1-based source line.
    pub line: u32,
    /// What happened, e.g. `entry = malloc(17)` or `memcpy(…) writes 6 bytes`.
    pub text: String,
    /// Set when this step's write overflowed its buffer.
    pub overflow: Option<Overflow>,
}

/// The result of emulating a function with a set of inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub function: String,
    /// 1-based first and last line of the function.
    pub first_line: u32,
    pub last_line: u32,
    /// The inputs the run used, in first-seen order.
    pub inputs: Vec<Input>,
    /// One step per executed statement, in order.
    pub steps: Vec<Step>,
}

impl Trace {
    /// The first step whose write overflowed, if any.
    pub fn first_overflow(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.overflow.is_some())
    }
}

/// Emulates the function holding `file:line` with `given` inputs (name →
/// value; unset default to 0). `None` if there's no such function or it won't
/// parse.
pub fn run(
    graph: &CodeGraph,
    file: &str,
    line: u32,
    given: &HashMap<String, i128>,
) -> Option<Trace> {
    let source = graph.file_contents.get(file)?;
    let mut parser = cpp_parser()?;
    let tree = parser.parse(source, None)?;
    let function = function_containing(tree.root_node(), line)?;
    if function.has_error() {
        return None;
    }
    let body = function.child_by_field_name("body")?;
    let name = expr::declared_name(function)
        .map_or_else(|| "function".to_string(), |n| text(source, n).to_string());

    let mut machine = machine::Machine::new(given);
    machine.setup_params(function, source);
    machine.run_block(body, source);

    Some(Trace {
        function: name,
        first_line: function.start_position().row as u32 + 1,
        last_line: function.end_position().row as u32 + 1,
        inputs: machine
            .inputs
            .iter()
            .map(|(name, value)| Input {
                name: name.clone(),
                value: *value,
            })
            .collect(),
        steps: machine.steps,
    })
}
