//! What makes a function worth fuzzing: a name that says it handles input,
//! risky calls inside it, many paths through it, and a caller that reads
//! input and hands it over.

use std::collections::HashMap;

use super::params::words;
use super::{reason, Reason};
use crate::analysis::{Finding, Severity};
use crate::model::{CodeGraph, EdgeKind, FunctionMetrics, SymbolId};

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
/// Library calls that bring outside input into the program.
const INPUT_SOURCES: &[&str] = &[
    "recv",
    "recvfrom",
    "recvmsg",
    "read",
    "pread",
    "readv",
    "fread",
    "fgets",
    "getline",
    "getdelim",
    "ReadFile",
    "WSARecv",
    "WSARecvFrom",
    "SSL_read",
    "BIO_read",
    "gzread",
];
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
    /// Findings per file.
    findings: HashMap<&'g str, Vec<&'g Finding>>,
    /// Project functions that read input, and the first (by name) of the
    /// library functions they read it with.
    readers: HashMap<SymbolId, &'g str>,
    /// Functions called by a reader: the first such caller by name, and
    /// what it reads input with.
    fed: HashMap<SymbolId, (&'g str, &'g str)>,
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
        let mut readers: HashMap<SymbolId, &str> = HashMap::new();
        let calls = graph.external_calls.values().flatten();
        for call in calls {
            let name = call.name.rsplit("::").next().unwrap_or(&call.name);
            if INPUT_SOURCES.contains(&name) {
                let first = readers.entry(call.caller).or_insert(name);
                *first = (*first).min(name);
            }
        }
        let fed = fed_by_readers(graph, &readers);
        Self {
            findings: by_file,
            readers,
            fed,
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

    /// A caller that reads input (and so probably passes it on).
    fn fed_reason(&self, symbol: SymbolId) -> Option<Reason> {
        if self.readers.contains_key(&symbol) {
            return None;
        }
        let (caller, source) = self.fed.get(&symbol)?;
        Some(reason(
            FED_BY_INPUT_POINTS,
            format!("called from {caller}, which reads input with {source}()"),
        ))
    }
}

/// For each function a reader calls, its first reader by name.
fn fed_by_readers<'g>(
    graph: &'g CodeGraph,
    readers: &HashMap<SymbolId, &'g str>,
) -> HashMap<SymbolId, (&'g str, &'g str)> {
    let mut fed: HashMap<SymbolId, (&str, &str)> = HashMap::new();
    let calls = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls && e.from != e.to);
    for edge in calls {
        let (Some(source), Some(caller)) = (readers.get(&edge.from), graph.symbols.get(&edge.from))
        else {
            continue;
        };
        let candidate = (caller.qualified_name.as_str(), *source);
        let best = fed.entry(edge.to).or_insert(candidate);
        *best = (*best).min(candidate);
    }
    fed
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
