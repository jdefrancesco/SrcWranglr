//! Where outside input comes in, and which calls a function that reads it
//! hands it on to: those passed what was read (the buffer `recv` read into,
//! the variable `getenv`'s result went to, `main`'s `argv`, …), or the
//! reading call itself. This is word matching on the call's argument text,
//! one step deep — not data-flow analysis.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::code::{calls, words, Code};
use crate::model::{CodeGraph, EdgeKind, ExternalCall, OccurrenceRole, SymbolId};

/// Where a library call puts what it reads.
#[derive(Debug, Clone, Copy)]
enum Lands {
    /// In the buffer passed as this argument.
    Arg(usize),
    /// In every argument from this one on (`scanf`'s targets).
    ArgsFrom(usize),
    /// In its return value.
    Returned,
}

/// Library calls that bring outside input into the program.
const SOURCES: &[(&str, Lands)] = &[
    ("recv", Lands::Arg(1)),
    ("recvfrom", Lands::Arg(1)),
    ("recvmsg", Lands::Arg(1)),
    ("read", Lands::Arg(1)),
    ("pread", Lands::Arg(1)),
    ("readv", Lands::Arg(1)),
    ("fread", Lands::Arg(0)),
    ("fgets", Lands::Arg(0)),
    ("fgetc", Lands::Returned),
    ("getc", Lands::Returned),
    ("getchar", Lands::Returned),
    ("getline", Lands::Arg(0)),
    ("getdelim", Lands::Arg(0)),
    ("scanf", Lands::ArgsFrom(1)),
    ("fscanf", Lands::ArgsFrom(2)),
    ("getenv", Lands::Returned),
    ("ReadFile", Lands::Arg(1)),
    ("WSARecv", Lands::Arg(1)),
    ("WSARecvFrom", Lands::Arg(1)),
    ("SSL_read", Lands::Arg(1)),
    ("BIO_read", Lands::Arg(1)),
    ("gzread", Lands::Arg(1)),
];
/// Entry points whose parameters are the command line.
const MAIN_NAMES: &[&str] = &["main", "wmain", "_tmain"];
/// How the command line is named as a source.
pub const COMMAND_LINE: &str = "the command line";
/// A function that reads input itself, in one file, and what it hands it to.
#[derive(Debug, Clone)]
pub(super) struct Reader {
    pub symbol: SymbolId,
    pub file: String,
    /// The first (by name) of the sources it reads from in that file.
    pub source: String,
    /// The functions it passes the input to there, in name order.
    pub hands_to: Vec<SymbolId>,
    /// Names the input is kept in there.
    names: HashSet<String>,
    /// Sources whose results it uses where it calls them.
    inline: HashSet<&'static str>,
}

impl Reader {
    /// Whether `code` (from this function, in this file, literals and
    /// comments blanked) uses the input.
    pub fn uses_input(&self, code: &str) -> bool {
        words(code).any(|w| self.names.contains(w))
            || self.inline.iter().any(|source| calls(code, source))
    }
}

/// One function's reading in one file, while it is gathered.
#[derive(Default)]
struct Reading {
    sources: BTreeSet<String>,
    /// Names the input is kept in.
    names: HashSet<String>,
    /// Sources whose results are used where they are called.
    inline: HashSet<&'static str>,
}

/// Where input comes in.
pub(super) struct Found {
    /// The functions that read it, per file, in name order.
    pub readers: Vec<Reader>,
    /// Every source, in name order.
    pub sources: Vec<String>,
    /// The readers' files, read as code.
    pub codes: HashMap<String, Code>,
}

