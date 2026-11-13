//! Which functions are worth fuzzing, and how hard each is to harness.
//! Offline and explainable: every point comes with its reason.
//!
//! Worth: a name that says it handles input, risky calls inside, many
//! paths, a caller that reads input. Ease: a buffer and its length is
//! best; values built by hand, needing an object, or being `static` cost.

pub mod dictionary;
pub mod harness;
pub mod makefile;
pub mod params;
mod worth;

use std::cmp::Reverse;
use std::collections::HashSet;

use self::params::{is_control_name, is_path_name, plan, words, Fill};
use super::Finding;
use crate::model::{CodeGraph, FunctionMetrics, Param, Symbol, SymbolId, SymbolKind};

/// Functions scoring below this aren't listed, unless their worth alone
/// reaches `MIN_WORTH` (worth fuzzing even if hard to harness).
pub const MIN_SCORE: i32 = 4;
pub const MIN_WORTH: i32 = 6;
/// Ease points at or above which a harness is easy, and below which hard.
const EASY_FROM: i32 = 3;
const HARD_BELOW: i32 = 0;
/// Already entry points, or harnesses themselves.
const SKIPPED_NAMES: &[&str] = &[
    "main",
    "wmain",
    "_tmain",
    "WinMain",
    "wWinMain",
    "DllMain",
    "LLVMFuzzerTestOneInput",
    "LLVMFuzzerInitialize",
    "LLVMFuzzerCustomMutator",
    "LLVMFuzzerCustomCrossOver",
];
/// Name words of helpers that report rather than process input.
const HELPER_WORDS: &[&str] = &[
    "error", "err", "die", "fatal", "quit", "bye", "usage", "log", "warn", "warning", "debug",
    "trace", "fail", "abort", "panic", "assert", "perror", "help",
];
const BUFFER_AND_LENGTH_POINTS: i32 = 4;
const STRING_POINTS: i32 = 3;
const BUFFER_ONLY_POINTS: i32 = 2;
const NO_INPUT_POINTS: i32 = -2;
const NO_PARAMS_POINTS: i32 = -3;
const FILE_NAME_POINTS: i32 = -2;
const MANUAL_POINTS: i32 = -2;
const MAX_MANUAL_PENALTY: i32 = -6;
const METHOD_POINTS: i32 = -2;
const STATIC_POINTS: i32 = -1;
const VARIADIC_POINTS: i32 = -1;

/// Where a function sits, for calling it from a harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The class or struct it is a member of, if any.
    pub class: Option<String>,
    pub is_constructor: bool,
    /// A member that needs an object to be called on.
    pub needs_object: bool,
}

impl Placement {
    /// Where `symbol` sits, given the graph's `classes` (from
    /// [`class_names`]).
    pub fn of(classes: &HashSet<&str>, symbol: &Symbol, is_static: bool) -> Self {
        let (scope, leaf) = split_name(&symbol.qualified_name);
        let class = scope
            .filter(|scope| symbol.kind == SymbolKind::Method || classes.contains(scope))
            .map(str::to_string);
        let is_constructor = class.as_deref().is_some_and(|c| split_name(c).1 == leaf);
        Self {
            needs_object: class.is_some() && !is_static && !is_constructor,
            class,
            is_constructor,
        }
    }
}

/// Qualified names of the classes and structs in `graph`.
pub fn class_names(graph: &CodeGraph) -> HashSet<&str> {
    graph
        .symbols
        .values()
        .filter(|s| matches!(s.kind, SymbolKind::Class | SymbolKind::Struct))
        .map(|s| s.qualified_name.as_str())
        .collect()
}

/// `a::B::run` → (`Some("a::B")`, `run`).
pub fn split_name(qualified: &str) -> (Option<&str>, &str) {
    match qualified.rsplit_once("::") {
        Some((scope, leaf)) => (Some(scope), leaf),
        None => (None, qualified),
    }
}

/// A function worth a fuzz harness, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzTarget {
    pub symbol: SymbolId,
    pub name: String,
    pub file: String,
    pub line: u32,
    pub score: i32,
    /// The part of the score saying it's worth fuzzing (the rest is ease).
    pub worth: i32,
    pub ease: Ease,
    /// What the score is made of, worth first, then ease.
    pub reasons: Vec<Reason>,
}

