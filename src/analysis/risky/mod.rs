//! Calls to C library, OS and kernel functions that are easy to misuse:
//! unbounded copies, shell commands, predictable temp files, data from user
//! space, weak crypto, and the like. Calls are judged by their arguments
//! where those tell more (`heuristics`), else by the function's rule here.
//! Library functions the project defines itself count too, unless their
//! names are generic (`is_checked_in_tree`).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::dangerous::{DangerousFunction, DangerousList};
use super::heuristics::{self, assess, judged_names, Assessment, Verdict};
use super::{Finding, Severity};
use crate::model::{CodeGraph, EdgeKind, ExternalCall, OccurrenceRole, Span};

pub const TOOL: &str = "SrcWranglr";
/// What findings from the user's dangerous-functions list say found them.
pub const LIST_TOOL: &str = "your dangerous-functions list";

mod crypto;
mod kernel;
mod libc;
mod windows;

struct Rule {
    names: &'static [&'static str],
    severity: Severity,
    cwe: u32,
    why: &'static str,
    fix: &'static str,
}

/// A family of rules, and whether calls to a project's own definitions of
/// its names are judged too.
struct RuleSet {
    rules: &'static [Rule],
    is_checked_in_tree: bool,
}

/// The built-in rules: the C library and POSIX, Windows, and the Linux
/// kernel, which kernels and libc copies define themselves; and weak
/// cryptography, which a project defining `MD5_Init` or `BF_set_key` is
/// more likely implementing (as bcrypt does) than misusing.
const RULE_SETS: [RuleSet; 4] = [
    RuleSet {
        rules: libc::RULES,
        is_checked_in_tree: true,
    },
    RuleSet {
        rules: windows::RULES,
        is_checked_in_tree: true,
    },
    RuleSet {
        rules: kernel::RULES,
        is_checked_in_tree: true,
    },
    RuleSet {
        rules: crypto::RULES,
        is_checked_in_tree: false,
    },
];

/// Names as likely to be a project's own, unrelated function as the
/// library one: the project's own `access()` or `warn()` isn't judged as
/// the library's would be.
const GENERIC_NAMES: &[&str] = &[
    "access", "_access", "system", "rand", "random", "srand", "err", "errx", "warn", "warnx",
];

/// Each function name's rule, and whether calls to the project's own
/// definition of it are judged.
fn rules_by_name() -> &'static HashMap<&'static str, (&'static Rule, bool)> {
    static BY_NAME: OnceLock<HashMap<&'static str, (&'static Rule, bool)>> = OnceLock::new();
    BY_NAME.get_or_init(|| {
        RULE_SETS
            .iter()
            .flat_map(|set| set.rules.iter().map(|rule| (rule, set.is_checked_in_tree)))
            .flat_map(|(rule, in_tree)| rule.names.iter().map(move |&name| (name, (rule, in_tree))))
            .collect()
    })
}

/// Whether calls to the project's own definition of `name` are judged as
/// calls to the library function would be. Kernels and libc copies define
/// `strcpy`, `sprintf` and `copy_from_user` themselves; a generic name
/// such as `access` is more likely the project's own, different function.
pub fn is_checked_in_tree(name: &str) -> bool {
    static CHECKED: OnceLock<HashSet<&'static str>> = OnceLock::new();
    // Looked up for every call to a project function while indexing.
    CHECKED
        .get_or_init(|| {
            let ruled = rules_by_name().keys().copied();
            ruled
                .chain(judged_names())
                .filter(|name| !GENERIC_NAMES.contains(name))
                .filter(|name| {
                    rules_by_name()
                        .get(name)
                        .is_none_or(|&(_, in_tree)| in_tree)
                })
                .collect()
        })
        .contains(name)
}

/// Every call in `graph` to a risky function, by file and line. Functions
/// on `list` are flagged as it says (or not at all, if it says "off");
/// other library calls are judged by their arguments where those tell
/// more, else by the function's built-in rule.
pub fn risky_calls(graph: &CodeGraph, list: &DangerousList) -> Vec<Finding> {
    let library = [&graph.external_calls, &graph.library_calls_in_tree]
        .into_iter()
        .flatten()
        .flat_map(|(file, calls)| calls.iter().map(move |call| (file, call)))
        .filter_map(|(file, call)| library_call(list, file, call));
    // The project's own functions with library names are judged above.
    let judged: HashSet<(&str, Span)> = graph
        .library_calls_in_tree
        .iter()
        .flat_map(|(file, calls)| calls.iter().map(move |c| (file.as_str(), c.span)))
        .collect();
    let mut findings: Vec<Finding> = library
        .chain(listed_project_calls(graph, list, &judged))
        .collect();
    findings.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    findings
}

/// The finding the built-in rules and argument checks make on `call` in
/// `file` (your dangerous-functions list aside).
pub fn built_in_finding(file: &str, call: &ExternalCall) -> Option<Finding> {
    judge(file, call)
}

/// Whether `name` is a library function the built-in rules know: a
/// project function of that name is its own copy of it, not a wrapper.
pub fn is_library_name(name: &str) -> bool {
    rules_by_name().contains_key(name)
        || judged_names().any(|judged| judged == name)
        || heuristics::format_argument(name).is_some()
}

/// The library function a call written `called` names, if it names one
/// by its bare name (`strcpy`, `::strcpy`, `std::strcpy`).
pub fn bare_library_name(called: &str) -> Option<&str> {
    library_name(called)
}