pub(super) fn readers(graph: &CodeGraph) -> Found {
    let mut codes: HashMap<String, Code> = HashMap::new();
    let mut readings: HashMap<(SymbolId, String), Reading> = HashMap::new();
    for (file, calls) in &graph.external_calls {
        for call in calls {
            let Some((name, lands)) = source_of(call) else {
                continue;
            };
            let code = code_of(&mut codes, graph, file);
            let reading = readings.entry((call.caller, file.clone())).or_default();
            reading.sources.insert(format!("{name}()"));
            match lands {
                Lands::Arg(i) => reading
                    .names
                    .extend(call.args.get(i).into_iter().flat_map(|a| names_in(a))),
                Lands::ArgsFrom(i) => {
                    reading
                        .names
                        .extend(call.args.iter().skip(i).flat_map(|a| names_in(a)));
                }
                Lands::Returned => {
                    let target = code
                        .offset(call.span.line, call.span.start)
                        .and_then(|at| code.assigned_at(at));
                    match target {
                        Some(target) => {
                            reading.names.insert(target);
                        }
                        None => {
                            reading.inline.insert(name);
                        }
                    }
                }
            }
        }
    }
    for main in graph.functions.iter().filter(|f| is_main(graph, f.symbol)) {
        if main.signature.params.is_empty() {
            continue;
        }
        code_of(&mut codes, graph, &main.file);
        let arrays = main
            .signature
            .params
            .iter()
            .filter(|p| p.type_text.contains(['*', '[']))
            .filter_map(|p| p.name.clone());
        let reading = readings
            .entry((main.symbol, main.file.clone()))
            .or_default();
        reading.names.extend(arrays);
        reading.sources.insert(COMMAND_LINE.to_string());
    }
    let sources: BTreeSet<String> = readings
        .values()
        .flat_map(|r| r.sources.iter().cloned())
        .collect();
    let mut readers: Vec<Reader> = readings
        .into_iter()
        .filter_map(|((symbol, file), reading)| {
            let source = reading.sources.iter().next()?.clone();
            let hands_to = handed_to(graph, symbol, &file, &reading, codes.get(&file)?);
            Some(Reader {
                symbol,
                hands_to,
                file,
                source,
                names: reading.names,
                inline: reading.inline,
            })
        })
        .collect();
    readers.sort_by(|a, b| {
        (sort_key(graph, a.symbol), &a.file).cmp(&(sort_key(graph, b.symbol), &b.file))
    });
    Found {
        readers,
        sources: sources.into_iter().collect(),
        codes,
    }
}

/// `file` read as code, once.
fn code_of<'c>(codes: &'c mut HashMap<String, Code>, graph: &CodeGraph, file: &str) -> &'c Code {
    codes
        .entry(file.to_string())
        .or_insert_with(|| Code::of(graph.file_contents.get(file).map_or("", String::as_str)))
}

/// The input source a call is, if any: by bare name (or `std::` or
/// global), so `mylib::read` is not `read`.
fn source_of(call: &ExternalCall) -> Option<(&'static str, Lands)> {
    let (scope, name) = call
        .name
        .rsplit_once("::")
        .unwrap_or(("", call.name.as_str()));
    if !matches!(scope, "" | "std") {
        return None;
    }
    // C++'s `std::getline(stream, line)` reads into its second argument.
    if scope == "std" && name == "getline" {
        return Some(("getline", Lands::Arg(1)));
    }
    SOURCES.iter().find(|(source, _)| *source == name).copied()
}

fn is_main(graph: &CodeGraph, symbol: SymbolId) -> bool {
    graph
        .symbols
        .get(&symbol)
        .is_some_and(|s| MAIN_NAMES.contains(&s.qualified_name.as_str()))
}

/// The names written in an argument, literals left out:
/// `(char *)conn->buf + 4` → `conn`, `buf`.
fn names_in(arg: &str) -> Vec<String> {
    words(Code::of(arg).text()).map(str::to_string).collect()
}

/// What `caller` passes the input to in `file`, in name order.
fn handed_to(
    graph: &CodeGraph,
    caller: SymbolId,
    file: &str,
    reading: &Reading,
    code: &Code,
) -> Vec<SymbolId> {
    let mut called: Vec<SymbolId> = graph
        .occurrences
        .get(file)
        .into_iter()
        .flatten()
        .filter(|o| {
            o.role == OccurrenceRole::Reference(EdgeKind::Calls) && o.context == Some(caller)
        })
        .filter(|o| {
            let arguments = code
                .offset(o.span.line, o.span.end)
                .and_then(|at| code.arguments_at(at));
            arguments.is_some_and(|args| {
                words(args).any(|w| reading.names.contains(w))
                    || reading.inline.iter().any(|source| calls(args, source))
            })
        })
        .map(|o| o.symbol)
        .collect();
    called.sort_by(|a, b| sort_key(graph, *a).cmp(&sort_key(graph, *b)));
    called.dedup();
    called
}

pub(super) fn sort_key(graph: &CodeGraph, id: SymbolId) -> (&str, SymbolId) {
    let name = graph
        .symbols
        .get(&id)
        .map_or("", |s| s.qualified_name.as_str());
    (name, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_names_in_an_argument_but_not_in_its_strings() {
        assert_eq!(names_in("(char *)conn->buf + 4"), ["conn", "buf"]);
        assert!(names_in("\"buf\"").is_empty());
    }
}