/// Points for or against a function, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub points: i32,
    pub text: String,
}

/// How much work a harness for it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ease {
    Easy,
    Moderate,
    Hard,
}

impl Ease {
    pub fn label(self) -> &'static str {
        match self {
            Ease::Easy => "easy",
            Ease::Moderate => "moderate",
            Ease::Hard => "hard",
        }
    }
}

/// Every function definition in `graph` worth fuzzing, best first.
/// `findings` (risky calls and tool results) count toward each function's
/// worth.
pub fn fuzz_targets(graph: &CodeGraph, findings: &[Finding]) -> Vec<FuzzTarget> {
    let worth = worth::Worth::new(graph, findings);
    let classes = class_names(graph);
    let mut targets: Vec<FuzzTarget> = graph
        .functions
        .iter()
        .filter_map(|f| assess(graph, &worth, &classes, f))
        .filter(|t| t.score >= MIN_SCORE || t.worth >= MIN_WORTH)
        .collect();
    targets.sort_by(|a, b| {
        (Reverse(a.score), a.ease, &a.name, &a.file, a.line).cmp(&(
            Reverse(b.score),
            b.ease,
            &b.name,
            &b.file,
            b.line,
        ))
    });
    targets
}

fn assess(
    graph: &CodeGraph,
    worth: &worth::Worth,
    classes: &HashSet<&str>,
    f: &FunctionMetrics,
) -> Option<FuzzTarget> {
    let symbol = graph.symbols.get(&f.symbol)?;
    if is_skipped(split_name(&symbol.qualified_name).1) {
        return None;
    }
    let placement = Placement::of(classes, symbol, f.signature.is_static);
    let fills = plan(&f.signature.params);
    let input = best_input(&f.signature.params, &fills);
    let takes_input = input.is_some();
    let ease = ease_reasons(f, &fills, input, &placement);
    let ease_points: i32 = ease.iter().map(|r| r.points).sum();
    let worth_reasons = worth.reasons(f, &symbol.qualified_name, takes_input);
    let worth_points: i32 = worth_reasons.iter().map(|r| r.points).sum();
    let reasons: Vec<Reason> = worth_reasons.into_iter().chain(ease).collect();
    Some(FuzzTarget {
        symbol: f.symbol,
        name: symbol.qualified_name.clone(),
        file: f.file.clone(),
        line: f.line,
        score: reasons.iter().map(|r| r.points).sum(),
        worth: worth_points,
        ease: if ease_points >= EASY_FROM {
            Ease::Easy
        } else if ease_points >= HARD_BELOW {
            Ease::Moderate
        } else {
            Ease::Hard
        },
        reasons,
    })
}

/// Entry points, harnesses, destructors, operators, and helpers that
/// report errors or log.
fn is_skipped(leaf: &str) -> bool {
    SKIPPED_NAMES.contains(&leaf)
        || leaf.starts_with('~')
        || is_operator(leaf)
        || words(leaf)
            .iter()
            .any(|w| HELPER_WORDS.contains(&w.as_str()))
}

