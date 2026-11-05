//! What makes a function worth fuzzing: a name that says it handles input,
//! risky calls inside it, many paths through it, and outside input that
//! reaches it down the call graph.

use std::collections::HashMap;

use super::params::words;
use super::{reason, Reason};
use crate::analysis::reach::{describe, Reach};
use crate::analysis::{Finding, Severity};
use crate::model::{CodeGraph, FunctionMetrics, SymbolId};

/// Name words that say a function takes input apart.
const PARSER_WORDS: &[&str] = &[
    "parse",
    "decode",
    "deserialize",
    "deserialise",
    "unserialize",
    "unpack",
    "unmarshal",
    "inflate",
    "decompress",
    "uncompress",
    "unzip",
    "lex",
    "tokenize",
    "tokenise",
    "unescape",
    "demux",
    "dissect",
];
/// Name words that say a function probably handles input.
const INPUT_WORDS: &[&str] = &[
    "read", "load", "import", "scan", "extract", "recv", "receive", "handle", "process", "decrypt",
    "verify", "packet", "message", "msg", "request", "header", "chunk", "frame", "record", "input",
];
/// Endings that keep a word the same word: `parser`, `decoded`, `reading`.
const WORD_ENDINGS: &[&str] = &[
    "", "s", "es", "d", "ed", "r", "er", "ers", "or", "ors", "ing",
];
const PARSER_POINTS: i32 = 3;
const INPUT_POINTS: i32 = 1;
const FED_BY_INPUT_POINTS: i32 = 2;
/// Complexity above each bound, and the points it adds.
const COMPLEXITY_POINTS: [(u32, i32); 3] = [(50, 3), (20, 2), (10, 1)];
/// Folders whose code is rarely the project's shipped code.
const NON_LIBRARY_DIRS: &[&str] = &[
    "example",
    "examples",
    "test",
    "tests",
    "testing",
    "demo",
    "demos",
    "sample",
    "samples",
    "bench",
    "benches",
    "benchmark",
    "benchmarks",
];
const NON_LIBRARY_POINTS: i32 = -2;
const HIGH_FINDING_POINTS: i32 = 2;
const MEDIUM_FINDING_POINTS: i32 = 1;
const MAX_FINDING_POINTS: i32 = 6;

/// What the whole graph says about each function's worth, gathered once.
pub(super) struct Worth<'g> {
    graph: &'g CodeGraph,
    /// Findings per file.
    findings: HashMap<&'g str, Vec<&'g Finding>>,
    /// Which functions outside input reaches, and how.
    reach: Reach,
}

impl<'g> Worth<'g> {
    pub(super) fn new(graph: &'g CodeGraph, findings: &'g [Finding]) -> Self {
        let mut by_file: HashMap<&str, Vec<&Finding>> = HashMap::new();
        for finding in findings {
            by_file
                .entry(finding.file.as_str())
                .or_default()
                .push(finding);
        }
        Self {
            graph,
            findings: by_file,
            reach: Reach::of(graph),
        }
    }

    /// Why `f` (named `qualified`) is worth fuzzing. A caller's input only
    /// counts if `f` `takes_input` it could be handed.
    pub(super) fn reasons(
        &self,
        f: &FunctionMetrics,
        qualified: &str,
        takes_input: bool,
    ) -> Vec<Reason> {
        [
            name_reason(qualified),
            self.findings_reason(f),
            complexity_reason(f.complexity),
            takes_input.then(|| self.fed_reason(f.symbol)).flatten(),
            location_reason(&f.file),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Risky calls and tool findings inside the definition.
    fn findings_reason(&self, f: &FunctionMetrics) -> Option<Reason> {
        let end = f.line.saturating_add(f.lines);
        let inside: Vec<&Finding> = self
            .findings
            .get(f.file.as_str())?
            .iter()
            .copied()
            .filter(|finding| (f.line..end).contains(&finding.line))
            .collect();
        let points: i32 = inside
            .iter()
            .map(|finding| match finding.severity {
                Severity::High => HIGH_FINDING_POINTS,
                Severity::Medium => MEDIUM_FINDING_POINTS,
                Severity::Low => 0,
            })
            .sum();
        if points == 0 {
            return None;
        }
        let mut rules: Vec<&str> = inside.iter().map(|finding| finding.rule.as_str()).collect();
        rules.sort_unstable();
        rules.dedup();
        let noun = if inside.len() == 1 {
            "risky call"
        } else {
            "risky calls"
        };
        Some(reason(
            points.min(MAX_FINDING_POINTS),
            format!("{} {noun} inside: {}", inside.len(), rules.join(", ")),
        ))
    }

    /// Input that reaches it from a function further up (one that reads
    /// input itself is the I/O loop, not what to fuzz).
    fn fed_reason(&self, symbol: SymbolId) -> Option<Reason> {
        let path = self.reach.path_to(symbol)?;
        if path.functions.len() < 2 {
            return None;
        }
        Some(reason(
            FED_BY_INPUT_POINTS,
            format!("input reaches it: {}", describe(&path, self.graph)),
        ))
    }
}

/// What the function's (and its class's) name says it does.
fn name_reason(qualified: &str) -> Option<Reason> {
    let segments: Vec<&str> = qualified.rsplit("::").take(2).collect();
    let words: Vec<String> = segments.iter().flat_map(|s| words(s)).collect();
    let hit = |stems: &[&'static str]| {
        stems
            .iter()
            .copied()
            .find(|stem| words.iter().any(|w| is_form_of(w, stem)))
    };
    if let Some(stem) = hit(PARSER_WORDS) {
        return Some(reason(
            PARSER_POINTS,
            format!("its name says it takes input apart ({stem})"),
        ));
    }
    hit(INPUT_WORDS).map(|stem| {
        reason(
            INPUT_POINTS,
            format!("its name says it handles input ({stem})"),
        )
    })
}

fn is_form_of(word: &str, stem: &str) -> bool {
    word.strip_prefix(stem)
        .is_some_and(|ending| WORD_ENDINGS.contains(&ending))
        // `parse` + `d`, but also `decode` → `decoding`.
        || stem
            .strip_suffix('e')
            .and_then(|s| word.strip_prefix(s))
            .is_some_and(|ending| ending == "ing")
}

/// Code in examples, tests or benchmarks counts for less.
fn location_reason(file: &str) -> Option<Reason> {
    let folders = file.rsplit_once('/').map_or("", |(dir, _)| dir);
    let folder = folders
        .split('/')
        .find(|d| NON_LIBRARY_DIRS.contains(&d.to_ascii_lowercase().as_str()))?;
    Some(reason(
        NON_LIBRARY_POINTS,
        format!("in {folder}/, not the library itself"),
    ))
}

fn complexity_reason(complexity: u32) -> Option<Reason> {
    let (_, points) = COMPLEXITY_POINTS
        .iter()
        .find(|(above, _)| complexity > *above)?;
    Some(reason(
        *points,
        format!("complexity {complexity}: many paths to explore"),
    ))
}
