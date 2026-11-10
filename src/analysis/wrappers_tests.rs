use std::sync::atomic::AtomicBool;

use super::*;
use crate::index::graph_of_files;

const SOURCE: &str = "\
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <stdarg.h>
void log_msg(int level, const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  vprintf(fmt, ap);
  va_end(ap);
}
void copy_name(char *dst, const char *name) { strcpy(dst, name); }
void copy_n(void *d, const void *s, size_t n) { memcpy(d, s, n); }
void copy_fixed(void *d, const void *s) { memcpy(d, s, 16); }
void *xmalloc(size_t size) { return malloc(size); }
void local(void) { char b[8]; strcpy(b, getenv(\"X\")); }
void caller(char *u, size_t a, size_t b) {
  log_msg(1, u);
  log_msg(1, \"ok %s\", u);
  copy_name(u, u);
  copy_n(u, u, 4);
  copy_n(u, u, a);
  copy_n(u, u, sizeof(u));
  xmalloc(4);
  xmalloc(a * b);
  copy_n(u, u, a);
  copy_n(u, u, 0x10);
}
";

fn found() -> Vec<Wrapper> {
    let graph = graph_of_files(&[("w.c", SOURCE)]);
    wrappers(&graph, &AtomicBool::new(false)).expect("not stopped")
}

fn passes(name: &str) -> Option<Passes> {
    found()
        .into_iter()
        .find(|w| w.name == name)
        .map(|w| w.passes)
}

#[test]
fn finds_what_each_wrapper_passes_on() {
    assert_eq!(passes("log_msg"), Some(Passes::Format { param: 1 }));
    assert_eq!(passes("copy_name"), Some(Passes::Arguments));
    assert_eq!(passes("copy_n"), Some(Passes::CopySize { param: 2 }));
    assert_eq!(passes("xmalloc"), Some(Passes::AllocSize { param: 0 }));
}

#[test]
fn functions_passing_nothing_risky_on_are_not_wrappers() {
    assert_eq!(passes("copy_fixed"), None);
    assert_eq!(passes("local"), None);
    assert_eq!(passes("caller"), None);
}

#[test]
fn the_most_called_come_first_and_say_what_they_pass() {
    let all = found();
    assert_eq!(all[0].name, "copy_n");
    assert_eq!(all[0].callers, 5);
    assert_eq!(all[0].passed, ["d", "s", "n"]);
    assert_eq!(all[0].callee, "memcpy");
    let log = all.iter().find(|w| w.name == "log_msg").expect("log_msg");
    assert_eq!(log.passed, ["fmt"]);
    assert_eq!(log.severity, Severity::High);
}

#[test]
fn reading_parameters() {
    assert_eq!(parameter_name("const char *name").as_deref(), Some("name"));
    assert_eq!(parameter_name("int (*cb)(int)").as_deref(), Some("cb"));
    assert_eq!(parameter_name("char buf[64]").as_deref(), Some("buf"));
    assert_eq!(parameter_name("int x = 3").as_deref(), Some("x"));
    for unnamed in ["int", "size_t", "struct foo *", "unsigned long", ""] {
        assert_eq!(parameter_name(unnamed), None, "{unnamed}");
    }
    assert_eq!(without_cast("(char *)p"), "p");
    assert_eq!(without_cast("((p))"), "p");
    assert_eq!(without_cast("(size_t) n"), "n");
    assert_eq!(without_cast("n + 1"), "n + 1");
}

#[test]
fn rules_find_the_calls_worth_a_look() {
    let all = found();
    let log = all.iter().find(|w| w.name == "log_msg").expect("log_msg");
    let yaml = rules_for(&[log]);
    assert!(yaml.contains("  - id: wraps-vprintf-log-msg\n"), "{yaml}");
    assert!(yaml.contains("    severity: ERROR\n"), "{yaml}");
    assert!(
        yaml.contains("      - pattern: \"log_msg($X1, $FORMAT, ...)\"\n"),
        "{yaml}"
    );
    assert!(
        yaml.contains("      - pattern-not: \"log_msg($X1, \\\"...\\\", ...)\"\n"),
        "{yaml}"
    );
    assert!(
        yaml.contains("passes its `fmt` to vprintf() (w.c:8)"),
        "{yaml}"
    );
}

/// Runs the real semgrep on the rules: `cargo test -- --ignored`.
#[test]
#[ignore = "needs semgrep installed; slow"]
fn semgrep_finds_the_calls_worth_a_look() {
    let all = found();
    let refs: Vec<&Wrapper> = all.iter().collect();
    let yaml = rules_for(&refs);
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::write(dir.path().join("w.c"), SOURCE).expect("write");
    let rules = dir.path().join("rules.yaml");
    std::fs::write(&rules, &yaml).expect("rules");
    let (program, args) = crate::analysis::tools::Tool::Semgrep.command(
        &dir.path().join("w.c"),
        &[rules.to_string_lossy().into_owned()],
    );
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .expect("semgrep");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let found = crate::analysis::tools::Tool::Semgrep
        .parse(&stdout, "", dir.path())
        .unwrap_or_else(|e| panic!("{e}\n{yaml}"));
    let mut lines: Vec<u32> = found.iter().map(|f| f.line).collect();
    lines.sort_unstable();
    // log_msg(1, u); copy_name; copy_n(.., a) twice; xmalloc(a * b).
    assert_eq!(lines, [17, 19, 21, 24, 25], "{yaml}");
}

#[test]
fn comments_and_arrows_dont_shift_parameters() {
    let source = "\
#include <string.h>
void put(char *dst, /* dst, len) */ const char *src, // a, b
         size_t n) { memcpy(dst, src, n); }
";
    let graph = graph_of_files(&[("p.c", source)]);
    let found = wrappers(&graph, &AtomicBool::new(false)).expect("not stopped");
    let put = found.iter().find(|w| w.name == "put").expect("put");
    assert_eq!(put.passes, Passes::CopySize { param: 2 });
    assert_eq!(split_top_level("a, int (*f)(b->c, d), e").count(), 3);
}

#[test]
fn rules_get_ids_of_their_own() {
    let wrapper = |name: &str| Wrapper {
        name: name.into(),
        symbol: 1,
        file: "a.c".into(),
        line: 1,
        callee: "strcpy".into(),
        call_line: 1,
        passes: Passes::Arguments,
        passed: Vec::new(),
        callers: 0,
        severity: Severity::High,
        why: String::new(),
    };
    let (a, b) = (wrapper("copy_a"), wrapper("Copy_A"));
    let yaml = rules_for(&[&a, &b]);
    assert!(yaml.contains("  - id: wraps-strcpy-copy-a\n"), "{yaml}");
    assert!(yaml.contains("  - id: wraps-strcpy-copy-a-2\n"), "{yaml}");
}
