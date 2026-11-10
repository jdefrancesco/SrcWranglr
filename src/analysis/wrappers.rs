//! The project's own functions that pass their parameters straight on to
//! a risky call, such as `void copy(char *d, const char *s) { strcpy(d, s); }`
//! or `void log_msg(const char *fmt, ...) { … vprintf(fmt, ap); }`. Each
//! call to one is a call to the risky function with the caller's values,
//! so each gets a semgrep rule that finds the calls worth a look:
//!
//! - a format passed on: calls whose format isn't a literal;
//! - a size passed on to a copy: calls whose size isn't a constant;
//! - a size passed on to an allocator: calls whose size is a product or
//!   a sum, which can overflow;
//! - anything else passed on to a call that is risky whatever its values
//!   (`strcpy`, `system`): every call.
//!
//! Only one level: a wrapper of a wrapper isn't followed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use super::heuristics;
use super::risky;
use super::semgrep_rule::{rule_id, rules_yaml, Clause, RuleSpec};
use super::Severity;
use crate::model::{
    CodeGraph, EdgeKind, ExternalCall, OccurrenceRole, Symbol, SymbolId, SymbolKind,
};

/// What a wrapper passes on, and so which calls to it are worth a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Passes {
    /// Its parameter `param` (0-based) as the format.
    Format { param: usize },
    /// Its parameter `param` as the size of a copy.
    CopySize { param: usize },
    /// Its parameter `param` as the size of an allocation.
    AllocSize { param: usize },
    /// Some of its parameters, to a call risky whatever they are.
    Arguments,
}

/// A function passing its parameters on to a risky call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapper {
    pub name: String,
    pub symbol: SymbolId,
    /// Where it is defined.
    pub file: String,
    pub line: u32,
    /// The risky function it calls, and the line of the call.
    pub callee: String,
    pub call_line: u32,
    pub passes: Passes,
    /// The parameters it passes on, by name.
    pub passed: Vec<String>,
    /// Calls to it in the project.
    pub callers: usize,
    pub severity: Severity,
    pub why: String,
}

/// Wrappers kept at most, those called most first.
const MAX_WRAPPERS: usize = 5_000;
/// How much of a function's text, from its name on, is read for its
/// parameters.
const MAX_HEADER_BYTES: usize = 4_096;
const MAX_ID_CHARS: usize = 60;
/// A copy size with a lowercase name in it, that isn't a number literal.
const NAMES_SOMETHING: &str = "(?!(0[xX][0-9a-fA-F]+|[0-9]+)[uUlL]*$).*[a-z]";
/// Words that are part of a parameter's type, never its name.
const TYPE_WORDS: [&str; 18] = [
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "const",
    "volatile", "struct", "union", "enum", "bool", "_Bool", "register", "restrict",
];

/// The wrappers in `graph`, those called most first; `None` if `stop`
/// was set.
pub fn wrappers(graph: &CodeGraph, stop: &AtomicBool) -> Option<Vec<Wrapper>> {
    let callers = call_counts(graph);
    let mut parameters: HashMap<SymbolId, Option<Vec<Option<String>>>> = HashMap::new();
    let mut found: HashMap<String, Wrapper> = HashMap::new();
    let files = graph
        .external_calls
        .iter()
        .chain(&graph.library_calls_in_tree);
    for (file, calls) in files {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        for call in calls {
            let Some(caller) = graph.symbols.get(&call.caller) else {
                continue;
            };
            if !could_wrap(caller) {
                continue;
            }
            let params = parameters
                .entry(caller.id)
                .or_insert_with(|| parameters_of(graph, caller));
            let Some(params) = params.as_deref() else {
                continue;
            };
            let Some(wrapper) = wrapper_of(file, call, caller, params) else {
                continue;
            };
            let callers = callers.get(&caller.id).copied().unwrap_or(0);
            let wrapper = Wrapper { callers, ..wrapper };
            let kept = match found.remove(&wrapper.name) {
                Some(kept) => merged(kept, wrapper),
                None => wrapper,
            };
            found.insert(kept.name.clone(), kept);
        }
    }
    let mut list: Vec<Wrapper> = found.into_values().collect();
    list.sort_by(|a, b| (b.callers, a.severity, &a.name).cmp(&(a.callers, b.severity, &b.name)));
    list.truncate(MAX_WRAPPERS);
    Some(list)
}

/// One wrapper for two of the same name: the most severe risky call. Two
/// functions of one name (static ones, in different files) that pass on
/// different things get a rule for every call, which fits both.
fn merged(kept: Wrapper, other: Wrapper) -> Wrapper {
    let is_other_function = kept.symbol != other.symbol;
    let is_different = is_other_function && kept.passes != other.passes;
    let callers = match is_other_function {
        true => kept.callers + other.callers,
        false => kept.callers,
    };
    let more_severe = match other.severity < kept.severity {
        true => other,
        false => kept,
    };
    Wrapper {
        callers,
        passes: match is_different {
            true => Passes::Arguments,
            false => more_severe.passes,
        },
        ..more_severe
    }
}