/// `operator<<`, `operator new`, but not `operator_parse`.
fn is_operator(leaf: &str) -> bool {
    leaf.strip_prefix("operator")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// How hard calling `f` from a harness is, given how its parameters are
/// filled and its best `input`.
fn ease_reasons(
    f: &FunctionMetrics,
    fills: &[Fill],
    input: Option<Reason>,
    placement: &Placement,
) -> Vec<Reason> {
    let params = &f.signature.params;
    let paths: Vec<usize> = (0..fills.len())
        .filter(|&i| fills[i].is_input() && is_path(params, i))
        .collect();
    let mut reasons: Vec<Reason> = input.into_iter().collect();
    reasons.extend(paths.iter().map(|&i| {
        reason(
            FILE_NAME_POINTS,
            format!(
                "takes a file name ({}), not content: the harness must write the input to a file",
                param_name(params, i)
            ),
        )
    }));
    if reasons.is_empty() {
        reasons.push(if params.is_empty() {
            reason(
                NO_PARAMS_POINTS,
                "takes no parameters: its input comes from elsewhere".into(),
            )
        } else {
            reason(NO_INPUT_POINTS, "takes no buffer or string".into())
        });
    }
    reasons.extend(manual_reason(params, fills));
    if let (Some(class), true) = (&placement.class, placement.needs_object) {
        reasons.push(reason(
            METHOD_POINTS,
            format!("a method of {class}: needs an object to call it on"),
        ));
    }
    if f.signature.is_static && placement.class.is_none() {
        reasons.push(reason(
            STATIC_POINTS,
            format!("static: the harness has to #include {}", f.file),
        ));
    }
    if f.signature.is_variadic {
        reasons.push(reason(
            VARIADIC_POINTS,
            "takes variable arguments (...)".into(),
        ));
    }
    reasons
}

fn is_path(params: &[Param], i: usize) -> bool {
    params[i].name.as_deref().is_some_and(is_path_name)
}

/// The best fuzz input among the parameters, leaving out file names and
/// unpaired control strings (formats, modes, versions).
fn best_input(params: &[Param], fills: &[Fill]) -> Option<Reason> {
    let is_control = |i: usize| params[i].name.as_deref().is_some_and(is_control_name);
    fills
        .iter()
        .enumerate()
        .filter(|(i, fill)| {
            let is_paired = matches!(
                fill,
                Fill::Bytes {
                    length: Some(_),
                    ..
                }
            );
            !is_path(params, *i) && (is_paired || !is_control(*i))
        })
        .filter_map(|(i, fill)| {
            let name = param_name(params, i);
            let (points, text) = match fill {
                Fill::Bytes {
                    length: Some(length),
                    ..
                } => (
                    BUFFER_AND_LENGTH_POINTS,
                    format!(
                        "takes a buffer and its length ({name}, {})",
                        param_name(params, *length)
                    ),
                ),
                Fill::Bytes { length: None, .. } => (
                    BUFFER_ONLY_POINTS,
                    format!("takes a buffer ({name}) but no length"),
                ),
                Fill::CString { .. } | Fill::Text => {
                    (STRING_POINTS, format!("takes a string ({name})"))
                }
                Fill::ByteVector { .. } => (STRING_POINTS, format!("takes a byte vector ({name})")),
                _ => return None,
            };
            let is_read_only = matches!(
                fill,
                Fill::CString { is_const: true } | Fill::Text | Fill::Bytes { is_const: true, .. }
            );
            Some((points, is_read_only, Reverse(i), text))
        })
        .max()
        .map(|(points, _, _, text)| reason(points, text))
}

/// One reason for all the values a harness has to build by hand.
fn manual_reason(params: &[Param], fills: &[Fill]) -> Option<Reason> {
    let manual: Vec<String> = fills
        .iter()
        .enumerate()
        .filter(|(_, fill)| **fill == Fill::Manual)
        .map(|(i, _)| {
            let ty = &params[i].type_text;
            let ty = if ty.is_empty() {
                "an unreadable type"
            } else {
                ty
            };
            format!("{ty} ({})", param_name(params, i))
        })
        .collect();
    if manual.is_empty() {
        return None;
    }
    let count = i32::try_from(manual.len()).unwrap_or(i32::MAX);
    Some(reason(
        MANUAL_POINTS.saturating_mul(count).max(MAX_MANUAL_PENALTY),
        format!("needs values built by hand: {}", manual.join(", ")),
    ))
}

/// A parameter's name, or its position if it has none.
pub fn param_name(params: &[Param], i: usize) -> String {
    params[i]
        .name
        .clone()
        .unwrap_or_else(|| format!("parameter {}", i + 1))
}

fn reason(points: i32, text: String) -> Reason {
    Reason { points, text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::dangerous::DangerousList;
    use crate::analysis::risky::risky_calls;
    use crate::index::graph_of_files;

    fn targets_of(sources: &[(&str, &str)]) -> Vec<FuzzTarget> {
        let graph = graph_of_files(sources);
        let findings = risky_calls(&graph, &DangerousList::default());
        fuzz_targets(&graph, &findings)
    }

    fn target<'t>(targets: &'t [FuzzTarget], name: &str) -> &'t FuzzTarget {
        targets
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} not listed in {targets:#?}"))
    }

    fn has_reason(target: &FuzzTarget, needle: &str) -> bool {
        target.reasons.iter().any(|r| r.text.contains(needle))
    }

    #[test]
    fn a_parser_taking_a_buffer_and_length_is_an_easy_target() {
        let targets = targets_of(&[(
            "png.c",
            "int parse_chunk(const uint8_t *buf, size_t len) { return buf[0] + (int)len; }",
        )]);
        let parse = target(&targets, "parse_chunk");
        assert_eq!(parse.ease, Ease::Easy);
        assert!(has_reason(parse, "parse"), "{parse:#?}");
        assert!(
            has_reason(parse, "buffer and its length (buf, len)"),
            "{parse:#?}"
        );
        assert_eq!(
            parse.score,
            parse.reasons.iter().map(|r| r.points).sum::<i32>()
        );
        assert_eq!((parse.file.as_str(), parse.line), ("png.c", 1));
    }

    #[test]
    fn functions_without_input_or_worth_are_left_out() {
        let targets = targets_of(&[(
            "m.c",
            "int add(int a, int b) { return a + b; }\nvoid tick(void) {}",
        )]);
        assert!(targets.is_empty(), "{targets:#?}");
    }

    #[test]
    fn entry_points_and_existing_harnesses_are_left_out() {
        let targets = targets_of(&[(
            "m.c",
            "int main(int argc, char **argv) { return 0; }\n\
             int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) { return 0; }",
        )]);
        assert!(targets.is_empty(), "{targets:#?}");
    }

    #[test]
    fn risky_calls_inside_add_worth() {
        let targets = targets_of(&[(
            "s.c",
            "void copy_name(char *dst, const char *src) { strcpy(dst, src); }\n\
             void show_name(char *dst, const char *src) { puts(src); }",
        )]);
        let copy = target(&targets, "copy_name");
        assert!(has_reason(copy, "risky call"), "{copy:#?}");
        let show_score = targets
            .iter()
            .find(|t| t.name == "show_name")
            .map_or(i32::MIN, |t| t.score);
        assert!(copy.score > show_score);
    }

    #[test]
    fn a_caller_that_reads_input_adds_worth() {
        let targets = targets_of(&[(
            "net.c",
            "int handle_packet(const char *p, int n) { return n; }\n\
             void serve(int fd) { char b[64]; int n = recv(fd, b, 64, 0); handle_packet(b, n); }",
        )]);
        let handle = target(&targets, "handle_packet");
        assert!(
            has_reason(handle, "input reaches it: recv() in serve » handle_packet"),
            "{handle:#?}"
        );
    }

    #[test]
    fn input_from_further_up_the_call_graph_adds_worth() {
        let targets = targets_of(&[(
            "deep.c",
            "int parse_record(const char *p, int n) { return n; }\n\
             int dispatch(const char *p, int n) { return parse_record(p, n); }\n\
             void serve(int fd) { char b[64]; int n = recv(fd, b, 64, 0); dispatch(b, n); }",
        )]);
        let parse = target(&targets, "parse_record");
        assert!(
            has_reason(parse, "recv() in serve » dispatch » parse_record"),
            "{parse:#?}"
        );
    }

    #[test]
    fn high_complexity_adds_worth() {
        let branches = "if (s[0]) n++; ".repeat(25);
        let source = format!("int scan_line(const char *s) {{ int n = 0; {branches} return n; }}");
        let targets = targets_of(&[("c.c", &source)]);
        assert!(has_reason(target(&targets, "scan_line"), "complexity 26"));
    }

    #[test]
    fn methods_static_functions_and_hand_built_values_cost_ease() {
        let targets = targets_of(&[(
            "p.cpp",
            "struct ctx;\n\
             class Decoder { public: int decode(const uint8_t *p, size_t n) { return 0; } };\n\
             static int decode_frame(const uint8_t *p, size_t n) { return 0; }\n\
             int decode_with(struct ctx *c, const uint8_t *p, size_t n) { return 0; }",
        )]);
        assert!(has_reason(
            target(&targets, "Decoder::decode"),
            "needs an object"
        ));
        assert!(has_reason(target(&targets, "decode_frame"), "static"));
        let with = target(&targets, "decode_with");
        assert!(has_reason(with, "struct ctx *"), "{with:#?}");
        let free_score = |name| target(&targets, name).score;
        assert!(free_score("decode_with") < free_score("decode_frame") + 2);
    }

    #[test]
    fn a_file_name_parameter_is_not_fuzz_input() {
        let targets = targets_of(&[(
            "f.c",
            "int parse_file(const char *path) { return 0; }\n\
             int parse_text(const char *text) { return 0; }",
        )]);
        let text = target(&targets, "parse_text");
        let file_score = targets
            .iter()
            .find(|t| t.name == "parse_file")
            .map_or(i32::MIN, |t| t.score);
        assert!(text.score > file_score);
        if let Some(file) = targets.iter().find(|t| t.name == "parse_file") {
            assert!(has_reason(file, "file name (path)"), "{file:#?}");
        }
    }

    #[test]
    fn error_and_logging_helpers_are_left_out() {
        let targets = targets_of(&[(
            "e.c",
            "void fatal_error(const char *msg) {}\n\
             void usage(const char *prog) {}\n\
             void log_packet(const uint8_t *p, size_t n) {}\n\
             void serve(int fd) { char b[64]; read(fd, b, 64); fatal_error(b); usage(b); log_packet(b, 64); }",
        )]);
        assert!(targets.is_empty(), "{targets:#?}");
    }

    #[test]
    fn a_format_string_is_not_fuzz_input() {
        let targets = targets_of(&[("f.c", "int emit_line(const char *fmt) { return 0; }")]);
        assert!(targets.is_empty(), "{targets:#?}");
    }

    #[test]
    fn a_reader_only_feeds_functions_that_take_input() {
        let targets = targets_of(&[(
            "r.c",
            "int parse_flags(int flags) { return flags; }\n\
             void serve(int fd) { char b[64]; read(fd, b, 64); parse_flags(b[0]); }",
        )]);
        assert!(targets.is_empty(), "{targets:#?}");
    }

    #[test]
    fn examples_and_tests_count_for_less() {
        let source = "int parse_chunk(const uint8_t *buf, size_t len) { return 0; }";
        let library = targets_of(&[("src/png.c", source)]);
        let example = targets_of(&[("examples/png.c", source)]);
        let score = |targets: &[FuzzTarget]| targets.first().map_or(i32::MIN, |t| t.score);
        assert!(score(&example) < score(&library));
        if let Some(target) = example.first() {
            assert!(has_reason(target, "examples/"), "{target:#?}");
        }
    }

    #[test]
    fn only_real_operators_are_skipped() {
        assert!(
            is_operator("operator<<") && is_operator("operator") && is_operator("operator new")
        );
        assert!(!is_operator("operator_parse") && !is_operator("operatorTable"));
    }

    #[test]
    fn worthy_targets_are_listed_even_when_hard() {
        let branches = "if (s->n) s->n++; ".repeat(55);
        let source = format!(
            "struct z {{ int n; }};\nint inflate_block(struct z *s, int flush) {{ {branches} return 0; }}"
        );
        let targets = targets_of(&[("i.c", &source)]);
        let inflate = target(&targets, "inflate_block");
        assert_eq!(inflate.ease, Ease::Hard);
        assert!(inflate.score < MIN_SCORE);
    }

    #[test]
    fn a_read_only_buffer_is_the_likelier_input() {
        let targets = targets_of(&[(
            "p.c",
            "int unpack(unsigned char *dest, unsigned long *destlen, \
             const unsigned char *source, unsigned long *sourcelen) { return 0; }",
        )]);
        let unpack = target(&targets, "unpack");
        assert!(
            has_reason(unpack, "takes a buffer and its length (source, sourcelen)"),
            "{unpack:#?}"
        );
    }

    #[test]
    fn best_targets_come_first() {
        let targets = targets_of(&[(
            "o.c",
            "int read_header(const char *s) { return 0; }\n\
             int parse_body(const uint8_t *buf, size_t len) { char d[8]; memcpy(d, buf, len); return 0; }",
        )]);
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["parse_body", "read_header"]);
        assert!(targets.windows(2).all(|w| w[0].score >= w[1].score));
    }
}
