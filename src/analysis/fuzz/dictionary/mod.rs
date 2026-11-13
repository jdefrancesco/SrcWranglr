//! Drafts a fuzzing dictionary for one function: the strings and
//! characters it and the functions it calls compare the input with or
//! write, in the format both AFL++ (`-x`) and libFuzzer (`-dict=`) read.
//! A fuzzer splices these tokens into inputs, which gets it past checks
//! like `strcmp(cmd, "PING")` far sooner than flipping bits would.
//!
//! The tokens come from a source tree that may be hostile, so every byte
//! outside printable ASCII is written as `\xNN`, and names in comments
//! keep only printable ASCII: nothing can add a line of its own.

mod literal;

use std::collections::{BTreeMap, HashSet};

use super::harness::{comment_safe, stem_for};
use crate::model::{CodeGraph, EdgeKind, ExternalCall, FunctionMetrics, SymbolId};
use literal::{char_constants, is_format, string_bytes};

/// Tokens kept, most useful first.
pub const MAX_TOKENS: usize = 200;
/// AFL++ refuses a dictionary with a longer token.
pub const MAX_TOKEN_BYTES: usize = 128;
/// How many calls deep the functions it calls are read.
const CALL_DEPTH: usize = 2;
/// The functions it calls read at most.
const MAX_CALLEES: usize = 100;
/// Library calls whose constant arguments are compared with the input.
const COMPARING_CALLS: &[&str] = &[
    "strcmp",
    "strncmp",
    "strcasecmp",
    "strncasecmp",
    "memcmp",
    "bcmp",
    "strstr",
    "strcasestr",
    "strnstr",
    "memmem",
    "strchr",
    "strrchr",
    "memchr",
    "strpbrk",
    "strspn",
    "strcspn",
    "strtok",
    "strtok_r",
    "strsep",
];
/// Of those, the ones whose string argument is a set of characters, each
/// of which is a token: `strcspn(s, " \t")`, and `strchr("\"'", c)`
/// asking whether `c` is one of them.
const SET_CALLS: &[&str] = &[
    "strspn", "strcspn", "strpbrk", "strtok", "strtok_r", "strsep", "strchr", "strrchr", "memchr",
];

/// A drafted dictionary, ready to show and save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dictionary {
    /// `fuzz_<function>.dict`, next to the harnesses.
    pub file_name: String,
    pub text: String,
    pub token_count: usize,
}

/// One group of tokens, under a comment saying where they came from.
struct Section {
    heading: String,
    tokens: Vec<Vec<u8>>,
}

/// The dictionary for `function`; `None` if it and the functions it calls
/// have no constants (or its symbol isn't in `graph`).
pub fn draft(graph: &CodeGraph, function: &FunctionMetrics) -> Option<Dictionary> {
    let name = graph.symbols.get(&function.symbol)?.qualified_name.as_str();
    let callees = callees(graph, function.symbol);
    let everything: Vec<&FunctionMetrics> = [function].into_iter().chain(callees.clone()).collect();
    let (compared, sets) = compared(graph, &everything);
    let sections = [
        Section {
            heading: "Compared with the input (strcmp, memcmp, strchr and the like)".into(),
            tokens: compared,
        },
        Section {
            heading: format!("Written in {}", comment_safe(name)),
            tokens: written(graph, &[function]),
        },
        Section {
            heading: "Written in the functions it calls".into(),
            tokens: written(graph, &callees),
        },
    ];
    let (listed, token_count, is_cut) = listed(sections, &sets);
    if token_count == 0 {
        return None;
    }
    let file_name = format!("{}.dict", stem_for(name));
    let mut lines = intro(name, function, &file_name);
    if is_cut {
        lines.push(format!(
            "# Only the first {MAX_TOKENS} tokens are kept; more were left out."
        ));
    }
    lines.extend(listed);
    Some(Dictionary {
        file_name,
        text: lines.join("\n") + "\n",
        token_count,
    })
}

/// What the dictionary is for, and how to use it.
fn intro(name: &str, function: &FunctionMetrics, file_name: &str) -> Vec<String> {
    vec![
        format!(
            "# Fuzzing dictionary for {} ({}:{}), drafted by SrcWranglr:",
            comment_safe(name),
            comment_safe(&function.file),
            function.line
        ),
        "# strings and characters it and the functions it calls compare or write.".into(),
        format!("# Use it with afl-fuzz -x {file_name}, or libFuzzer's -dict={file_name}."),
        "# Remove tokens that only go into messages; add ones you know the input has.".into(),
    ]
}

/// Each section's token lines under its heading, without repeats, tokens
/// not worth one, or the `sets` already listed character by character,
/// up to `MAX_TOKENS` in all; how many were listed, and whether some were
/// left out.
fn listed(sections: [Section; 3], sets: &HashSet<Vec<u8>>) -> (Vec<String>, usize, bool) {
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut lines = Vec::new();
    let mut is_cut = false;
    for section in sections {
        let fresh: Vec<Vec<u8>> = section
            .tokens
            .into_iter()
            .filter(|t| is_worth_a_token(t) && !sets.contains(t))
            .filter(|t| seen.insert(t.clone()))
            .collect();
        let room = MAX_TOKENS - (seen.len() - fresh.len()).min(MAX_TOKENS);
        is_cut |= fresh.len() > room;
        if fresh.is_empty() || room == 0 {
            continue;
        }
        lines.extend([String::new(), format!("# {}", section.heading)]);
        lines.extend(fresh.iter().take(room).map(|t| token_line(t)));
    }
    let count = lines.iter().filter(|l| l.starts_with('"')).count();
    (lines, count, is_cut)
}

