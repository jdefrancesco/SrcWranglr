//! Calls to C library and OS functions that are easy to misuse: unbounded
//! copies, shell commands, predictable temp files, and the like.

use super::dangerous::{DangerousFunction, DangerousList};
use super::heuristics::{assess, Assessment, Verdict};
use super::{Finding, Severity};
use crate::model::{CodeGraph, EdgeKind, ExternalCall, OccurrenceRole};

pub const TOOL: &str = "SrcWranglr";
/// What findings from the user's dangerous-functions list say found them.
pub const LIST_TOOL: &str = "your dangerous-functions list";

struct Rule {
    names: &'static [&'static str],
    severity: Severity,
    cwe: u32,
    why: &'static str,
    fix: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        names: &["gets", "_getws", "getwd"],
        severity: Severity::High,
        cwe: 242,
        why: "Reads input with no length limit, so any long enough input overflows the buffer.",
        fix: "Use fgets (or getcwd with a size).",
    },
    Rule {
        names: &[
            "strcpy", "wcscpy", "_tcscpy", "_mbscpy", "lstrcpy", "lstrcpyA", "lstrcpyW", "StrCpy",
            "stpcpy",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Copies without knowing the destination's size: overflows if the source is longer.",
        fix: "Use snprintf or strlcpy with sizeof(dest), or check strlen(src) first.",
    },
    Rule {
        names: &[
            "strcat", "wcscat", "_tcscat", "_mbscat", "lstrcat", "lstrcatA", "lstrcatW", "StrCat",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Appends without knowing the destination's size: overflows if the result is longer.",
        fix: "Use snprintf or strlcat with sizeof(dest).",
    },
    Rule {
        names: &[
            "sprintf",
            "vsprintf",
            "wsprintf",
            "wsprintfA",
            "wsprintfW",
            "_stprintf",
        ],
        severity: Severity::High,
        cwe: 120,
        why: "Formats into a buffer of unknown size: a long argument overflows it.",
        fix: "Use snprintf with sizeof(dest) and check the result.",
    },
    Rule {
        names: &[
            "system",
            "_wsystem",
            "popen",
            "_popen",
            "_wpopen",
            "WinExec",
            "ShellExecute",
            "ShellExecuteA",
            "ShellExecuteW",
        ],
        severity: Severity::High,
        cwe: 78,
        why: "Runs a shell command: if any part comes from input, it can inject commands.",
        fix: "Use execv/posix_spawn with an argument list, or validate strictly.",
    },
    Rule {
        names: &[
            "execl",
            "execlp",
            "execle",
            "execv",
            "execvp",
            "execvpe",
            "CreateProcess",
            "CreateProcessA",
            "CreateProcessW",
        ],
        severity: Severity::Medium,
        cwe: 78,
        why: "Starts a program: a path or argument from input can run something else.",
        fix: "Use absolute paths and fixed arguments; never pass input unchecked.",
    },
    Rule {
        names: &[
            "scanf", "fscanf", "sscanf", "vscanf", "vfscanf", "vsscanf", "wscanf", "swscanf",
        ],
        severity: Severity::Medium,
        cwe: 120,
        why: "%s or %[ without a field width writes past the destination.",
        fix: "Give every %s / %[ a width, e.g. %63s for char buf[64].",
    },
    Rule {
        names: &["strncpy", "wcsncpy", "_tcsncpy"],
        severity: Severity::Medium,
        cwe: 170,
        why: "Leaves the destination without a NUL terminator when the source fills it.",
        fix: "Set dest[size - 1] = '\\0' after the call, or use strlcpy / snprintf.",
    },
    Rule {
        names: &["strncat", "wcsncat", "_tcsncat"],
        severity: Severity::Medium,
        cwe: 193,
        why: "The size is the space left (minus the NUL), not the buffer size: a common overflow.",
        fix: "Pass sizeof(dest) - strlen(dest) - 1, or use strlcat / snprintf.",
    },
    Rule {
        names: &["alloca", "_alloca"],
        severity: Severity::Medium,
        cwe: 770,
        why: "Allocates on the stack with no limit: a large size overflows the stack.",
        fix: "Bound the size, or allocate on the heap.",
    },
    Rule {
        names: &["mktemp", "_mktemp", "tmpnam", "tempnam", "_tempnam"],
        severity: Severity::Medium,
        cwe: 377,
        why: "The temporary file name is predictable: another process can create or link it first.",
        fix: "Use mkstemp / tmpfile.",
    },
    Rule {
        names: &["access", "_access"],
        severity: Severity::Medium,
        cwe: 367,
        why: "The file can change between this check and its use (TOCTOU).",
        fix: "Open the file and check the open handle (fstat) instead.",
    },
    Rule {
        names: &[
            "memcpy",
            "memmove",
            "wmemcpy",
            "wmemmove",
            "bcopy",
            "CopyMemory",
        ],
        severity: Severity::Low,
        cwe: 120,
        why: "Copies a byte count the destination may not hold.",
        fix: "Make sure the size can't exceed the destination's size.",
    },
    Rule {
        names: &["rand", "random", "srand", "drand48", "lrand48"],
        severity: Severity::Low,
        cwe: 338,
        why: "Predictable: not suitable for keys, tokens, nonces, or anything security-relevant.",
        fix: "Use getrandom / arc4random / a CSPRNG.",
    },
    Rule {
        names: &["atoi", "atol", "atoll", "atof"],
        severity: Severity::Low,
        cwe: 190,
        why: "Can't report bad input, and out-of-range values are undefined behavior.",
        fix: "Use strtol / strtoul and check errno and the end pointer.",
    },
    Rule {
        names: &["strtok", "wcstok"],
        severity: Severity::Low,
        cwe: 676,
        why: "Keeps hidden state: breaks across threads or nested tokenizing loops.",
        fix: "Use strtok_r.",
    },
];

/// Every call in `graph` to a risky function, by file and line. Functions
/// on `list` are flagged as it says (or not at all, if it says "off");
/// other library calls are judged by their arguments where those tell
/// more, else by the function's built-in rule.
pub fn risky_calls(graph: &CodeGraph, list: &DangerousList) -> Vec<Finding> {
    let external = graph.external_calls.iter().flat_map(|(file, calls)| {
        calls.iter().filter_map(move |call| {
            let listed_finding = |entry: &DangerousFunction| {
                listed_call(entry, file, &call.name, call.extent, call.caller)
            };
            match listed(list, &call.name) {
                // Advice can raise a finding, never lower or silence one.
                Some(entry) if entry.is_advisory => {
                    more_severe(listed_finding(entry), judge(file, call))
                }
                Some(entry) => listed_finding(entry),
                None => judge(file, call),
            }
        })
    });
    let mut findings: Vec<Finding> = external.chain(listed_project_calls(graph, list)).collect();
    findings.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    findings
}

fn more_severe(a: Option<Finding>, b: Option<Finding>) -> Option<Finding> {
    match (a, b) {
        (Some(a), Some(b)) if b.severity < a.severity => Some(b),
        (a, b) => a.or(b),
    }
}

/// The list's entry for a call written `called` (also as a library name).
fn listed<'l>(list: &'l DangerousList, called: &str) -> Option<&'l DangerousFunction> {
    list.lookup(called)
        .or_else(|| library_name(called).and_then(|bare| list.lookup(bare)))
}

