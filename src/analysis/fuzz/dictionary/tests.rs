//! Tests for drafting fuzzing dictionaries.

use super::literal::{char_constants, is_format, string_bytes};
use super::*;
use crate::index::graph_of_files;

const PARSER: &str = r#"
static int is_sep(char c) { return c == ':' || c == ';'; }

static void note(const char *m) { puts(m); }

int parse(const char *s, size_t n) {
  if (strncmp(s, "PING", 4) == 0) return 1;
  if (strchr(s, '=')) return 2;
  printf("bad input %s\n", s);
  note("CAP LS");
  return is_sep(s[0]) ? 3 : 0;
}
"#;

fn dictionary_for(sources: &[(&str, &str)], name: &str) -> Option<Dictionary> {
    let graph = graph_of_files(sources);
    let function = graph
        .functions
        .iter()
        .find(|f| graph.symbols[&f.symbol].qualified_name == name)
        .unwrap_or_else(|| panic!("no definition of {name}"));
    draft(&graph, function)
}

/// The token lines, in order.
fn tokens(dictionary: &Dictionary) -> Vec<&str> {
    dictionary
        .text
        .lines()
        .filter(|l| l.starts_with('"'))
        .collect()
}

#[test]
fn compared_constants_come_first_then_the_functions_own_then_its_callees() {
    let dictionary = dictionary_for(&[("src/irc.c", PARSER)], "parse").expect("tokens");
    assert_eq!(dictionary.file_name, "fuzz_parse.dict");
    assert_eq!(
        tokens(&dictionary),
        [r#""PING""#, r#""=""#, r#""CAP LS""#, r#"":""#, r#"";""#]
    );
    assert_eq!(dictionary.token_count, 5);
    assert!(dictionary.text.contains("afl-fuzz -x fuzz_parse.dict"));
    assert!(dictionary.text.contains("-dict=fuzz_parse.dict"));
}

#[test]
fn format_strings_are_left_out() {
    let dictionary = dictionary_for(&[("src/irc.c", PARSER)], "parse").expect("tokens");
    assert!(!dictionary.text.contains("bad input"));
    assert!(is_format(b"bad input %s\n"));
    assert!(is_format(b"%-08.3lu"));
    assert!(!is_format(b"100%"));
    assert!(!is_format(b"50%% off"));
}

#[test]
fn a_function_without_constants_has_no_dictionary() {
    let code = "int add(int a, int b) { return a + b; }";
    assert_eq!(dictionary_for(&[("a.c", code)], "add"), None);
}

#[test]
fn string_literals_are_read_as_the_compiler_would() {
    assert_eq!(
        string_bytes(r#""a\n\x41\101\"""#),
        Some(b"a\nAA\"".to_vec())
    );
    assert_eq!(string_bytes(r#""ab" "cd""#), Some(b"abcd".to_vec()));
    assert_eq!(string_bytes(r#"u8"x""#), Some(b"x".to_vec()));
    assert_eq!(string_bytes(r#"R"d(a\b)d""#), Some(br"a\b".to_vec()));
    assert_eq!(string_bytes(r#""x"_s"#), Some(b"x".to_vec()));
    assert_eq!(string_bytes(r#""é""#), Some("é".as_bytes().to_vec()));
    // Wide strings, cut-short text, macros and expressions aren't bytes.
    assert_eq!(string_bytes(r#"L"x""#), None);
    assert_eq!(string_bytes("\"abc…"), None);
    assert_eq!(string_bytes(r#"PREFIX "x""#), None);
    assert_eq!(string_bytes("buf + 6"), None);
    assert_eq!(string_bytes(r#""\x100""#), None);
}

#[test]
fn character_constants_are_found_outside_comments_and_strings() {
    let code = r#"if (c == ':' || c == '\r') n = 1'000; /* ',' */ s = "';'"; // '#'
        t = L'w'; u = '\'';"#;
    assert_eq!(
        char_constants(code),
        [b":".to_vec(), b"\r".to_vec(), b"'".to_vec()]
    );
}

#[test]
fn tokens_are_escaped_so_any_bytes_survive() {
    assert_eq!(token_line(b"PING"), r#""PING""#);
    assert_eq!(
        token_line("\r\n\"\\\u{1}é".as_bytes()),
        r#""\x0D\x0A\"\\\x01\xC3\xA9""#
    );
}

#[test]
fn long_tokens_are_left_out_and_the_count_is_capped() {
    let long = "x".repeat(MAX_TOKEN_BYTES + 1);
    let many: String = (0..MAX_TOKENS + 10)
        .map(|i| format!("  use(\"tok{i}\");\n"))
        .collect();
    let code = format!("void f(void) {{\n  use(\"{long}\");\n{many}}}\n");
    let dictionary = dictionary_for(&[("a.c", &code)], "f").expect("tokens");
    assert!(!dictionary.text.contains(&long));
    assert_eq!(dictionary.token_count, MAX_TOKENS);
    assert_eq!(tokens(&dictionary).len(), MAX_TOKENS);
    assert!(dictionary.text.contains("more were left out"));
}

#[test]
fn names_in_comments_cannot_add_lines() {
    let code = "int parse(const char *s) { return strcmp(s, \"GET\"); }";
    let graph = graph_of_files(&[("we\nird\".c", code)]);
    let function = &graph.functions[0];
    let dictionary = draft(&graph, function).expect("tokens");
    assert_eq!(tokens(&dictionary), [r#""GET""#]);
    assert!(dictionary
        .text
        .lines()
        .all(|l| l.is_empty() || l.starts_with('#') || l.starts_with('"')));
}

#[test]
fn a_set_of_characters_gives_each_one() {
    let code = r#"int split(char *s) {
        return strcspn(s, " \t") + strpbrk(s, ",;") + (strchr("@!", *s) != 0);
    }"#;
    let dictionary = dictionary_for(&[("a.c", code)], "split").expect("tokens");
    assert_eq!(
        tokens(&dictionary),
        [
            r#"" ""#,
            r#""\x09""#,
            r#"",""#,
            r#"";""#,
            r#""@""#,
            r#""!""#
        ]
    );
}

#[test]
fn a_lone_nul_byte_is_not_worth_a_token() {
    let code = r#"int end(const char *s) { return *s == '\0' ? 0 : strcmp(s, "\0x"); }"#;
    let dictionary = dictionary_for(&[("a.c", code)], "end").expect("tokens");
    assert_eq!(tokens(&dictionary), [r#""\x00x""#]);
}

#[test]
fn an_unclosed_string_ends_at_its_line_and_costs_one_pass() {
    assert_eq!(char_constants("x = \"abc ':' \n c == ';'"), [b";".to_vec()]);
    assert_eq!(char_constants("ERR\"':'\" c == ';'"), [b";".to_vec()]);
    let hostile = "\\\"".repeat(100_000) + &"R\"(".repeat(50_000);
    let started = std::time::Instant::now();
    assert!(char_constants(&format!("\"{hostile}")).is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "linear"
    );
}

#[test]
fn prefixes_and_line_splices_are_read_as_written() {
    assert_eq!(string_bytes(r#""a"L"b""#), None);
    assert_eq!(string_bytes(r#""a"u8"b""#), Some(b"ab".to_vec()));
    assert_eq!(string_bytes(r#""x"s"#), Some(b"x".to_vec()));
    assert_eq!(string_bytes("\"a\\\nb\""), Some(b"ab".to_vec()));
}

#[test]
fn scanf_formats_and_reused_sets_are_handled() {
    let code = r#"int cfg(const char *s, int *v) {
        if (strcmp(s, " \t") == 0) return 1;
        return sscanf(s, "%d", v) + strspn(s, " \t");
    }"#;
    let dictionary = dictionary_for(&[("a.c", code)], "cfg").expect("tokens");
    assert_eq!(tokens(&dictionary), [r#"" \x09""#, r#"" ""#, r#""\x09""#]);
}