/// Whether `token` is worth listing: not empty, not too long for AFL++,
/// and not a lone NUL byte (fuzzers make those on their own).
fn is_worth_a_token(token: &[u8]) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN_BYTES && token != [0]
}

/// A token as a dictionary line: printable ASCII as is, `"` and `\`
/// escaped, every other byte as `\xNN`.
fn token_line(token: &[u8]) -> String {
    let escaped: String = token
        .iter()
        .map(|&b| match b {
            b'"' => "\\\"".to_string(),
            b'\\' => "\\\\".to_string(),
            0x20..=0x7E => char::from(b).to_string(),
            _ => format!("\\x{b:02X}"),
        })
        .collect();
    format!("\"{escaped}\"")
}

/// The definitions of the functions `from` calls, `CALL_DEPTH` calls
/// deep, nearest first (and by place within a depth).
fn callees(graph: &CodeGraph, from: SymbolId) -> Vec<&FunctionMetrics> {
    let mut seen = HashSet::from([from]);
    let mut frontier = HashSet::from([from]);
    let mut levels: Vec<Vec<SymbolId>> = Vec::new();
    for _ in 0..CALL_DEPTH {
        let next: Vec<SymbolId> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && frontier.contains(&e.from))
            .map(|e| e.to)
            .filter(|to| seen.insert(*to))
            .collect();
        frontier = next.iter().copied().collect();
        levels.push(next);
    }
    let depth: BTreeMap<SymbolId, usize> = levels
        .iter()
        .enumerate()
        .flat_map(|(d, level)| level.iter().map(move |id| (*id, d)))
        .collect();
    let mut found: Vec<&FunctionMetrics> = graph
        .functions
        .iter()
        .filter(|f| depth.contains_key(&f.symbol))
        .collect();
    found.sort_by_key(|f| (depth[&f.symbol], f.file.as_str(), f.line));
    found.truncate(MAX_CALLEES);
    found
}

/// The constants `functions` pass to calls that compare (`strcmp(s,
/// "PING")`, `strchr(s, '=')`), and the strings among them that are sets
/// of characters (`strcspn(s, " \t")`), whose characters are listed alone.
fn compared(graph: &CodeGraph, functions: &[&FunctionMetrics]) -> (Vec<Vec<u8>>, HashSet<Vec<u8>>) {
    let mut tokens = Vec::new();
    let mut sets = HashSet::new();
    let mut whole = HashSet::new();
    for call in functions.iter().flat_map(|f| calls_in(graph, f)) {
        let leaf = call.name.rsplit("::").next().unwrap_or(&call.name);
        if !COMPARING_CALLS.contains(&leaf) {
            continue;
        }
        let is_set = SET_CALLS.contains(&leaf);
        for arg in &call.args {
            match string_bytes(arg) {
                Some(set) if is_set => {
                    tokens.extend(set.iter().map(|b| vec![*b]));
                    sets.insert(set);
                }
                Some(string) => {
                    whole.insert(string.clone());
                    tokens.push(string);
                }
                None => tokens.extend(char_constants(arg)),
            }
        }
    }
    // A set also compared whole (`strcmp(s, " \t")`) is a token after all.
    (tokens, sets.difference(&whole).cloned().collect())
}

/// The library calls `function` makes.
fn calls_in<'a>(
    graph: &'a CodeGraph,
    function: &'a FunctionMetrics,
) -> impl Iterator<Item = &'a ExternalCall> {
    let in_file = |calls: &'a std::collections::HashMap<String, Vec<ExternalCall>>| {
        calls.get(&function.file).into_iter().flatten()
    };
    in_file(&graph.external_calls)
        .chain(in_file(&graph.library_calls_in_tree))
        .filter(move |call| call.caller == function.symbol && is_inside(function, call.span.line))
}

/// The character constants, then the strings (not formats), each of
/// `functions` writes.
fn written(graph: &CodeGraph, functions: &[&FunctionMetrics]) -> Vec<Vec<u8>> {
    functions
        .iter()
        .flat_map(|f| {
            let strings = graph
                .strings
                .get(&f.file)
                .into_iter()
                .flatten()
                .filter(|s| s.function == Some(f.symbol) && is_inside(f, s.span.line))
                .filter_map(|s| string_bytes(&s.text))
                .filter(|bytes| !is_format(bytes));
            char_constants(source_of(graph, f))
                .into_iter()
                .chain(strings)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether `line` is within `function`'s definition.
fn is_inside(function: &FunctionMetrics, line: u32) -> bool {
    (function.line..function.line.saturating_add(function.lines)).contains(&line)
}

/// The source text of `function`'s definition.
fn source_of<'a>(graph: &'a CodeGraph, function: &FunctionMetrics) -> &'a str {
    let Some(contents) = graph.file_contents.get(&function.file) else {
        return "";
    };
    let line_start = |line: u32| -> usize {
        match line {
            0 | 1 => 0,
            line => contents
                .match_indices('\n')
                .nth(line as usize - 2)
                .map_or(contents.len(), |(at, _)| at + 1),
        }
    };
    let start = line_start(function.line);
    let end = line_start(function.line.saturating_add(function.lines)).max(start);
    &contents[start..end]
}

#[cfg(test)]
mod tests;
