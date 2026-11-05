//! Calls to C library and OS functions that are easy to misuse: unbounded
//! copies, shell commands, predictable temp files, and the like.

use super::{Finding, Severity};
use crate::model::CodeGraph;

pub const TOOL: &str = "slurcetrail";

struct Rule {
    names: &'static [&'static str],
    severity: Severity,
    cwe: u32,
    message: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        names: &["gets", "_getws", "getwd"],
        severity: Severity::High,
        cwe: 242,
        message: "Reads with no length limit: always overflowable. Use fgets / getcwd.",
    },
    Rule {
        names: &[
            "strcpy", "wcscpy", "_tcscpy", "_mbscpy", "lstrcpy", "lstrcpyA", "lstrcpyW", "StrCpy",
            "stpcpy",
        ],
        severity: Severity::High,
        cwe: 120,
        message: "Copies without the destination's size: overflows if the source is longer.",
    },
    Rule {
        names: &[
            "strcat", "wcscat", "_tcscat", "_mbscat", "lstrcat", "lstrcatA", "lstrcatW", "StrCat",
        ],
        severity: Severity::High,
        cwe: 120,
        message: "Appends without the destination's size: overflows if the result is longer.",
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
        message: "Formats into a buffer of unknown size: overflows. Use snprintf.",
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
        message: "Runs a shell command: command injection if any part comes from input.",
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
        message: "Starts a program: check where its path and arguments come from.",
    },
    Rule {
        names: &[
            "scanf", "fscanf", "sscanf", "vscanf", "vfscanf", "vsscanf", "wscanf", "swscanf",
        ],
        severity: Severity::Medium,
        cwe: 120,
        message: "%s or %[ without a width overflows the destination.",
    },
    Rule {
        names: &["strncpy", "wcsncpy", "_tcsncpy"],
        severity: Severity::Medium,
        cwe: 170,
        message: "Doesn't NUL-terminate when the source fills the buffer.",
    },
    Rule {
        names: &["strncat", "wcsncat", "_tcsncat"],
        severity: Severity::Medium,
        cwe: 193,
        message: "The size is the space left, not the buffer size: a common off-by-one.",
    },
    Rule {
        names: &["alloca", "_alloca"],
        severity: Severity::Medium,
        cwe: 770,
        message: "Unchecked stack allocation: a large size exhausts the stack.",
    },
    Rule {
        names: &["mktemp", "_mktemp", "tmpnam", "tempnam", "_tempnam"],
        severity: Severity::Medium,
        cwe: 377,
        message: "Predictable temporary file name: race / symlink attack. Use mkstemp.",
    },
    Rule {
        names: &["access", "_access"],
        severity: Severity::Medium,
        cwe: 367,
        message: "Check-then-use race: open the file and check the handle instead.",
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
        message: "Check the size against the destination buffer.",
    },
    Rule {
        names: &["rand", "random", "srand", "drand48", "lrand48"],
        severity: Severity::Low,
        cwe: 338,
        message: "Not cryptographically secure: don't use for keys, tokens, or nonces.",
    },
    Rule {
        names: &["atoi", "atol", "atoll", "atof"],
        severity: Severity::Low,
        cwe: 190,
        message: "No error detection; out-of-range input is undefined. Use strtol.",
    },
    Rule {
        names: &["strtok", "wcstok"],
        severity: Severity::Low,
        cwe: 676,
        message: "Not reentrant: unsafe across threads or nested loops. Use strtok_r.",
    },
];

/// Every call in `graph` to a risky function, by file and line.
pub fn risky_calls(graph: &CodeGraph) -> Vec<Finding> {
    let mut findings: Vec<Finding> = graph
        .external_calls
        .iter()
        .flat_map(|(file, calls)| {
            calls.iter().filter_map(move |call| {
                let name = library_name(&call.name)?;
                let rule = RULES.iter().find(|r| r.names.contains(&name))?;
                Some(Finding {
                    tool: TOOL.into(),
                    rule: name.into(),
                    severity: rule.severity,
                    message: rule.message.into(),
                    cwe: Some(rule.cwe),
                    file: file.clone(),
                    line: call.span.line,
                    symbol: Some(call.caller),
                })
            })
        })
        .collect();
    findings.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    findings
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
    use crate::index::{build_graph, Extractor, SourceFile};

    fn findings_in(source: &str) -> Vec<Finding> {
        let facts = Extractor::new()
            .expect("grammar")
            .extract(source)
            .expect("parse");
        risky_calls(&build_graph(vec![SourceFile {
            path: "a.c".into(),
            contents: source.into(),
            facts,
        }]))
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
            "void f(char *d, const char *s, char *cmd) {\n\
             \x20 strcpy(d, s);\n\
             \x20 system(cmd);\n\
             \x20 strncpy(d, s, 4);\n\
             \x20 std::memcpy(d, s, 4);\n\
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
        assert!(!strcpy.message.is_empty());
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
            assert!(!rule.names.is_empty() && !rule.message.is_empty() && rule.cwe > 0);
        }
    }
}