/// Calls to the project's own functions that are on `list`.
fn listed_project_calls<'g>(
    graph: &'g CodeGraph,
    list: &'g DangerousList,
) -> impl Iterator<Item = Finding> + 'g {
    graph
        .occurrences
        .iter()
        .flat_map(move |(file, occurrences)| {
            occurrences.iter().filter_map(move |o| {
                let OccurrenceRole::Reference(EdgeKind::Calls) = o.role else {
                    return None;
                };
                let callee = graph.symbols.get(&o.symbol)?;
                let entry = list.lookup(&callee.qualified_name)?;
                listed_call(entry, file, &callee.qualified_name, o.span, o.context?)
            })
        })
}

fn listed_call(
    entry: &DangerousFunction,
    file: &str,
    called: &str,
    at: crate::model::Span,
    caller: crate::model::SymbolId,
) -> Option<Finding> {
    Some(Finding {
        tool: LIST_TOOL.into(),
        rule: called.to_string(),
        severity: entry.severity?,
        message: entry.reason.clone(),
        fix: entry.fix.clone(),
        cwe: entry.cwe,
        file: file.to_string(),
        line: at.line,
        columns: Some((at.start, at.end)),
        symbol: Some(caller),
    })
}

fn judge(file: &str, call: &ExternalCall) -> Option<Finding> {
    let name = library_name(&call.name)?;
    let assessment = match assess(name, &call.args) {
        Verdict::Suppress => return None,
        Verdict::Flag(assessment) => Assessment {
            label: assessment.label,
            ..assessment
        },
        Verdict::Default => {
            let rule = RULES.iter().find(|r| r.names.contains(&name))?;
            Assessment {
                label: "",
                severity: rule.severity,
                cwe: rule.cwe,
                why: rule.why,
                fix: rule.fix,
            }
        }
    };
    let rule = match assessment.label {
        "" => name.to_string(),
        label => format!("{name}: {label}"),
    };
    Some(Finding {
        tool: TOOL.into(),
        rule,
        severity: assessment.severity,
        message: assessment.why.into(),
        fix: Some(assessment.fix.into()),
        cwe: Some(assessment.cwe),
        file: file.to_string(),
        line: call.extent.line,
        columns: Some((call.extent.start, call.extent.end)),
        symbol: Some(call.caller),
    })
}