/// The finding on a call to a library function, as `list` or the
/// built-in rules and argument checks judge it.
fn library_call(list: &DangerousList, file: &str, call: &ExternalCall) -> Option<Finding> {
    let listed_finding =
        |entry: &DangerousFunction| listed_call(entry, file, &call.name, call.extent, call.caller);
    match listed(list, &call.name) {
        // Advice can raise a finding, never lower or silence one.
        Some(entry) if entry.is_advisory => more_severe(listed_finding(entry), judge(file, call)),
        Some(entry) => listed_finding(entry),
        None => judge(file, call),
    }
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

/// Calls to the project's own functions that are on `list`, except those
/// in `judged` (already judged as library calls).
fn listed_project_calls<'g>(
    graph: &'g CodeGraph,
    list: &'g DangerousList,
    judged: &'g HashSet<(&'g str, Span)>,
) -> impl Iterator<Item = Finding> + 'g {
    graph
        .occurrences
        .iter()
        .flat_map(move |(file, occurrences)| {
            occurrences.iter().filter_map(move |o| {
                let OccurrenceRole::Reference(EdgeKind::Calls) = o.role else {
                    return None;
                };
                if judged.contains(&(file.as_str(), o.span)) {
                    return None;
                }
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
            let (rule, _) = rules_by_name().get(name)?;
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
    fn the_projects_own_library_functions_are_judged_like_the_library() {
        // As in the Linux kernel, which defines these itself.
        let findings = findings_in(
            "char *strcpy(char *d, const char *s) { return d; }\n\
             unsigned long copy_from_user(void *to, const void *from, unsigned long n) \
             { return 0; }\n\
             void f(char *d, char *s, void *u) {\n\
             \x20 strcpy(d, \"x\");\n\
             \x20 strcpy(d, s);\n\
             \x20 copy_from_user(d, u, 8);\n\
             }\n",
        );
        assert_eq!(
            summary(&findings),
            [
                ("strcpy: fixed string", Severity::Low, 4),
                ("strcpy", Severity::High, 5),
                ("copy_from_user", Severity::Medium, 6),
            ]
        );
    }

    #[test]
    fn the_projects_own_functions_with_generic_names_are_not_flagged() {
        let findings = findings_in(
            "int access(int user) { return user; }\n\
             namespace net { char *strcpy(char *d, const char *s) { return d; } }\n\
             void f(char *d, char *s) { access(1); }\n\
             namespace net { void g(char *d, char *s) { strcpy(d, s); } }\n",
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn weak_crypto_counts_only_when_called_from_a_library() {
        let findings = findings_in(
            "static void BF_set_key(const char *key) {}\n\
             void hash(const char *pw) {\n\
             \x20 BF_set_key(pw);\n\
             \x20 const void *md = EVP_md5();\n\
             }\n",
        );
        assert_eq!(summary(&findings), [("EVP_md5", Severity::Low, 4)]);
        assert_eq!(findings[0].cwe, Some(328));
    }

    #[test]
    fn a_listed_library_function_the_project_defines_is_flagged_once() {
        let list = DangerousList::parse(
            "[[function]]\nname = \"strcpy\"\nseverity = \"medium\"\nreason = \"ours\"\n\
             [[function]]\nname = \"sprintf\"\nseverity = \"off\"\nreason = \"reviewed\"\n",
        )
        .expect("list");
        let findings = findings_with(
            "char *strcpy(char *d, const char *s) { return d; }\n\
             int sprintf(char *d, const char *f, ...) { return 0; }\n\
             void f(char *d, char *s) {\n\
             \x20 strcpy(d, s);\n\
             \x20 sprintf(d, \"%s\", s);\n\
             }\n",
            &list,
        );
        let summary: Vec<(&str, Severity, &str)> = findings
            .iter()
            .map(|f| (f.rule.as_str(), f.severity, f.tool.as_str()))
            .collect();
        assert_eq!(summary, [("strcpy", Severity::Medium, LIST_TOOL)]);
    }

    #[test]
    fn checks_in_tree_only_names_with_a_rule_or_an_argument_check() {
        for name in ["strcpy", "copy_from_user", "kmalloc", "printk", "lstrcpyA"] {
            assert!(is_checked_in_tree(name), "{name}");
        }
        let not_checked = [
            "access",
            "warn",
            "parse_header",
            "kfree",
            "MD5_Init",
            "BF_set_key",
        ];
        for name in not_checked {
            assert!(!is_checked_in_tree(name), "{name}");
        }
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
        for set in RULE_SETS {
            assert!(!set.rules.is_empty());
            for rule in set.rules {
                assert!(!rule.names.is_empty() && rule.cwe > 0);
                assert!(!rule.why.is_empty() && !rule.fix.is_empty());
            }
        }
    }

    #[test]
    fn every_name_is_an_identifier_in_one_rule_only() {
        let names: Vec<&str> = RULE_SETS
            .iter()
            .flat_map(|set| set.rules.iter())
            .flat_map(|rule| rule.names.iter().copied())
            .collect();
        let is_identifier = |name: &str| {
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        };
        for name in &names {
            assert!(is_identifier(name), "{name:?}");
        }
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "a name is in two rules");
        assert_eq!(rules_by_name().len(), names.len());
        assert!(names.len() > 250, "{} names", names.len());
    }
}
