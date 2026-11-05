//! Static checks over the indexed code: calls to risky C library functions
//! (and, from external tools, their findings), and what each function
//! does by way of the APIs it calls.

pub mod capabilities;
pub mod dangerous;
pub mod heuristics;
pub mod risky;
pub mod tools;

use crate::model::SymbolId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Severity::High, Severity::Medium, Severity::Low];

    pub fn label(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

/// Something worth a look at one place in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What found it, e.g. `SrcWranglr` or `semgrep`.
    pub tool: String,
    /// The rule, e.g. `strcpy` or a semgrep rule id.
    pub rule: String,
    pub severity: Severity,
    /// Why it's worth a look.
    pub message: String,
    /// What to do about it, when the rule says.
    pub fix: Option<String>,
    pub cwe: Option<u32>,
    /// Project-relative file and 1-based line.
    pub file: String,
    pub line: u32,
    /// Byte columns `start..end` of the snippet on `line`, when known.
    pub columns: Option<(u32, u32)>,
    /// The symbol whose code it is in, if known.
    pub symbol: Option<SymbolId>,
}