/// `strcpy`, `::strcpy` and `std::strcpy` all name the library function;
/// other qualified names don't.
fn library_name(called: &str) -> Option<&str> {
    let bare = called.strip_prefix("::").unwrap_or(called);
    let bare = bare.strip_prefix("std::").unwrap_or(bare);
    (!bare.contains("::")).then_some(bare)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::dangerous::DangerousList;
    use crate::index::{build_graph, Extractor, SourceFile};

    fn findings_in(source: &str) -> Vec<Finding> {
        findings_with(source, &DangerousList::default())
    }

    fn findings_with(source: &str, list: &DangerousList) -> Vec<Finding> {
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        risky_calls(
            &build_graph(vec![SourceFile {
                path: "a.c".into(),
                contents: source.into(),
                facts,
            }]),
            list,
        )
    }

    fn summary(findings: &[Finding]) -> Vec<(&str, Severity, u32)> {
        findings
            .iter()
            .map(|f| (f.rule.as_str(), f.severity, f.line))
            .collect()
    }

    #[test]
    fn flags_risky_library_calls_by_severity() {
        let findings = findings_in(
            "void f(char *d, const char *s, char *cmd, int n) {\n\
             \x20 strcpy(d, s);\n\
             \x20 system(cmd);\n\
             \x20 strncpy(d, s, 4);\n\
             \x20 std::memcpy(d, s, n);\n\
             \x20 puts(s);\n\
             }\n",
        );
        assert_eq!(
            summary(&findings),
            [
                ("strcpy", Severity::High, 2),
                ("system", Severity::High, 3),
                ("strncpy", Severity::Medium, 4),
                ("memcpy", Severity::Low, 5),
            ]
        );
        let strcpy = &findings[0];
        assert_eq!(strcpy.tool, TOOL);
        assert_eq!(strcpy.cwe, Some(120));
        assert_eq!(strcpy.file, "a.c");
        assert!(strcpy.symbol.is_some());
        assert!(!strcpy.message.is_empty() && strcpy.fix.is_some());
        // `strcpy(d, s)` spans columns 2..14 of line 2.
        assert_eq!(strcpy.columns, Some((2, 14)));
    }

    #[test]
    fn arguments_sharpen_or_silence_the_verdict() {
        let findings = findings_in(
            "void f(char *d, const char *s, char *fmt, unsigned n) {\n\
             \x20 printf(fmt);\n\
             \x20 sscanf(s, \"%d\", &n);\n\
             \x20 memcpy(d, s, strlen(s));\n\
             \x20 memcpy(d, s, 8);\n\
             \x20 strcpy(d, \"ok\");\n\
             }\n",
        );
        assert_eq!(
            summary(&findings),
            [
                ("printf: non-literal format", Severity::High, 2),
                ("memcpy: size from the source", Severity::High, 4),
                ("memcpy: constant size", Severity::Low, 5),
                ("strcpy: fixed string", Severity::Low, 6),
            ]
        );
    }

    #[test]
    fn listed_functions_are_flagged_where_called_and_off_silences() {
        let list = DangerousList::parse(
            "[[function]]\nname = \"net::parse\"\nseverity = \"high\"\nreason = \"trusts len\"\n\
             [[function]]\nname = \"lib_decode\"\nseverity = \"medium\"\nreason = \"fuzzed crash\"\n\
             [[function]]\nname = \"strcpy\"\nseverity = \"off\"\nreason = \"reviewed\"\n",
        )
        .expect("list");
        let findings = findings_with(
            "namespace net { int parse(char *p) { return 0; } }\n\
             void f(char *d, char *s) {\n\
             \x20 net::parse(d);\n\
             \x20 lib_decode(s);\n\
             \x20 strcpy(d, s);\n\
             }\n",
            &list,
        );
        let summary: Vec<(&str, Severity, u32, &str)> = findings
            .iter()
            .map(|f| (f.rule.as_str(), f.severity, f.line, f.tool.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("net::parse", Severity::High, 3, LIST_TOOL),
                ("lib_decode", Severity::Medium, 4, LIST_TOOL),
            ]
        );
        assert_eq!(findings[0].message, "trusts len");
        assert!(findings[0].symbol.is_some() && findings[0].columns.is_some());
    }

    #[test]
    fn project_functions_with_risky_names_are_not_flagged() {
        let findings = findings_in(
            "void strcpy(char *d, const char *s) {}\n\
             void f(char *d) { strcpy(d, \"x\"); }\n",
        );
        assert!(findings.is_empty());
    }

    #[test]
    fn member_calls_with_risky_names_are_not_flagged() {
        let findings = findings_in("void f(Shell sh) { sh.system(\"ls\"); }\n");
        assert!(findings.is_empty());
    }

    #[test]
    fn qualified_names_outside_std_are_not_library_calls() {
        assert_eq!(library_name("::strcpy"), Some("strcpy"));
        assert_eq!(library_name("std::strcpy"), Some("strcpy"));
        assert_eq!(library_name("mylib::strcpy"), None);
    }

    #[test]
    fn every_rule_has_names_a_message_and_a_cwe() {
        assert!(!RULES.is_empty());
        for rule in RULES {
            assert!(!rule.names.is_empty() && rule.cwe > 0);
            assert!(!rule.why.is_empty() && !rule.fix.is_empty());
        }
    }
}