/// Whether `symbol` may be a wrapper: a plainly named function that isn't
/// `main` or the project's own copy of a library function.
fn could_wrap(symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Function
        && symbol.name != "main"
        && is_plain_name(&symbol.name)
        && !risky::is_library_name(&symbol.name)
}

fn is_plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The wrapper `caller` (with `params`) is if `call` (in `file`) passes
/// what makes it risky on from them.
fn wrapper_of(
    file: &str,
    call: &ExternalCall,
    caller: &Symbol,
    params: &[Option<String>],
) -> Option<Wrapper> {
    let callee = risky::bare_library_name(&call.name)?;
    let passed_at = |arg: usize| call.args.get(arg).and_then(|a| parameter_index(a, params));
    let (passes, severity, why) = if let Some(arg) = heuristics::format_argument(callee) {
        let assessment = heuristics::non_literal_format();
        let param = passed_at(arg)?;
        (
            Passes::Format { param },
            assessment.severity,
            assessment.why.to_string(),
        )
    } else if let Some(arg) = heuristics::alloc_size_argument(callee) {
        let assessment = heuristics::size_overflow();
        let param = passed_at(arg)?;
        (
            Passes::AllocSize { param },
            assessment.severity,
            assessment.why.to_string(),
        )
    } else {
        let finding = risky::built_in_finding(file, call)?;
        let passes = match heuristics::copy_size_argument(callee) {
            Some(arg) => Passes::CopySize {
                param: passed_at(arg)?,
            },
            None if (0..call.args.len()).any(|i| passed_at(i).is_some()) => Passes::Arguments,
            None => return None,
        };
        (passes, finding.severity, finding.message)
    };
    let mut passed: Vec<String> = Vec::new();
    for name in (0..call.args.len())
        .filter_map(passed_at)
        .filter_map(|p| params.get(p)?.clone())
    {
        if !passed.contains(&name) {
            passed.push(name);
        }
    }
    Some(Wrapper {
        name: caller.name.clone(),
        symbol: caller.id,
        file: caller.file.clone(),
        line: caller.line,
        callee: callee.to_string(),
        call_line: call.extent.line,
        passes,
        passed,
        callers: 0,
        severity,
        why,
    })
}

/// How many places call each function.
fn call_counts(graph: &CodeGraph) -> HashMap<SymbolId, usize> {
    let mut counts = HashMap::new();
    let calls = graph
        .occurrences
        .values()
        .flatten()
        .filter(|o| o.role == OccurrenceRole::Reference(EdgeKind::Calls));
    for call in calls {
        *counts.entry(call.symbol).or_insert(0) += 1;
    }
    counts
}

/// Which of `params` the argument `arg` is, as written (a cast or
/// parentheses around it aside).
fn parameter_index(arg: &str, params: &[Option<String>]) -> Option<usize> {
    let bare = without_cast(arg.trim());
    params
        .iter()
        .position(|p| p.as_deref().is_some_and(|name| name == bare))
}

fn without_cast(mut arg: &str) -> &str {
    for _ in 0..4 {
        let Some(inner) = arg.strip_prefix('(') else {
            break;
        };
        let Some(close) = matching_paren(inner) else {
            break;
        };
        let rest = inner[close + 1..].trim();
        arg = match rest.is_empty() {
            // `(x)`
            true => inner[..close].trim(),
            // `(char *)x`
            false => rest,
        };
    }
    arg
}

/// Where the `)` closing the text before it is, in `text` that follows a
/// `(`.
fn matching_paren(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The names of `function`'s parameters, in order (`None` for one with no
/// name), read from its text; `None` if they can't be read.
fn parameters_of(graph: &CodeGraph, function: &Symbol) -> Option<Vec<Option<String>>> {
    let source = graph.file_contents.get(&function.file)?;
    let start = line_start(source, function.line)?;
    let mut end = (start + MAX_HEADER_BYTES).min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let header = without_comments(&source[start..end]);
    let header = header.as_str();
    let open = name_followed_by_paren(header, &function.name)?;
    let inner = &header[open + 1..];
    let list = &inner[..matching_paren(inner)?];
    let params: Vec<&str> = split_top_level(list).collect();
    if let [only] = params.as_slice() {
        if matches!(only.trim(), "" | "void") {
            return Some(Vec::new());
        }
    }
    Some(
        params
            .into_iter()
            .filter(|p| p.trim() != "...")
            .map(parameter_name)
            .collect(),
    )
}

fn line_start(source: &str, line: u32) -> Option<usize> {
    let skip = usize::try_from(line.checked_sub(1)?).ok()?;
    if skip == 0 {
        return Some(0);
    }
    source.match_indices('\n').nth(skip - 1).map(|(i, _)| i + 1)
}

/// Where the `(` after `name` (as a whole word) is in `text`.
fn name_followed_by_paren(text: &str, name: &str) -> Option<usize> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(name).find_map(|(at, _)| {
        let before = text[..at].chars().next_back();
        if before.is_some_and(is_word) {
            return None;
        }
        let after = &text[at + name.len()..];
        let skipped = after.len() - after.trim_start().len();
        after
            .trim_start()
            .starts_with('(')
            .then_some(at + name.len() + skipped)
    })
}

