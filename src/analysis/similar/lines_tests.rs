//! Drafting from several lines, from several places, and exceptions.

use super::*;
use crate::analysis::semgrep_rule::ExceptionKind;

const SOURCE: &str = "\
void a(char *src, int n) {
  char *p = malloc(n);
  log_it(\"copying\");
  memcpy(p, src, n);
  free(p);
}
void b(char *s, int len) {
  char buf[16];
  if (len > sizeof(buf)) return;
  memcpy(buf, s, len);
}
void c(struct pkt *k, char *o) {
  char *q = malloc(k->len);
  memcpy(q, k->data, k->len);
}
void d(struct pkt *m, char *out) {
  memcpy(out, m->data, m->len);
}
void e(char *s) {
  char buf[16];
  memcpy(buf, s, sizeof(buf));
}
void f(char *s, int len) {
  char buf[16];
  len = MIN(len, sizeof(buf));
  memcpy(buf, s, len);
}
void g(char *x, char *y) {
  memmove(x, y, 4);
  printf(\"a %s\", x);
  printf(\"b %d\", y);
  send(x, y);
  send(x, y, 0);
}
";

fn from(examples: &[Example], allow_gaps: bool) -> Result<Draft, String> {
    let pairs: Vec<(Example, &str)> = examples.iter().map(|e| (e.clone(), SOURCE)).collect();
    draft_from(&pairs, allow_gaps)
}

fn shared(lines: &[u32]) -> String {
    let examples: Vec<Example> = lines.iter().map(|l| Example::line("a.c", *l)).collect();
    from(&examples, true).expect("draft").same_shape
}

#[test]
fn several_lines_make_a_sequence_that_follows_each_variable() {
    let draft = from(&[Example::lines("a.c", 2, 4)], true).expect("draft");
    assert_eq!(
        draft.same_shape,
        "char *$A = malloc($B);\n...\nlog_it(\"copying\");\n...\nmemcpy($A, $C, $B);"
    );
    let tight = from(&[Example::lines("a.c", 4, 2)], false).expect("draft");
    assert_eq!(
        tight.same_shape,
        "char *$A = malloc($B);\nlog_it(\"copying\");\nmemcpy($A, $C, $B);"
    );
    assert_eq!(draft.id, "like-malloc-2");
    assert!(
        draft
            .message
            .starts_with("Like a.c:2-4: char *p = malloc(n);"),
        "{}",
        draft.message
    );
    assert_eq!(draft.any_values, None);
}

#[test]
fn lines_holding_one_statement_or_function_draft_it_whole() {
    let call = from(&[Example::lines("a.c", 4, 4)], true).expect("draft");
    assert_eq!(call.same_shape, "memcpy($A, $B, $C)");
    let function = from(&[Example::lines("a.c", 16, 18)], true).expect("draft");
    assert_eq!(
        function.same_shape,
        "void $A(struct pkt *$B, char *$C) { memcpy($C, $B->data, $B->len); }"
    );
}

#[test]
fn lines_without_code_are_refused() {
    let source = "void f(void) {\n\n\n}\n";
    let error = draft_from(&[(Example::lines("a.c", 2, 3), source)], true).expect_err("none");
    assert!(error.contains("No code"), "{error}");
}

#[test]
fn several_places_keep_what_they_share() {
    assert_eq!(shared(&[4, 10, 14]), "memcpy($A, $B, $C)");
    // A variable used twice in each place stays one metavariable.
    assert_eq!(shared(&[14, 17]), "memcpy($A, $B->data, $B->len)");
    // Strings that differ become any string; numbers, a metavariable.
    assert_eq!(shared(&[30, 31]), "printf(\"...\", $A)");
    // Arguments only some places pass become `...`.
    assert_eq!(shared(&[32, 33]), "send($A, $B, ...)");
}

#[test]
fn places_sharing_only_their_shape_are_refused() {
    let examples = [Example::line("a.c", 4), Example::line("a.c", 29)];
    let error = from(&examples, true).expect_err("only shape");
    assert!(error.contains("match almost anything"), "{error}");
}

#[test]
fn places_of_different_kinds_are_refused() {
    let examples = [Example::line("a.c", 4), Example::line("a.c", 8)];
    let error = from(&examples, true).expect_err("call and declaration");
    assert!(error.contains("same kind"), "{error}");
}

#[test]
fn several_places_of_several_lines_keep_the_statements_they_share() {
    let examples = [Example::lines("a.c", 2, 4), Example::lines("a.c", 13, 14)];
    let draft = from(&examples, false).expect("draft");
    assert_eq!(
        draft.same_shape,
        "char *$A = malloc($B);\n...\nmemcpy($A, $C, $B);"
    );
    assert_eq!(draft.id, "like-malloc-2-places");
    assert!(draft.message.starts_with("Like 2 places, such as a.c:2-4"));
}

#[test]
fn a_check_becomes_an_exception_after_it_with_the_rules_names() {
    let draft = from(&[Example::line("a.c", 10)], true).expect("draft");
    assert_eq!(draft.same_shape, "memcpy($A, $B, $C)");
    let check =
        exception_from(SOURCE, "a.c", 9, &draft.names, draft.callee.as_deref()).expect("exception");
    assert_eq!(check.kind, ExceptionKind::After);
    assert_eq!(check.pattern, "if (<... $C > sizeof($A) ...>) ...");
}

#[test]
fn the_same_call_written_safely_is_an_exception_that_looks_like_it() {
    let draft = from(&[Example::line("a.c", 10)], true).expect("draft");
    let fixed = exception_from(SOURCE, "a.c", 21, &draft.names, draft.callee.as_deref())
        .expect("exception");
    assert_eq!(fixed.kind, ExceptionKind::LooksLike);
    assert_eq!(fixed.pattern, "memcpy($A, $B, sizeof($A))");
}

#[test]
fn other_statements_are_exceptions_after_them_and_new_names_get_new_letters() {
    let draft = from(&[Example::line("a.c", 10)], true).expect("draft");
    let clamp = exception_from(SOURCE, "a.c", 25, &draft.names, draft.callee.as_deref())
        .expect("exception");
    assert_eq!(clamp.kind, ExceptionKind::After);
    assert_eq!(clamp.pattern, "$C = MIN($C, sizeof($A));");
    let other = exception_from(SOURCE, "a.c", 13, &draft.names, draft.callee.as_deref())
        .expect("exception");
    assert_eq!(other.pattern, "char *$D = malloc($E->len);");
}

#[test]
fn a_rule_with_exceptions_lists_its_patterns() {
    let draft = from(&[Example::line("a.c", 10)], true).expect("draft");
    let check =
        exception_from(SOURCE, "a.c", 9, &draft.names, draft.callee.as_deref()).expect("exception");
    let yaml = draft.yaml(Looseness::SameShape, &[check]);
    assert!(
        yaml.contains("    patterns:\n      - pattern: \"memcpy($A, $B, $C)\"\n"),
        "{yaml}"
    );
    assert!(
        yaml.contains("      - pattern-not-inside: |\n          if (<... $C > sizeof($A) ...>) ...\n          ...\n"),
        "{yaml}"
    );
}

/// Runs the real semgrep on drafted rules: `cargo test -- --ignored`.
#[test]
#[ignore = "needs semgrep installed; slow"]
fn drafted_sequences_and_exceptions_find_what_they_say() {
    let draft = from(&[Example::line("a.c", 10)], true).expect("draft");
    let exceptions: Vec<_> = [9, 21]
        .iter()
        .map(|line| {
            exception_from(SOURCE, "a.c", *line, &draft.names, draft.callee.as_deref())
                .expect("exception")
        })
        .collect();
    let unguarded = draft.yaml(Looseness::SameShape, &exceptions);
    assert_eq!(semgrep_lines(&unguarded), [4, 14, 17, 26]);
    let sequence = from(&[Example::lines("a.c", 2, 4)], false).expect("draft");
    let tight = sequence.yaml(Looseness::SameShape, &[]);
    assert_eq!(semgrep_lines(&tight), [2]);
    let several = [Example::lines("a.c", 2, 4), Example::lines("a.c", 13, 14)];
    let shared = from(&several, false).expect("draft");
    assert_eq!(
        semgrep_lines(&shared.yaml(Looseness::SameShape, &[])),
        [2, 13]
    );
}

/// The lines `yaml`'s rules match in `SOURCE`.
pub(crate) fn semgrep_lines(yaml: &str) -> Vec<u32> {
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::write(dir.path().join("a.c"), SOURCE).expect("write");
    let rules = dir.path().join("rules.yaml");
    std::fs::write(&rules, yaml).expect("rules");
    let (program, args) = crate::analysis::tools::Tool::Semgrep.command(
        &dir.path().join("a.c"),
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
    lines.dedup();
    lines
}

#[test]
fn deeply_nested_places_are_unified_on_a_small_stack() {
    let depth = 3_000;
    let nested = |inner: &str| format!("f({}{inner}{});", "(".repeat(depth), ")".repeat(depth));
    let source = format!(
        "void a(void) {{\n  {}\n}}\nvoid b(void) {{\n  {}\n}}\n",
        nested("x"),
        nested("y")
    );
    // Half the main thread's stack on Windows, in a debug build.
    let drafted = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let places = [
                (Example::line("a.c", 2), source.as_str()),
                (Example::line("a.c", 5), source.as_str()),
            ];
            draft_from(&places, true).map(|d| d.same_shape)
        })
        .expect("thread")
        .join()
        .expect("no overflow");
    // Too deep to follow all the way: the depths below become one value.
    assert!(
        drafted.as_deref().is_ok_and(|p| p.starts_with("f(")),
        "{drafted:?}"
    );
}

#[test]
fn a_check_wrapped_in_parentheses_keeps_them() {
    let source = "void f(char *b, int n, int m) {\n  if ((n & 1) && (n > m)) return;\n  memcpy(b, b, n);\n}\n";
    let draft = draft_from(&[(Example::line("a.c", 3), source)], true).expect("draft");
    let check =
        exception_from(source, "a.c", 2, &draft.names, draft.callee.as_deref()).expect("exception");
    assert_eq!(check.pattern, "if (<... ($B & 1) && ($B > $C) ...>) ...");
}

#[test]
fn blocks_as_long_as_each_other_still_line_up_their_statements() {
    let source = "\
void a(int *p) {
  if (p) {
    lock();
    free(p);
    return;
  }
}
void b(int *q) {
  if (q) {
    lock();
    free(q);
    q = 0;
  }
}
";
    let places = [
        (Example::lines("a.c", 2, 6), source),
        (Example::lines("a.c", 9, 13), source),
    ];
    let draft = draft_from(&places, true).expect("draft");
    assert!(
        draft.same_shape.contains("lock();\nfree($A);"),
        "{}",
        draft.same_shape
    );
}
