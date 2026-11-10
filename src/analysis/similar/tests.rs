use super::*;
use crate::analysis::semgrep_rule::quoted;

/// A rule drafted from one line, as it reads with `looseness`.
#[derive(Debug)]
struct RuleDraft {
    id: String,
    pattern: String,
    yaml: String,
}

fn draft(source: &str, file: &str, line: u32, looseness: Looseness) -> Result<RuleDraft, String> {
    let draft = draft_from(&[(Example::line(file, line), source)], true)?;
    Ok(RuleDraft {
        id: draft.id.clone(),
        pattern: draft.pattern(looseness).to_string(),
        yaml: draft.yaml(looseness, &[]),
    })
}

/// The pattern drafted from line `line` of `source`.
fn pattern(source: &str, line: u32, looseness: Looseness) -> String {
    draft(source, "src/net.c", line, looseness)
        .expect("draft")
        .pattern
}

fn shape(code: &str) -> String {
    pattern(
        &format!("void f(void) {{\n  {code}\n}}\n"),
        2,
        Looseness::SameShape,
    )
}

fn loose(code: &str) -> String {
    pattern(
        &format!("void f(void) {{\n  {code}\n}}\n"),
        2,
        Looseness::AnyValues,
    )
}

#[test]
fn same_shape_turns_variables_into_metavariables() {
    assert_eq!(
        shape("memcpy(buf, pkt->data, ntohs(hdr->len));"),
        "memcpy($A, $B->data, ntohs($C->len))"
    );
    // The same variable is the same metavariable.
    assert_eq!(
        shape("memcpy(d, s, strlen(s));"),
        "memcpy($A, $B, strlen($B))"
    );
}

#[test]
fn called_names_macros_types_and_literals_stay() {
    assert_eq!(
        shape("strncpy(dst, src, MAX_LEN - 1);"),
        "strncpy($A, $B, MAX_LEN - 1)"
    );
    assert_eq!(
        shape("n = std::min(len, sizeof(struct hdr));"),
        "std::min($A, sizeof(struct hdr))"
    );
    assert_eq!(shape("conn->send(buf, 0x10);"), "$A->send($B, 0x10)");
    assert_eq!(shape("printf(\"%s\\n\", name);"), "printf(\"%s\\n\", $A)");
}

#[test]
fn any_values_keeps_only_the_call() {
    assert_eq!(
        loose("memcpy(buf, \"x\", ntohs(len));"),
        "memcpy($A, \"...\", ntohs(...))"
    );
    assert_eq!(
        loose("memcpy(buf, src, len * ntohs(n));"),
        "memcpy($A, $B, <... ntohs(...) ...>)"
    );
    // The same argument is the same metavariable.
    assert_eq!(loose("memcpy(a, a, 4);"), "memcpy($A, $A, $B)");
}

#[test]
fn without_a_call_the_statement_is_used() {
    assert_eq!(shape("char buf[64];"), "char $A[64];");
    assert_eq!(shape("total = total + LEN;"), "$A = $A + LEN");
    assert_eq!(loose("return size * 2;"), "return $A * 2;");
}

#[test]
fn code_of_variables_alone_is_refused() {
    for code in ["return len;", "x = y;", "a = b + c;"] {
        let source = format!("void f(void) {{\n  {code}\n}}\n");
        let error = draft(&source, "a.c", 2, Looseness::SameShape).expect_err(code);
        assert!(error.contains("match almost anything"), "{error}");
    }
}

#[test]
fn literals_that_read_as_pattern_syntax_become_any_string() {
    assert_eq!(shape("puts(\"$HOME\");"), "puts(\"...\")");
    assert_eq!(shape("strcmp(s, \"...\");"), "strcmp($A, \"...\")");
    assert_eq!(shape("strcmp(s, \"=~/x/\");"), "strcmp($A, \"...\")");
    // A name with a `$` in it is a variable, never a metavariable.
    assert_eq!(shape("strcpy(a$B, s);"), "strcpy($A, $B)");
}

#[test]
fn deeply_nested_code_does_not_overflow_the_stack() {
    let depth = 5_000;
    let code = format!("f(1 + {}g(x){});", "(".repeat(depth), ")".repeat(depth));
    let source = format!("void h(void) {{\n  {code}\n}}\n");
    // Too long for a rule, but found and walked without recursion.
    let error = draft(&source, "a.c", 2, Looseness::AnyValues).expect_err("too long");
    assert!(error.contains("too long"), "{error}");
}

