//! Tracing a value back to where it comes from: from a risky call's
//! argument (or any variable you pick) to the assignments, copies and calls
//! that set it, across functions through parameters, return values and the
//! buffers a function fills, until outside input (the command line,
//! `fgets`, `getenv`, …) or a stop it can name.
//!
//! It reads each function's syntax with tree-sitter and knows no types or
//! aliases: a write through another pointer to the same memory, a call
//! through a function pointer, or a check that limits the value isn't seen.
//! So a path means the value *may* come from there, and no path means none
//! was found.

mod calls;
mod defs;
mod files;
mod flows;
mod walk;

use crate::analysis::Finding;
use crate::model::{CodeGraph, Span, SymbolId};

/// Where a traced value comes from, and every step on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// What was traced: the risky call, or `` `name` ``.
    pub subject: String,
    pub file: String,
    pub line: u32,
    /// Paths from outside input first, then shorter ones.
    pub paths: Vec<TracePath>,
    /// A limit was hit, so there may be more paths than these.
    pub is_cut_short: bool,
}

/// One way the value gets to the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracePath {
    pub origin: Origin,
    /// From the origin to the subject.
    pub steps: Vec<Step>,
}

/// Where a path starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Outside input, e.g. `the command line` or `fgets()`.
    Input(String),
    /// Where the trace stopped, and why.
    Stopped(String),
}

impl Origin {
    pub fn is_input(&self) -> bool {
        matches!(self, Self::Input(_))
    }

    /// "from fgets()", or "stops: …".
    pub fn label(&self) -> String {
        match self {
            Self::Input(source) => format!("from {source}"),
            Self::Stopped(why) => format!("stops: {why}"),
        }
    }
}

/// One place the value passes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub file: String,
    pub line: u32,
    /// The traced name there, when it is written on one line.
    pub span: Option<Span>,
    /// The function the step is in, if indexed.
    pub symbol: Option<SymbolId>,
    /// The line's code, trimmed.
    pub code: String,
    /// What happens to the value here.
    pub note: String,
}

/// Where the arguments that make `finding`'s call risky come from. `None`
/// if no call is found where the finding is.
pub fn trace_finding(graph: &CodeGraph, finding: &Finding) -> Option<Trace> {
    walk::Tracer::new(graph).finding(finding)
}

/// Where the variable written at `line` (1-based), byte `column` of `file`
/// gets its value. `None` if no variable is written there.
pub fn trace_variable(graph: &CodeGraph, file: &str, line: u32, column: u32) -> Option<Trace> {
    walk::Tracer::new(graph).variable(file, line, column)
}

/// `paths` without repeats, those from outside input first, then shorter
/// ones.
fn ordered(paths: Vec<TracePath>) -> Vec<TracePath> {
    let mut unique: Vec<TracePath> = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique.sort_by_key(|p| (!p.origin.is_input(), p.steps.len()));
    unique
}

#[cfg(test)]
mod tests;