/// `list` split at the commas not inside brackets.
fn split_top_level(list: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0i32;
    let mut start = 0;
    let mut parts = Vec::new();
    let mut previous = ' ';
    for (i, c) in list.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            // `->` isn't a bracket.
            '>' if previous == '-' => {}
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        previous = c;
    }
    parts.push(&list[start..]);
    parts.into_iter()
}

/// `text` with its comments and preprocessor lines blanked out.
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
            out.push(' ');
        } else if rest.starts_with("//")
            || (rest.starts_with('#') && out.trim_end_matches(' ').ends_with('\n'))
        {
            rest = rest.find('\n').map_or("", |end| &rest[end..]);
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A parameter's name from its declaration (`const char *name`,
/// `int (*cb)(int)`, `char buf[64]`), if it has one.
fn parameter_name(param: &str) -> Option<String> {
    let param = param.split('=').next().unwrap_or(param).trim();
    if let Some(at) = param.find("(*") {
        let name: String = param[at + 2..]
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        return (!name.is_empty()).then_some(name);
    }
    let without_arrays = param.split('[').next().unwrap_or(param);
    let words: Vec<&str> = without_arrays
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let [.., before, last] = words.as_slice() else {
        return None;
    };
    let is_name = !TYPE_WORDS.contains(last)
        && !matches!(*before, "struct" | "union" | "enum")
        && !last.starts_with(|c: char| c.is_ascii_digit());
    is_name.then(|| last.to_string())
}

/// A rules file with a rule for each of `wrappers`.
pub fn rules_for(wrappers: &[&Wrapper]) -> String {
    let mut used: Vec<String> = Vec::new();
    let rules: Vec<RuleSpec> = wrappers
        .iter()
        .map(|w| {
            let rule = rule_for(w);
            // Names that differ only in case or `_` make the same id.
            let id = (1..)
                .map(|n| match n {
                    1 => rule.id.clone(),
                    n => format!("{}-{n}", rule.id),
                })
                .find(|id| !used.contains(id))
                .unwrap_or_default();
            used.push(id.clone());
            RuleSpec { id, ..rule }
        })
        .collect();
    rules_yaml(&rules)
}

fn rule_for(wrapper: &Wrapper) -> RuleSpec {
    let name = &wrapper.name;
    let before = |param: usize| -> String { (1..=param).map(|i| format!("$X{i}, ")).collect() };
    let clauses = match wrapper.passes {
        Passes::Format { param } => vec![
            Clause::Pattern(format!("{name}({}$FORMAT, ...)", before(param))),
            Clause::Not(format!("{name}({}\"...\", ...)", before(param))),
        ],
        Passes::CopySize { param } => vec![
            Clause::Pattern(format!("{name}({}$SIZE, ...)", before(param))),
            Clause::Not(format!("{name}({}sizeof(...), ...)", before(param))),
            // A size that names something: not a number (`16`, `0x10u`)
            // or a CONSTANT.
            Clause::Regex {
                metavariable: "$SIZE".into(),
                regex: NAMES_SOMETHING.into(),
            },
        ],
        Passes::AllocSize { param } => vec![Clause::Either(vec![
            format!("{name}({}$N * $M, ...)", before(param)),
            format!("{name}({}$N + $M, ...)", before(param)),
        ])],
        Passes::Arguments => vec![Clause::Pattern(format!("{name}(...)"))],
    };
    let what = match wrapper.passed.as_slice() {
        [] => "parameters".to_string(),
        names => format!("`{}`", names.join("`, `")),
    };
    RuleSpec {
        id: rule_id(&format!("wraps-{}-{name}", wrapper.callee), MAX_ID_CHARS),
        message: format!(
            "{name}() passes its {what} to {}() ({}:{}). {}",
            wrapper.callee, wrapper.file, wrapper.call_line, wrapper.why
        ),
        severity: wrapper.severity.into(),
        clauses,
    }
}

#[cfg(test)]
#[path = "wrappers_tests.rs"]
mod tests;
