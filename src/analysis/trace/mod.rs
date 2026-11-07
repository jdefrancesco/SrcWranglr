//! Tracing a value through the code, either way:
//!
//! - **Back**, to where it comes from: from a risky call's argument (or
//!   any variable you pick) to the assignments, copies and calls that set
//!   it, across functions through parameters, return values and the
//!   buffers a function fills, until outside input (the command line,
//!   `fgets`, `getenv`, …) or a stop it can name.
//! - **Forward**, to where it goes: from a variable you pick to the risky
//!   calls (the findings) it reaches, through the same assignments,
//!   copies, calls, returns and buffers.
//!
//! It reads each function's syntax with tree-sitter and knows no types or
//! aliases: a write through another pointer to the same memory, a call
//! through a function pointer, or a check that limits the value isn't seen.
//! So a path means the value *may* go that way, and no path means none
//! was found.

mod calls;
mod defs;
mod files;
mod flows;
mod forward;
mod goes;
mod scope;
mod sites;
mod walk;

use crate::analysis::Finding;
use crate::model::{CodeGraph, Span, SymbolId};

/// Where a traced value comes from or goes, and every step on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// What was traced: the risky call, or `` `name` ``.
    pub subject: String,
    pub file: String,
    pub line: u32,
    pub direction: Direction,
    /// Paths to what was looked for (outside input, a risky call) first,
    /// then shorter ones.
    pub paths: Vec<TracePath>,
    /// A limit was hit, so there may be more paths than these.
    pub is_cut_short: bool,
}

/// Which way a trace follows the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Back to where it comes from.
    Back,
    /// Forward to the risky calls it reaches.
    Forward,
}

/// One way the value goes, its steps in the order the value takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracePath {
    /// Traced back, where the path starts; traced forward, where it ends.
    pub endpoint: Endpoint,
    pub steps: Vec<Step>,
}

/// The far end of a path from what was traced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Traced back: outside input, e.g. `the command line` or `fgets()`.
    Input(String),
    /// Traced forward: a risky call it reaches, e.g. "`strcpy()` (high)".
    Sink(String),
    /// Where the trace stopped, and why.
    Stopped(String),
}

impl Endpoint {
    pub fn is_input(&self) -> bool {
        matches!(self, Self::Input(_))
    }

    pub fn is_sink(&self) -> bool {
        matches!(self, Self::Sink(_))
    }

    /// "from fgets()", "reaches `strcpy()` (high)", or "stops: …".
    pub fn label(&self) -> String {
        match self {
            Self::Input(source) => format!("from {source}"),
            Self::Sink(call) => format!("reaches {call}"),
            Self::Stopped(why) => format!("stops: {why}"),
        }
    }
}

/// A risky call a value traced forward may reach: where a finding is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    pub file: String,
    /// 1-based line of the call.
    pub line: u32,
    /// Byte column of the call on that line, when the finding knows it.
    pub column: Option<u32>,
    /// How bad, and who says so: `high`, `medium, semgrep`.
    pub detail: String,
}

impl Sink {
    /// The call `finding` is about.
    pub fn of(finding: &Finding) -> Self {
        let severity = finding.severity.label();
        let detail = match finding.tool.as_str() {
            crate::analysis::risky::TOOL => severity.to_string(),
            tool => format!("{severity}, {tool}"),
        };
        Self {
            file: finding.file.clone(),
            line: finding.line,
            column: finding.columns.map(|(start, _)| start),
            detail,
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
#[cfg(test)]
pub fn trace_finding(graph: &CodeGraph, finding: &crate::analysis::Finding) -> Option<Trace> {
    let column = finding.columns.map_or(0, |(start, _)| start);
    trace_call(graph, &finding.file, finding.line, column)
}

/// Where the arguments that make the call at `line` (1-based), byte
/// `column` of `file` risky come from: the call named there, else the
/// first one starting on that line at or after it.
pub fn trace_call(graph: &CodeGraph, file: &str, line: u32, column: u32) -> Option<Trace> {
    walk::Tracer::new(graph).call(file, line, column)
}

/// Where the variable written at `line` (1-based), byte `column` of `file`
/// gets its value. `None` if no variable is written there.
pub fn trace_variable(graph: &CodeGraph, file: &str, line: u32, column: u32) -> Option<Trace> {
    walk::Tracer::new(graph).variable(file, line, column)
}

/// Where the variable written at `line` (1-based), byte `column` of `file`
/// goes from there: the paths to the `sinks` (risky calls) it reaches.
/// `None` if no variable is written there.
pub fn trace_forward(
    graph: &CodeGraph,
    sinks: &[Sink],
    file: &str,
    line: u32,
    column: u32,
) -> Option<Trace> {
    forward::Follower::new(graph, sinks).variable(file, line, column)
}

/// `paths` without repeats, those to what was looked for (outside input, a
/// risky call) first, then shorter ones.
fn ordered(paths: Vec<TracePath>) -> Vec<TracePath> {
    let mut unique: Vec<TracePath> = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    let is_found = |p: &TracePath| p.endpoint.is_input() || p.endpoint.is_sink();
    unique.sort_by_key(|p| (!is_found(p), p.steps.len()));
    unique
}

#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod tests;