#[test]
fn a_call_over_lines_is_one_pattern_from_any_of_its_lines() {
    let source = "void f(void) {\n  memcpy(dst,\n         src /* from wire */,\n         n);\n}\n";
    let from_first = pattern(source, 2, Looseness::SameShape);
    assert_eq!(from_first, "memcpy($A, $B , $C)");
    assert_eq!(pattern(source, 3, Looseness::SameShape), from_first);
}

#[test]
fn a_line_without_code_is_refused() {
    let source = "void f(void) {\n\n  // just a comment\n}\n";
    assert!(draft(source, "a.c", 2, Looseness::SameShape).is_err());
    assert!(draft(source, "a.c", 3, Looseness::SameShape).is_err());
    assert!(draft(source, "a.c", 0, Looseness::SameShape).is_err());
    assert!(draft(source, "a.c", 99, Looseness::SameShape).is_err());
}

#[test]
fn the_rule_names_the_call_and_line() {
    let source = "void f(void) {\n  ns::Copy(a, b);\n}\n";
    let rule = draft(source, "src/weird name!.c", 2, Looseness::SameShape).expect("draft");
    assert_eq!(rule.id, "like-ns-copy-2");
    let rule = draft("int x;\n", "src/My File.c", 1, Looseness::SameShape).expect("draft");
    assert_eq!(rule.id, "like-my-file-1");
    assert_eq!(
        rule.yaml,
        "rules:\n  - id: like-my-file-1\n    languages: [c, cpp]\n    severity: WARNING\n    \
         message: \"Like src/My File.c:1: int x;\"\n    pattern: \"int $A;\"\n"
    );
}

#[test]
fn nothing_from_the_code_can_break_out_of_the_rule() {
    // A raw string may hold quotes and line breaks.
    let source = "void f(void) {\n  puts(R\"(a\"\n  - id: evil\u{202E})\");\n}\n";
    let rule = draft(source, "x\"\n  - id: evil.c", 2, Looseness::SameShape).expect("draft");
    assert_eq!(rule.yaml.lines().count(), 6, "{}", rule.yaml);
    assert!(rule.yaml.contains("\\u202E"), "{}", rule.yaml);
    assert!(rule.yaml.lines().all(|line| line.is_ascii()));
    assert_eq!(
        quoted("a\"b\\c\nd\u{1F600}"),
        "\"a\\\"b\\\\c\\nd\\U0001F600\""
    );
}

#[test]
fn many_variables_still_get_names() {
    let args: Vec<String> = (0..30).map(|i| format!("v{i}")).collect();
    let pattern = shape(&format!("f({});", args.join(", ")));
    assert!(pattern.starts_with("f($A, $B,"), "{pattern}");
    assert!(pattern.contains("$Z, $V27"), "{pattern}");
}

/// Runs the real semgrep on the drafted rules: `cargo test -- --ignored`.
#[test]
#[ignore = "needs semgrep installed; slow"]
fn drafted_rules_find_the_variants() {
    let source = "#include <string.h>\n\
        struct pkt { char *data; char *body; };\n\
        void a(char *buf, const struct pkt *p, unsigned short n) {\n\
          memcpy(buf, p->data, ntohs(n));\n\
        }\n\
        void b(char *out, const struct pkt *in, unsigned short m) {\n\
          memcpy(out, in->data, ntohs(m));\n\
          memcpy(out, in->data, 4);\n\
          memcpy(out, in->body, ntohs(m + 1));\n\
        }\n";
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::write(dir.path().join("v.c"), source).expect("write");
    let lines_found = |looseness| -> Vec<u32> {
        let rule = draft(source, "v.c", 4, looseness).expect("draft");
        let rules = dir.path().join(format!("{}.yaml", rule.id));
        std::fs::write(&rules, &rule.yaml).expect("rules");
        let (program, args) = crate::analysis::tools::Tool::Semgrep.command(
            &dir.path().join("v.c"),
            &[rules.to_string_lossy().into_owned()],
        );
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .expect("semgrep");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let found = crate::analysis::tools::Tool::Semgrep
            .parse(&stdout, "", dir.path())
            .expect("parse");
        let mut lines: Vec<u32> = found.iter().map(|f| f.line).collect();
        lines.sort_unstable();
        lines
    };
    assert_eq!(lines_found(Looseness::SameShape), [4, 7]);
    assert_eq!(lines_found(Looseness::AnyValues), [4, 7, 9]);
}
